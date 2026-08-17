//! Structured-output configuration and framework-owned control-tool identities.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::primitives::Digest;
use crate::primitives::RawJson;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{EffectId, MessageId, ModelRequestId, ToolCallId, TurnId};

/// Reserved namespace for framework-owned tools.
pub const INTERNAL_TOOL_NAMESPACE: &str = "finstack.internal.";
/// Framework-owned tool used to submit one structured final result.
pub const SUBMIT_FINAL_OUTPUT_TOOL: &str = "finstack.internal.submit_final_output";
/// Reserved framework-owned tool used to request model-driven capability activation.
pub const LOAD_CAPABILITY_TOOL: &str = "finstack.internal.load_capability";

/// JSON Schema dialect accepted by the structured-output contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonSchemaDraft {
    /// JSON Schema draft 2020-12.
    Draft202012,
}

/// Immutable reference to a canonical JSON Schema document.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaRef {
    /// Frozen schema dialect.
    pub draft: JsonSchemaDraft,
    /// Application-owned schema revision.
    pub schema_version: u16,
    /// Digest of the canonical schema document.
    pub schema_digest: Digest,
}

impl<'de> Deserialize<'de> for SchemaRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            draft: JsonSchemaDraft,
            schema_version: u16,
            schema_digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.schema_version == 0 {
            return Err(serde::de::Error::custom("schema_version_must_be_positive"));
        }
        Ok(Self {
            draft: wire.draft,
            schema_version: wire.schema_version,
            schema_digest: wire.schema_digest,
        })
    }
}

/// Final-output mode for one run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum OutputSpec {
    /// The final assistant message is accepted without structured validation.
    #[default]
    PlainText,
    /// A final candidate must validate against the referenced JSON Schema.
    JsonSchema {
        /// Canonical schema reference supplied to the external validator.
        schema: SchemaRef,
    },
}

/// How application tool calls compete with a valid structured final result.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputEndStrategy {
    /// A valid final result wins and application tool calls in the same response are skipped.
    Early,
    /// Application tool calls complete before the valid final result is finalized.
    #[default]
    Exhaustive,
}

/// Durable run-level output configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfiguration {
    /// Requested output mode.
    #[serde(default)]
    pub output: OutputSpec,
    /// Competition policy for structured output and application tools.
    #[serde(default)]
    pub end_strategy: OutputEndStrategy,
}

impl OutputConfiguration {
    /// Validate intrinsic configuration invariants shared by decide, replay, and snapshots.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if matches!(
            self.output,
            OutputSpec::JsonSchema {
                schema: SchemaRef {
                    schema_version: 0,
                    ..
                }
            }
        ) {
            return Err("schema_version_must_be_positive");
        }
        Ok(())
    }
}

/// Assistant content location from which a structured result was extracted.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum StructuredResultSource {
    /// A canonical JSON content block at the exact zero-based content index.
    JsonBlock {
        /// Zero-based message content index.
        content_index: u32,
    },
    /// The arguments of the framework-owned final-output tool call.
    InternalTool {
        /// Exact source tool-call identity.
        tool_call_id: ToolCallId,
    },
}

/// Replay-complete valid structured result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FinalResultRecorded {
    /// Model cycle that produced the result.
    pub cycle: u64,
    /// Originating turn.
    pub turn_id: TurnId,
    /// Originating model request.
    pub model_request_id: ModelRequestId,
    /// Originating model effect.
    pub effect_id: EffectId,
    /// Durable assistant message containing the result.
    pub message_id: MessageId,
    /// Schema used by the external validator.
    pub schema: SchemaRef,
    /// Exact canonical result value.
    pub value: RawJson,
    /// Digest of `value`.
    pub value_digest: Digest,
    /// Exact assistant content source.
    pub source: StructuredResultSource,
    /// Configured competition policy.
    pub end_strategy: OutputEndStrategy,
    /// Application calls intentionally skipped by `Early`.
    pub skipped_tool_call_ids: Arc<[ToolCallId]>,
}

impl FinalResultRecorded {
    /// Validate fields that do not require the surrounding journal state.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.schema.schema_version == 0 {
            return Err("schema_version_must_be_positive");
        }
        if self.value_digest != self.value.digest() {
            return Err("value_digest_mismatch");
        }
        if self.skipped_tool_call_ids.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err("too_many_skipped_tool_calls");
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for FinalResultRecorded {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            cycle: u64,
            turn_id: TurnId,
            model_request_id: ModelRequestId,
            effect_id: EffectId,
            message_id: MessageId,
            schema: SchemaRef,
            value: RawJson,
            value_digest: Digest,
            source: StructuredResultSource,
            end_strategy: OutputEndStrategy,
            skipped_tool_call_ids: BoundedVec<ToolCallId, SEMANTIC_ARRAY_MAX_ITEMS>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            cycle: wire.cycle,
            turn_id: wire.turn_id,
            model_request_id: wire.model_request_id,
            effect_id: wire.effect_id,
            message_id: wire.message_id,
            schema: wire.schema,
            value: wire.value,
            value_digest: wire.value_digest,
            source: wire.source,
            end_strategy: wire.end_strategy,
            skipped_tool_call_ids: Arc::from(wire.skipped_tool_call_ids.into_inner()),
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

/// Whether a tool name belongs to the reserved framework namespace.
#[must_use]
pub fn is_internal_tool_name(name: &str) -> bool {
    name.starts_with(INTERNAL_TOOL_NAMESPACE)
}
