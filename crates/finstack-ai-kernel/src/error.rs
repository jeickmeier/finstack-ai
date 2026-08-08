//! Stable, source-free error descriptors for durable and cross-language boundaries.

use core::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{
    AppendBatchId, ArtifactId, BudgetReservationId, BudgetScopeId, CancellationRequestId, EffectId,
    EventId, InteractionId, LaneId, MessageId, ModelRequestId, RecordId, RunId, SessionId,
    ToolBatchId, ToolCallId, TurnId,
};
use crate::raw_json::Metadata;

/// Stable machine-readable error code string.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ErrorCode(Arc<str>);

impl ErrorCode {
    /// Construct a stable error code.
    ///
    /// Codes are lowercase `snake_case` identifiers and must remain identical across
    /// bindings.
    #[must_use]
    pub fn new(code: impl AsRef<str>) -> Self {
        Self(Arc::<str>::from(code.as_ref()))
    }

    /// Borrow the code text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ErrorCode").field(&self.0).finish()
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for ErrorCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Stable error category (TDD §30.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Invalid configuration.
    Configuration,
    /// Registration / resolution failure.
    Registration,
    /// Input or schema validation failure.
    Validation,
    /// Model-provider failure.
    Model,
    /// Tool failure.
    Tool,
    /// Context-provider failure.
    Context,
    /// Middleware failure.
    Middleware,
    /// Store / journal failure.
    Store,
    /// Cancellation outcome.
    Cancellation,
    /// Deadline outcome.
    Deadline,
    /// Limit enforcement.
    Limit,
    /// Recovery / restart failure.
    Recovery,
    /// Corruption / conflict.
    Corruption,
    /// Protocol decode/encode failure.
    Protocol,
    /// Plugin failure.
    Plugin,
    /// Internal invariant failure converted at a boundary.
    Internal,
}

impl ErrorCategory {
    /// Canonical lowercase category name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Registration => "registration",
            Self::Validation => "validation",
            Self::Model => "model",
            Self::Tool => "tool",
            Self::Context => "context",
            Self::Middleware => "middleware",
            Self::Store => "store",
            Self::Cancellation => "cancellation",
            Self::Deadline => "deadline",
            Self::Limit => "limit",
            Self::Recovery => "recovery",
            Self::Corruption => "corruption",
            Self::Protocol => "protocol",
            Self::Plugin => "plugin",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Optional typed identifier context attached to an error descriptor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorIdentifiers {
    /// Session id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Lane id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane_id: Option<LaneId>,
    /// Run id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Turn id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// Message id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<MessageId>,
    /// Model request id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_request_id: Option<ModelRequestId>,
    /// Tool batch id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_batch_id: Option<ToolBatchId>,
    /// Tool call id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<ToolCallId>,
    /// Effect id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<EffectId>,
    /// Interaction id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<InteractionId>,
    /// Event id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<EventId>,
    /// Budget scope id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_scope_id: Option<BudgetScopeId>,
    /// Budget reservation id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_reservation_id: Option<BudgetReservationId>,
    /// Cancellation request id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_request_id: Option<CancellationRequestId>,
    /// Record id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_id: Option<RecordId>,
    /// Append batch id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub append_batch_id: Option<AppendBatchId>,
    /// Artifact id when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<ArtifactId>,
}

/// Source-free serializable error descriptor.
///
/// This is the only durable / cross-language error form. Local Rust source chains
/// belong on runtime [`crate`]-external wrappers and must be stripped before any
/// kernel, record, digest, binding, or remote boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDescriptor {
    /// Stable code.
    pub code: ErrorCode,
    /// Safe human-readable message.
    pub message: Arc<str>,
    /// Stable category.
    pub category: ErrorCategory,
    /// Whether a generic retry may be appropriate.
    pub retryable: bool,
    /// Optional typed identifier context.
    #[serde(default)]
    pub identifiers: ErrorIdentifiers,
    /// Non-secret structured details.
    #[serde(default)]
    pub safe_details: Metadata,
}

impl ErrorDescriptor {
    /// Construct a descriptor with empty identifiers and metadata.
    #[must_use]
    pub fn new(
        code: impl AsRef<str>,
        message: impl AsRef<str>,
        category: ErrorCategory,
        retryable: bool,
    ) -> Self {
        Self {
            code: ErrorCode::new(code),
            message: Arc::<str>::from(message.as_ref()),
            category,
            retryable,
            identifiers: ErrorIdentifiers::default(),
            safe_details: Metadata::empty(),
        }
    }
}

impl fmt::Display for ErrorDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}/{}): {}",
            self.code,
            self.category,
            if self.retryable {
                "retryable"
            } else {
                "terminal"
            },
            self.message
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunId;

    #[test]
    fn error_descriptor_json_round_trip_preserves_stable_fields() {
        let mut descriptor = ErrorDescriptor::new(
            "raw_json_too_large",
            "JSON source span exceeded the v1 ceiling",
            ErrorCategory::Validation,
            false,
        );
        descriptor.identifiers.run_id =
            Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"));
        descriptor.safe_details =
            Metadata::parse(r#"{"limit_bytes":1048576,"observed_bytes":1048577}"#).expect("meta");

        let json = serde_json::to_string(&descriptor).expect("ser");
        let round: ErrorDescriptor = serde_json::from_str(&json).expect("de");
        assert_eq!(round, descriptor);
        assert_eq!(round.code.as_str(), "raw_json_too_large");
        assert!(!round.retryable);
        assert_eq!(round.category, ErrorCategory::Validation);
        assert!(round.identifiers.session_id.is_none());
    }
}
