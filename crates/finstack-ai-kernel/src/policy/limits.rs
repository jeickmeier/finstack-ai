//! Value-only run limits and cost policy (TDD §22.1).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::primitives::BoundedMap;
use crate::primitives::Digest;
use crate::primitives::Duration;
use crate::primitives::LimitKey;
use crate::refs::{CostAmount, RefsError, validated_label};

/// Limit dimension used by reserved limit-reached surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LimitDimension {
    /// Model requests.
    ModelRequests,
    /// Turns.
    Turns,
    /// Tool calls.
    ToolCalls,
    /// Parallel tools.
    ParallelTools,
    /// Input tokens.
    InputTokens,
    /// Output tokens.
    OutputTokens,
    /// Context bytes.
    ContextBytes,
    /// Output bytes.
    OutputBytes,
    /// Retries.
    Retries,
    /// Wall time.
    WallTime,
    /// Cost.
    Cost,
    /// Extension counter.
    Extension {
        /// Namespaced key.
        key: LimitKey,
    },
}

/// Typed observed or configured value carried by [`LimitReached`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitValue {
    /// Count-like dimension.
    Count(u64),
    /// Byte dimension.
    Bytes(u64),
    /// Duration dimension.
    Duration(Duration),
    /// Exact integer-micro-unit cost dimension.
    Cost(CostAmount),
}

/// Replay-derived cumulative limit usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LimitUsage {
    /// Committed model requests.
    pub model_requests: u64,
    /// Committed turns.
    pub turns: u64,
    /// Accepted tool calls.
    pub tool_calls: u64,
    /// Largest admitted tool execution group.
    pub max_parallel_tools: u32,
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
    /// Cumulative canonical context bytes.
    pub context_bytes: u64,
    /// Cumulative canonical output bytes.
    pub output_bytes: u64,
    /// Additional semantic retry attempts.
    pub retries: u32,
    /// Latest replay-derived wall duration.
    pub wall_time: Duration,
    /// Cumulative exact cost, when a cost policy is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostAmount>,
    /// Cumulative registered extension counters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extension_counters: BTreeMap<LimitKey, u64>,
}

impl Default for LimitUsage {
    fn default() -> Self {
        Self {
            model_requests: 0,
            turns: 0,
            tool_calls: 0,
            max_parallel_tools: 0,
            input_tokens: 0,
            output_tokens: 0,
            context_bytes: 0,
            output_bytes: 0,
            retries: 0,
            wall_time: Duration::from_millis(0),
            cost: None,
            extension_counters: BTreeMap::new(),
        }
    }
}

impl LimitUsage {
    /// Compute canonical bytes for replay-stable limit evidence.
    ///
    /// # Errors
    ///
    /// Returns [`LimitsError::Serialize`] when canonicalization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, LimitsError> {
        serde_json_canonicalizer::to_vec(self).map_err(|_| LimitsError::Serialize)
    }

    /// Compute the durable usage digest.
    ///
    /// # Errors
    ///
    /// Returns [`LimitsError::Serialize`] when canonicalization or hashing fails.
    pub fn digest(&self) -> Result<Digest, LimitsError> {
        let bytes = self.canonical_bytes()?;
        Digest::domain_separated("limit-usage", 1, &bytes).map_err(|_| LimitsError::Serialize)
    }
}

impl<'de> Deserialize<'de> for LimitUsage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            model_requests: u64,
            turns: u64,
            tool_calls: u64,
            max_parallel_tools: u32,
            input_tokens: u64,
            output_tokens: u64,
            context_bytes: u64,
            output_bytes: u64,
            retries: u32,
            wall_time: Duration,
            #[serde(default)]
            cost: Option<CostAmount>,
            #[serde(default)]
            extension_counters: BoundedMap<LimitKey, u64, { RunLimits::MAX_EXTENSION_COUNTERS }>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            model_requests: wire.model_requests,
            turns: wire.turns,
            tool_calls: wire.tool_calls,
            max_parallel_tools: wire.max_parallel_tools,
            input_tokens: wire.input_tokens,
            output_tokens: wire.output_tokens,
            context_bytes: wire.context_bytes,
            output_bytes: wire.output_bytes,
            retries: wire.retries,
            wall_time: wire.wall_time,
            cost: wire.cost,
            extension_counters: wire.extension_counters.into_inner(),
        })
    }
}

/// Durable evidence that one configured limit was crossed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LimitReached {
    /// Crossed dimension.
    pub dimension: LimitDimension,
    /// First representable observation above the maximum.
    pub observed: LimitValue,
    /// Configured maximum.
    pub maximum: LimitValue,
    /// Complete bounded cumulative usage at the decision boundary.
    pub usage: LimitUsage,
    /// Digest of cumulative usage at the decision boundary.
    pub usage_digest: Digest,
}

impl LimitReached {
    /// Validate that observed and maximum values match the dimension family.
    ///
    /// # Errors
    ///
    /// Returns [`LimitsError::ValueKindMismatch`] for mismatched value variants.
    pub fn validate(&self) -> Result<(), LimitsError> {
        let matches = match self.dimension {
            LimitDimension::ContextBytes | LimitDimension::OutputBytes => {
                matches!(
                    (&self.observed, &self.maximum),
                    (LimitValue::Bytes(_), LimitValue::Bytes(_))
                )
            }
            LimitDimension::WallTime => {
                matches!(
                    (&self.observed, &self.maximum),
                    (LimitValue::Duration(_), LimitValue::Duration(_))
                )
            }
            LimitDimension::Cost => {
                matches!(
                    (&self.observed, &self.maximum),
                    (LimitValue::Cost(_), LimitValue::Cost(_))
                )
            }
            _ => matches!(
                (&self.observed, &self.maximum),
                (LimitValue::Count(_), LimitValue::Count(_))
            ),
        };
        if !matches {
            return Err(LimitsError::ValueKindMismatch);
        }
        if self.usage.digest()? != self.usage_digest {
            return Err(LimitsError::UsageDigestMismatch);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for LimitReached {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            dimension: LimitDimension,
            observed: LimitValue,
            maximum: LimitValue,
            usage: LimitUsage,
            usage_digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            dimension: wire.dimension,
            observed: wire.observed,
            maximum: wire.maximum,
            usage: wire.usage,
            usage_digest: wire.usage_digest,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

/// Unknown-usage policy for cost limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownUsagePolicy {
    /// Fail closed when usage is unknown.
    FailClosed,
    /// Suspend for an operator/application decision.
    SuspendForDecision,
    /// Allow within a reserved maximum.
    AllowWithinReservedMaximum,
}

/// Cost ceiling with pricing policy metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CostLimit {
    unit: Arc<str>,
    #[serde(serialize_with = "serialize_micros")]
    micros: u64,
    pricing_policy_version: Arc<str>,
    unknown_usage: UnknownUsagePolicy,
}

impl CostLimit {
    /// Construct a cost limit.
    ///
    /// # Errors
    ///
    /// Returns [`LimitsError::InvalidLabel`] when unit/policy labels fail rules.
    pub fn try_new(
        unit: impl AsRef<str>,
        micros: u64,
        pricing_policy_version: impl AsRef<str>,
        unknown_usage: UnknownUsagePolicy,
    ) -> Result<Self, LimitsError> {
        Ok(Self {
            unit: validated_label(unit.as_ref(), "unit").map_err(LimitsError::from)?,
            micros,
            pricing_policy_version: validated_label(
                pricing_policy_version.as_ref(),
                "pricing_policy_version",
            )
            .map_err(LimitsError::from)?,
            unknown_usage,
        })
    }

    /// Borrow the unit.
    #[must_use]
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// Return micros.
    #[must_use]
    pub fn micros(&self) -> u64 {
        self.micros
    }

    /// Borrow the pricing policy version.
    #[must_use]
    pub fn pricing_policy_version(&self) -> &str {
        &self.pricing_policy_version
    }

    /// Return unknown-usage policy.
    #[must_use]
    pub fn unknown_usage(&self) -> UnknownUsagePolicy {
        self.unknown_usage
    }
}

impl<'de> Deserialize<'de> for CostLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            unit: BoundedString<LABEL_MAX_BYTES>,
            #[serde(deserialize_with = "deserialize_micros")]
            micros: u64,
            pricing_policy_version: BoundedString<LABEL_MAX_BYTES>,
            unknown_usage: UnknownUsagePolicy,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.unit.into_inner(),
            wire.micros,
            wire.pricing_policy_version.into_inner(),
            wire.unknown_usage,
        )
        .map_err(de::Error::custom)
    }
}

/// Value-only run limits stored on `RunAccepted` (enforcement is PR-011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunLimits {
    /// Max model requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_model_requests: Option<u64>,
    /// Max turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u64>,
    /// Max tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u64>,
    /// Max parallel tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel_tools: Option<u32>,
    /// Max input tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    /// Max output tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Max context bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_bytes: Option<u64>,
    /// Max output bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<u64>,
    /// Max retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// Max wall time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_time: Option<Duration>,
    /// Max cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost: Option<CostLimit>,
    /// Extension counter ceilings.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extension_counters: BTreeMap<LimitKey, u64>,
}

impl RunLimits {
    /// V1 maximum registered extension counters per resolved agent.
    pub const MAX_EXTENSION_COUNTERS: usize = 32;

    /// Empty limits (no ceilings).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            max_model_requests: None,
            max_turns: None,
            max_tool_calls: None,
            max_parallel_tools: None,
            max_input_tokens: None,
            max_output_tokens: None,
            max_context_bytes: None,
            max_output_bytes: None,
            max_retries: None,
            max_wall_time: None,
            max_cost: None,
            extension_counters: BTreeMap::new(),
        }
    }

    /// True when every field of `child` is less than or equal to `self` where both are set.
    ///
    /// Absent parent ceilings do not constrain children. Present child ceilings must not
    /// exceed the parent. Extension keys present only on the child are allowed only when
    /// the parent map is empty; otherwise child keys must be a subset and values attenuated.
    #[must_use]
    pub fn allows_child_attenuation(&self, child: &Self) -> bool {
        opt_le(self.max_model_requests, child.max_model_requests)
            && opt_le(self.max_turns, child.max_turns)
            && opt_le(self.max_tool_calls, child.max_tool_calls)
            && opt_le(self.max_parallel_tools, child.max_parallel_tools)
            && opt_le(self.max_input_tokens, child.max_input_tokens)
            && opt_le(self.max_output_tokens, child.max_output_tokens)
            && opt_le(self.max_context_bytes, child.max_context_bytes)
            && opt_le(self.max_output_bytes, child.max_output_bytes)
            && opt_le(self.max_retries, child.max_retries)
            && opt_duration_le(self.max_wall_time, child.max_wall_time)
            && cost_le(self.max_cost.as_ref(), child.max_cost.as_ref())
            && counters_attenuated(&self.extension_counters, &child.extension_counters)
    }

    /// Validate collection ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`LimitsError::TooManyEntries`] when extension counters exceed the v1 ceiling.
    pub fn validate(&self) -> Result<(), LimitsError> {
        if self.extension_counters.len() > Self::MAX_EXTENSION_COUNTERS {
            return Err(LimitsError::TooManyEntries {
                field: "run_limits.extension_counters",
                len: self.extension_counters.len(),
                max: Self::MAX_EXTENSION_COUNTERS,
            });
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for RunLimits {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            max_model_requests: Option<u64>,
            #[serde(default)]
            max_turns: Option<u64>,
            #[serde(default)]
            max_tool_calls: Option<u64>,
            #[serde(default)]
            max_parallel_tools: Option<u32>,
            #[serde(default)]
            max_input_tokens: Option<u64>,
            #[serde(default)]
            max_output_tokens: Option<u64>,
            #[serde(default)]
            max_context_bytes: Option<u64>,
            #[serde(default)]
            max_output_bytes: Option<u64>,
            #[serde(default)]
            max_retries: Option<u32>,
            #[serde(default)]
            max_wall_time: Option<Duration>,
            #[serde(default)]
            max_cost: Option<CostLimit>,
            #[serde(default)]
            extension_counters: BoundedMap<LimitKey, u64, { RunLimits::MAX_EXTENSION_COUNTERS }>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let limits = Self {
            max_model_requests: wire.max_model_requests,
            max_turns: wire.max_turns,
            max_tool_calls: wire.max_tool_calls,
            max_parallel_tools: wire.max_parallel_tools,
            max_input_tokens: wire.max_input_tokens,
            max_output_tokens: wire.max_output_tokens,
            max_context_bytes: wire.max_context_bytes,
            max_output_bytes: wire.max_output_bytes,
            max_retries: wire.max_retries,
            max_wall_time: wire.max_wall_time,
            max_cost: wire.max_cost,
            extension_counters: wire.extension_counters.into_inner(),
        };
        limits.validate().map_err(de::Error::custom)?;
        Ok(limits)
    }
}

/// Limits validation errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LimitsError {
    /// Label failed validation.
    #[error(transparent)]
    InvalidLabel(#[from] RefsError),
    /// Semantic map exceeded its v1 entry ceiling.
    #[error("{field} has {len} entries; max {max}")]
    TooManyEntries {
        /// Field name.
        field: &'static str,
        /// Observed entry count.
        len: usize,
        /// Maximum entry count.
        max: usize,
    },
    /// Canonical limit evidence could not be represented.
    #[error("limit evidence serialization failed")]
    Serialize,
    /// Limit value variant does not match its dimension.
    #[error("limit value kind does not match dimension")]
    ValueKindMismatch,
    /// Limit usage snapshot does not match its digest.
    #[error("limit usage digest mismatch")]
    UsageDigestMismatch,
}

impl LimitsError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLabel(inner) => inner.code(),
            Self::TooManyEntries { .. } => "too_many_entries",
            Self::Serialize => "limit_serialize_failed",
            Self::ValueKindMismatch => "limit_value_kind_mismatch",
            Self::UsageDigestMismatch => "limit_usage_digest_mismatch",
        }
    }
}

fn opt_le<T: PartialOrd>(parent: Option<T>, child: Option<T>) -> bool {
    match (parent, child) {
        (Some(p), Some(c)) => c <= p,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

fn opt_duration_le(parent: Option<Duration>, child: Option<Duration>) -> bool {
    match (parent, child) {
        (Some(p), Some(c)) => c.as_millis() <= p.as_millis(),
        (None, _) => true,
        (Some(_), None) => false,
    }
}

fn cost_le(parent: Option<&CostLimit>, child: Option<&CostLimit>) -> bool {
    match (parent, child) {
        (Some(p), Some(c)) => {
            c.unit() == p.unit()
                && c.pricing_policy_version() == p.pricing_policy_version()
                && c.micros() <= p.micros()
                && unknown_usage_at_least_as_strict(p.unknown_usage(), c.unknown_usage())
        }
        (None, _) => true,
        (Some(_), None) => false,
    }
}

fn counters_attenuated(parent: &BTreeMap<LimitKey, u64>, child: &BTreeMap<LimitKey, u64>) -> bool {
    parent.iter().all(|(key, parent_value)| {
        child
            .get(key)
            .is_some_and(|child_value| child_value <= parent_value)
    })
}

fn unknown_usage_at_least_as_strict(parent: UnknownUsagePolicy, child: UnknownUsagePolicy) -> bool {
    unknown_usage_rank(child) <= unknown_usage_rank(parent)
}

const fn unknown_usage_rank(policy: UnknownUsagePolicy) -> u8 {
    match policy {
        UnknownUsagePolicy::FailClosed => 0,
        UnknownUsagePolicy::SuspendForDecision => 1,
        UnknownUsagePolicy::AllowWithinReservedMaximum => 2,
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn serialize_micros<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn deserialize_micros<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(de::Error::custom(
            "micros must be a canonical decimal string",
        ));
    }
    text.parse::<u64>()
        .map_err(|_| de::Error::custom("micros overflow or invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_limits_must_attenuate() {
        let parent = RunLimits {
            max_turns: Some(10),
            max_cost: Some(
                CostLimit::try_new("USD", 1000, "p1", UnknownUsagePolicy::FailClosed)
                    .expect("cost"),
            ),
            ..RunLimits::empty()
        };
        let ok = RunLimits {
            max_turns: Some(5),
            max_cost: Some(
                CostLimit::try_new("USD", 500, "p1", UnknownUsagePolicy::FailClosed).expect("cost"),
            ),
            ..RunLimits::empty()
        };
        let bad = RunLimits {
            max_turns: Some(11),
            ..RunLimits::empty()
        };
        assert!(parent.allows_child_attenuation(&ok));
        assert!(!parent.allows_child_attenuation(&bad));
    }

    #[test]
    fn child_cannot_remove_parent_ceilings_or_weaken_cost_policy() {
        let parent = RunLimits {
            max_turns: Some(10),
            max_wall_time: Some(Duration::from_millis(1_000)),
            max_cost: Some(
                CostLimit::try_new("USD", 1000, "p1", UnknownUsagePolicy::FailClosed)
                    .expect("cost"),
            ),
            extension_counters: BTreeMap::from([(LimitKey::parse("app.calls").expect("key"), 10)]),
            ..RunLimits::empty()
        };
        assert!(!parent.allows_child_attenuation(&RunLimits::empty()));

        let weaker_cost = RunLimits {
            max_turns: Some(10),
            max_wall_time: Some(Duration::from_millis(1_000)),
            max_cost: Some(
                CostLimit::try_new(
                    "USD",
                    1000,
                    "p1",
                    UnknownUsagePolicy::AllowWithinReservedMaximum,
                )
                .expect("cost"),
            ),
            extension_counters: parent.extension_counters.clone(),
            ..RunLimits::empty()
        };
        assert!(!parent.allows_child_attenuation(&weaker_cost));
    }

    #[test]
    fn child_must_preserve_parent_extension_ceilings_but_may_add_stricter_ones() {
        let calls = LimitKey::parse("app.calls").expect("key");
        let parent = RunLimits {
            extension_counters: BTreeMap::from([(calls.clone(), 10)]),
            ..RunLimits::empty()
        };
        let missing = RunLimits::empty();
        assert!(!parent.allows_child_attenuation(&missing));

        let child = RunLimits {
            extension_counters: BTreeMap::from([
                (calls, 5),
                (LimitKey::parse("app.extra").expect("key"), 1),
            ]),
            ..RunLimits::empty()
        };
        assert!(parent.allows_child_attenuation(&child));
    }

    #[test]
    fn run_limits_reject_more_than_32_extension_counters() {
        let limits = RunLimits {
            extension_counters: (0..33)
                .map(|index| {
                    (
                        LimitKey::parse(format!("app.counter-{index}")).expect("key"),
                        1,
                    )
                })
                .collect(),
            ..RunLimits::empty()
        };
        let json = serde_json::to_string(&limits).expect("serialize");
        let error = serde_json::from_str::<RunLimits>(&json).expect_err("over limit");
        assert!(error.to_string().contains("map entry count"));
    }
}
