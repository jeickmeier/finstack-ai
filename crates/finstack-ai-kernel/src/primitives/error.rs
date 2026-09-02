//! Stable, source-free error descriptors for durable and cross-language boundaries.

use core::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::primitives::Metadata;
use crate::primitives::{
    AppendBatchId, ArtifactId, BudgetReservationId, BudgetScopeId, CancellationRequestId, EffectId,
    EventId, InteractionId, LaneId, MessageId, ModelRequestId, RecordId, RunId, SessionId,
    ToolBatchId, ToolCallId, TurnId,
};

/// Stable machine-readable error code string.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ErrorCode(Arc<str>);

/// Compile-time validated input for [`ErrorCode::from_static`].
///
/// Construct this through [`ErrorCode::static_literal`] or the
/// [`static_error_code!`](crate::static_error_code) macro.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticErrorCode(&'static str);

impl ErrorCode {
    /// Construct a stable error code.
    ///
    /// Codes are lowercase `snake_case` identifiers and must remain identical across
    /// bindings.
    ///
    /// # Arguments
    ///
    /// * `code` - Lowercase `snake_case` identifier. It must stay identical
    ///   across language bindings.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorCodeError`] when the code is empty, exceeds the label
    /// ceiling, or is not lowercase `snake_case`.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::ErrorCode;
    ///
    /// let code = ErrorCode::new("invalid_input").expect("code");
    /// assert_eq!(code.as_str(), "invalid_input");
    /// assert!(ErrorCode::new("InvalidInput").is_err());
    /// ```
    pub fn new(code: impl AsRef<str>) -> Result<Self, ErrorCodeError> {
        let code = code.as_ref();
        if !error_code_is_valid(code) {
            return Err(ErrorCodeError {
                code: code.to_owned(),
            });
        }
        Ok(Self(Arc::<str>::from(code)))
    }

    /// Validate a compile-time `snake_case` literal without allocating.
    #[doc(hidden)]
    #[must_use]
    pub const fn static_literal(code: &'static str) -> Option<StaticErrorCode> {
        if error_code_is_valid(code) {
            Some(StaticErrorCode(code))
        } else {
            None
        }
    }

    /// Construct a code from a validated compile-time literal.
    ///
    /// Use this for frozen engine codes that are part of the crate source.
    /// [`ErrorCode::new`] remains the fallible constructor for untrusted text.
    ///
    /// # Arguments
    ///
    /// * `code` - A validated lowercase `snake_case` literal.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{ErrorCode, static_error_code};
    ///
    /// let code = static_error_code!("invalid_input");
    /// assert_eq!(code.as_str(), "invalid_input");
    /// ```
    #[must_use]
    pub fn from_static(code: StaticErrorCode) -> Self {
        Self(Arc::from(code.0))
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

impl PartialEq<str> for ErrorCode {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ErrorCode {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<ErrorCode> for str {
    fn eq(&self, other: &ErrorCode) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<ErrorCode> for &str {
    fn eq(&self, other: &ErrorCode) -> bool {
        *self == other.as_str()
    }
}

impl AsRef<str> for ErrorCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ErrorCodeVisitor;

        impl serde::de::Visitor<'_> for ErrorCodeVisitor {
            type Value = ErrorCode;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    formatter,
                    "a lowercase snake_case error code no longer than {LABEL_MAX_BYTES} bytes"
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                ErrorCode::new(value).map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                ErrorCode::new(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(ErrorCodeVisitor)
    }
}

/// Invalid [`ErrorCode`] text.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid error code (expected lowercase snake_case): {code}")]
pub struct ErrorCodeError {
    /// Rejected code text.
    pub code: String,
}

const fn error_code_is_valid(code: &str) -> bool {
    let bytes = code.as_bytes();
    if bytes.is_empty() || bytes.len() > LABEL_MAX_BYTES || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut index = 1;
    let mut previous_underscore = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_underscore = false;
        } else if byte == b'_' && !previous_underscore {
            previous_underscore = true;
        } else {
            return false;
        }
        index += 1;
    }
    !previous_underscore
}

/// Construct an [`ErrorCode`] from a compile-time-validated literal.
///
/// Invalid literals fail during compilation:
///
/// ```compile_fail
/// use finstack_ai_kernel::static_error_code;
///
/// let _ = static_error_code!("InvalidCode");
/// ```
#[macro_export]
macro_rules! static_error_code {
    ($code:expr) => {{
        let validated = const {
            match $crate::ErrorCode::static_literal($code) {
                Some(validated) => validated,
                None => panic!("invalid static error code"),
            }
        };
        $crate::ErrorCode::from_static(validated)
    }};
}

/// Stable error category (contract section 30.2).
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
#[serde(deny_unknown_fields)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    pub identifiers: ErrorIdentifiers,
    /// Non-secret structured details.
    pub safe_details: Metadata,
}

impl ErrorDescriptor {
    /// Construct a descriptor with empty identifiers and metadata.
    ///
    /// # Arguments
    ///
    /// * `code` - Stable lowercase `snake_case` error code.
    /// * `message` - Safe human-readable message (non-empty, no NUL).
    /// * `category` - Stable error category.
    /// * `retryable` - Whether a generic retry may be appropriate.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorDescriptorError`] when `code` is invalid or `message`
    /// violates the semantic text ceiling.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{ErrorCategory, ErrorDescriptor};
    ///
    /// let error = ErrorDescriptor::new(
    ///     "invalid_input",
    ///     "payload rejected",
    ///     ErrorCategory::Validation,
    ///     false,
    /// )
    /// .expect("descriptor");
    /// assert_eq!(error.code.as_str(), "invalid_input");
    /// ```
    pub fn new(
        code: impl AsRef<str>,
        message: impl AsRef<str>,
        category: ErrorCategory,
        retryable: bool,
    ) -> Result<Self, ErrorDescriptorError> {
        let descriptor = Self {
            code: ErrorCode::new(code).map_err(ErrorDescriptorError::Code)?,
            message: Arc::<str>::from(message.as_ref()),
            category,
            retryable,
            identifiers: ErrorIdentifiers::default(),
            safe_details: Metadata::empty(),
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Validate programmatically assembled descriptor text.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorDescriptorError::InvalidMessage`] when the message is
    /// empty, contains NUL, or exceeds the semantic text ceiling.
    pub fn validate(&self) -> Result<(), ErrorDescriptorError> {
        if self.message.is_empty()
            || self.message.len() > TEXT_MAX_BYTES
            || self.message.as_bytes().contains(&0)
        {
            return Err(ErrorDescriptorError::InvalidMessage);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ErrorDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            code: ErrorCode,
            message: BoundedString<TEXT_MAX_BYTES>,
            category: ErrorCategory,
            retryable: bool,
            #[serde(default)]
            identifiers: ErrorIdentifiers,
            #[serde(default)]
            safe_details: Metadata,
        }

        let wire = Wire::deserialize(deserializer)?;
        let descriptor = Self {
            code: wire.code,
            message: Arc::from(wire.message.into_inner()),
            category: wire.category,
            retryable: wire.retryable,
            identifiers: wire.identifiers,
            safe_details: wire.safe_details,
        };
        descriptor.validate().map_err(serde::de::Error::custom)?;
        Ok(descriptor)
    }
}

/// Invalid durable error descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ErrorDescriptorError {
    /// Stable error code is invalid.
    #[error(transparent)]
    Code(#[from] ErrorCodeError),
    /// Safe message violates the semantic text contract.
    #[error("invalid error message")]
    InvalidMessage,
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
    use crate::primitives::RunId;

    #[test]
    fn error_descriptor_json_round_trip_preserves_stable_fields() {
        let mut descriptor = ErrorDescriptor::new(
            "raw_json_too_large",
            "JSON source span exceeded the v1 ceiling",
            ErrorCategory::Validation,
            false,
        )
        .expect("descriptor");
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

    #[test]
    fn error_code_requires_lowercase_snake_case() {
        assert!(ErrorCode::new("raw_json_too_large").is_ok());
        assert!(ErrorCode::new("a").is_ok());
        assert!(ErrorCode::new("RawJson").is_err());
        assert!(ErrorCode::new("raw-json").is_err());
        assert!(ErrorCode::new("_leading").is_err());
        assert!(ErrorCode::new("trailing_").is_err());
        assert!(ErrorCode::new("double__underscore").is_err());
        assert!(ErrorCode::new("").is_err());
        assert!(serde_json::from_str::<ErrorCode>("\"Not_Snake\"").is_err());
    }

    #[test]
    fn static_error_codes_share_the_runtime_validation_contract() {
        for code in [
            "internal",
            "raw_json_too_large",
            "InvalidCode",
            "double__underscore",
            "trailing_",
            "",
        ] {
            assert_eq!(
                ErrorCode::static_literal(code).is_some(),
                ErrorCode::new(code).is_ok(),
                "static/runtime validation drifted for {code:?}",
            );
        }
        assert_eq!(crate::static_error_code!("internal").as_str(), "internal");
    }

    #[test]
    fn error_code_enforces_label_ceiling_before_decode() {
        let exact = "x".repeat(crate::LABEL_MAX_BYTES);
        assert!(ErrorCode::new(&exact).is_ok());
        let one_over = "x".repeat(crate::LABEL_MAX_BYTES + 1);
        assert!(ErrorCode::new(&one_over).is_err());
        let encoded = serde_json::to_string(&one_over).expect("encoded error code");
        assert!(serde_json::from_str::<ErrorCode>(&encoded).is_err());
        let escaped = format!("\"{}\"", "\\u0078".repeat(crate::LABEL_MAX_BYTES + 1));
        assert!(serde_json::from_str::<ErrorCode>(&escaped).is_err());
    }

    #[test]
    fn error_descriptor_rejects_unknown_nested_fields() {
        let descriptor = serde_json::json!({
            "code": "provider_failed",
            "message": "failed",
            "category": "model",
            "retryable": false,
            "identifiers": {
                "run_id": "01234567-89ab-7cde-89ab-0123456789ab",
                "unknown_identifier": true
            },
            "safe_details": {}
        });
        assert!(serde_json::from_value::<ErrorDescriptor>(descriptor).is_err());

        let descriptor = serde_json::json!({
            "code": "provider_failed",
            "message": "failed",
            "category": "model",
            "retryable": false,
            "identifiers": {},
            "safe_details": {},
            "unknown_descriptor": true
        });
        assert!(serde_json::from_value::<ErrorDescriptor>(descriptor).is_err());
    }

    #[test]
    fn error_descriptor_message_enforces_text_ceiling() {
        let exact = "x".repeat(crate::content::TEXT_MAX_BYTES);
        assert!(
            ErrorDescriptor::new("provider_failed", exact, ErrorCategory::Model, false).is_ok()
        );
        let one_over = "x".repeat(crate::content::TEXT_MAX_BYTES + 1);
        assert!(
            ErrorDescriptor::new("provider_failed", one_over, ErrorCategory::Model, false).is_err()
        );
    }
}
