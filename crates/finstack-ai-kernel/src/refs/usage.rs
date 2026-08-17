//! Cost and usage counters.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES};

use super::error::{RefsError, deserialize_micros, serialize_micros, validated_label};
use crate::primitives::BoundedMap;
use crate::primitives::LimitKey;
use std::collections::BTreeMap;

/// Recorded cost amount (integer millionths; JSON decimal string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CostAmount {
    unit: Arc<str>,
    #[serde(serialize_with = "serialize_micros")]
    micros: u64,
    pricing_policy_version: Arc<str>,
}

impl CostAmount {
    /// Construct a cost amount.
    ///
    /// # Arguments
    ///
    /// * `unit` - Cost-unit label (for example a currency or token unit).
    /// * `micros` - Amount in millionths of `unit`.
    /// * `pricing_policy_version` - Pricing-policy version that produced the amount.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when unit/policy labels fail rules.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::CostAmount;
    ///
    /// let cost = CostAmount::try_new("USD", 250_000, "pricing-v1").expect("cost");
    /// assert_eq!(cost.unit(), "USD");
    /// assert_eq!(cost.micros(), 250_000);
    /// ```
    pub fn try_new(
        unit: impl AsRef<str>,
        micros: u64,
        pricing_policy_version: impl AsRef<str>,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            unit: validated_label(unit.as_ref(), "unit")?,
            micros,
            pricing_policy_version: validated_label(
                pricing_policy_version.as_ref(),
                "pricing_policy_version",
            )?,
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
}

impl<'de> Deserialize<'de> for CostAmount {
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
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.unit.into_inner(),
            wire.micros,
            wire.pricing_policy_version.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}

/// Normalized token/cost usage for effect completion and budget charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Usage {
    /// Optional input token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) input_tokens: Option<u64>,
    /// Optional output token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) output_tokens: Option<u64>,
    /// Optional total token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) total_tokens: Option<u64>,
    /// Optional recorded cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) cost: Option<CostAmount>,
    /// Namespaced extension counters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) extension_counters: BTreeMap<LimitKey, u64>,
}

impl Usage {
    /// V1 maximum registered extension counters per resolved agent.
    pub const MAX_EXTENSION_COUNTERS: usize = 32;

    /// Empty usage.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cost: None,
            extension_counters: BTreeMap::new(),
        }
    }

    /// Construct validated normalized usage.
    ///
    /// # Arguments
    ///
    /// * `input_tokens` - Optional input-token count; `None` means unused.
    /// * `output_tokens` - Optional output-token count; `None` means unused.
    /// * `total_tokens` - Optional total-token count; `None` means unused.
    /// * `cost` - Optional normalized cost; `None` means unused.
    /// * `extension_counters` - Additional named counters, bounded by the v1 map
    ///   ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::TooManyEntries`] when extension counters exceed the v1 ceiling.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::Usage;
    ///
    /// let usage = Usage::try_new(Some(10), Some(4), Some(14), None, Default::default())
    ///     .expect("usage");
    /// assert_eq!(usage.total_tokens(), Some(14));
    /// ```
    pub fn try_new(
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        cost: Option<CostAmount>,
        extension_counters: BTreeMap<LimitKey, u64>,
    ) -> Result<Self, RefsError> {
        if extension_counters.len() > Self::MAX_EXTENSION_COUNTERS {
            return Err(RefsError::TooManyEntries {
                field: "usage.extension_counters",
                len: extension_counters.len(),
                max: Self::MAX_EXTENSION_COUNTERS,
            });
        }
        Ok(Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cost,
            extension_counters,
        })
    }

    /// Validate collection ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::TooManyEntries`] when extension counters exceed the v1 ceiling.
    pub fn validate(&self) -> Result<(), RefsError> {
        if self.extension_counters.len() > Self::MAX_EXTENSION_COUNTERS {
            return Err(RefsError::TooManyEntries {
                field: "usage.extension_counters",
                len: self.extension_counters.len(),
                max: Self::MAX_EXTENSION_COUNTERS,
            });
        }
        Ok(())
    }

    /// Input token count.
    #[must_use]
    pub fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    /// Output token count.
    #[must_use]
    pub fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }

    /// Total token count.
    #[must_use]
    pub fn total_tokens(&self) -> Option<u64> {
        self.total_tokens
    }

    /// Recorded cost.
    #[must_use]
    pub fn cost(&self) -> Option<&CostAmount> {
        self.cost.as_ref()
    }

    /// Extension counters.
    #[must_use]
    pub fn extension_counters(&self) -> &BTreeMap<LimitKey, u64> {
        &self.extension_counters
    }

    /// Canonical JSON bytes for digesting usage under effect domains.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::Serialize`] when serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RefsError> {
        serde_json_canonicalizer::to_vec(self).map_err(|error| RefsError::Serialize {
            detail: error.to_string(),
        })
    }
}

impl<'de> Deserialize<'de> for Usage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            input_tokens: Option<u64>,
            #[serde(default)]
            output_tokens: Option<u64>,
            #[serde(default)]
            total_tokens: Option<u64>,
            #[serde(default)]
            cost: Option<CostAmount>,
            #[serde(default)]
            extension_counters: BoundedMap<LimitKey, u64, { Usage::MAX_EXTENSION_COUNTERS }>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.input_tokens,
            wire.output_tokens,
            wire.total_tokens,
            wire.cost,
            wire.extension_counters.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}
