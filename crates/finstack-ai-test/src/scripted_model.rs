//! Deterministic scripted input format for golden traces.
//!
//! Covers model chunks, tool calls/results, cancellation, errors, and timers
//! without requiring a live model or Phase 1 reducer.

use serde::{Deserialize, Serialize};

use crate::trace_fixture::PayloadDeclaration;

/// Kind discriminator for one scripted step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptedStepKind {
    /// One model text chunk.
    ModelChunk,
    /// Complete the current model request from accumulated text chunks.
    ModelCompleted,
    /// Defer the current model request under a non-secret external handle.
    ModelDeferred,
    /// Complete the deferred model request with final external text.
    ExternalCompleted,
    /// Continue from `before_finalize` into a fresh model cycle.
    BeforeFinalizeContinue,
    /// One tool invocation request.
    ToolCall,
    /// One tool result.
    ToolResult,
    /// Cancellation boundary.
    Cancellation,
    /// Provider or tool error.
    Error,
    /// Manual-clock timer fire.
    Timer,
}

/// One deterministic scripted outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptedStep {
    /// Step kind.
    pub kind: ScriptedStepKind,
    /// Optional stable identifier for correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Text payload for model chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tool name for call/result steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool arguments object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    /// Tool result payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Stable error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Human-readable diagnostic message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Timer duration in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Optional TDD §6.5 payload declaration ceilings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_declaration: Option<PayloadDeclaration>,
}

/// Scripted input sequence consumed by golden traces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptedInput {
    /// Format major version. Always `1` for this schema.
    pub format_version: u32,
    /// Ordered scripted steps.
    pub steps: Vec<ScriptedStep>,
}

impl ScriptedInput {
    /// Construct an empty scripted input.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            format_version: 1,
            steps: Vec::new(),
        }
    }
}
