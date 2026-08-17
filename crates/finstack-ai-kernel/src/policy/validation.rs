//! Validator-independent structured-output outcomes and retry feedback.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::policy::{SchemaRef, StructuredResultSource};
use crate::primitives::Digest;
use crate::primitives::RawJson;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{EffectId, MessageId, ModelRequestId, ToolCallId, TurnId};
use crate::primitives::{ErrorCategory, ErrorDescriptor, ErrorDescriptorError};

pub(crate) fn expected_validation_error(
    retry_attempts: u32,
    max_retries: Option<u32>,
) -> Result<ErrorDescriptor, ErrorDescriptorError> {
    let retryable = max_retries.is_none_or(|maximum| retry_attempts < maximum);
    ErrorDescriptor::new(
        if retryable {
            "structured_output_validation_failed"
        } else {
            "structured_output_retries_exhausted"
        },
        if retryable {
            "structured output did not satisfy the configured schema"
        } else {
            "structured output retry limit was exhausted"
        },
        ErrorCategory::Validation,
        retryable,
    )
}

/// One normalized, validator-independent JSON Schema issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationIssue {
    /// JSON Pointer into the candidate instance.
    pub instance_path: Arc<str>,
    /// JSON Pointer into the canonical schema.
    pub schema_path: Arc<str>,
    /// Stable JSON Schema keyword when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<Arc<str>>,
    /// Safe model-visible issue summary.
    pub message: Arc<str>,
}

impl ValidationIssue {
    /// Construct one bounded validator-independent issue.
    ///
    /// # Arguments
    ///
    /// * `instance_path` - JSON pointer into the instance document.
    /// * `schema_path` - JSON pointer into the schema document.
    /// * `keyword` - Optional schema keyword label; `None` omits it.
    /// * `message` - Non-empty validator-independent issue text.
    ///
    /// # Errors
    ///
    /// Returns a stable validation reason when a pointer, keyword, or message
    /// violates the semantic text bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::ValidationIssue;
    ///
    /// let issue = ValidationIssue::try_new("/", "/properties/name", Some("required"), "missing name")
    ///     .expect("issue");
    /// assert_eq!(issue.keyword.as_deref(), Some("required"));
    /// ```
    pub fn try_new(
        instance_path: impl Into<Arc<str>>,
        schema_path: impl Into<Arc<str>>,
        keyword: Option<impl Into<Arc<str>>>,
        message: impl Into<Arc<str>>,
    ) -> Result<Self, &'static str> {
        let value = Self {
            instance_path: instance_path.into(),
            schema_path: schema_path.into(),
            keyword: keyword.map(Into::into),
            message: message.into(),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self.instance_path.len() > TEXT_MAX_BYTES
            || self.schema_path.len() > TEXT_MAX_BYTES
            || self.message.is_empty()
            || self.message.len() > TEXT_MAX_BYTES
            || self.instance_path.as_bytes().contains(&0)
            || self.schema_path.as_bytes().contains(&0)
            || self.message.as_bytes().contains(&0)
        {
            return Err("invalid_validation_issue_text");
        }
        if self
            .keyword
            .as_ref()
            .is_some_and(|keyword| !crate::label_is_valid(keyword))
        {
            return Err("invalid_validation_keyword");
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ValidationIssue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            instance_path: BoundedString<TEXT_MAX_BYTES>,
            schema_path: BoundedString<TEXT_MAX_BYTES>,
            #[serde(default)]
            keyword: Option<BoundedString<LABEL_MAX_BYTES>>,
            message: BoundedString<TEXT_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            instance_path: Arc::from(wire.instance_path.into_inner()),
            schema_path: Arc::from(wire.schema_path.into_inner()),
            keyword: wire.keyword.map(|value| Arc::from(value.into_inner())),
            message: Arc::from(wire.message.into_inner()),
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

/// Normalized result of executing an external schema validator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationOutcome {
    /// Candidate satisfied the exact schema reference.
    Valid,
    /// Candidate failed with bounded normalized issues and retry feedback.
    Invalid {
        /// Deterministically ordered validation issues.
        issues: Arc<[ValidationIssue]>,
        /// Safe feedback to expose to the next model attempt.
        feedback: Arc<str>,
    },
}

impl ValidationOutcome {
    /// Construct a bounded invalid outcome.
    ///
    /// # Errors
    ///
    /// Returns a stable validation reason when issue cardinality or feedback
    /// violates the semantic bounds.
    pub fn try_invalid(
        issues: impl Into<Arc<[ValidationIssue]>>,
        feedback: impl Into<Arc<str>>,
    ) -> Result<Self, &'static str> {
        let value = Self::Invalid {
            issues: issues.into(),
            feedback: feedback.into(),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), &'static str> {
        if let Self::Invalid { issues, feedback } = self {
            if issues.is_empty() || issues.len() > SEMANTIC_ARRAY_MAX_ITEMS {
                return Err("invalid_validation_issue_count");
            }
            if feedback.len() > TEXT_MAX_BYTES || feedback.as_bytes().contains(&0) {
                return Err("invalid_validation_feedback");
            }
            for issue in issues.iter() {
                issue.validate()?;
            }
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ValidationOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct InvalidWire {
            issues: BoundedVec<ValidationIssue, SEMANTIC_ARRAY_MAX_ITEMS>,
            feedback: BoundedString<TEXT_MAX_BYTES>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire {
            Valid,
            Invalid(InvalidWire),
        }
        let value = match Wire::deserialize(deserializer)? {
            Wire::Valid => Self::Valid,
            Wire::Invalid(value) => Self::Invalid {
                issues: Arc::from(value.issues.into_inner()),
                feedback: Arc::from(value.feedback.into_inner()),
            },
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}

/// Post-model normalized validation command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputValidated {
    /// Durable assistant message being validated.
    pub message_id: MessageId,
    /// Exact configured schema reference.
    pub schema: SchemaRef,
    /// Exact canonical candidate passed to the validator.
    pub candidate: RawJson,
    /// Assistant content location from which `candidate` was extracted.
    pub source: StructuredResultSource,
    /// Normalized validator result.
    pub outcome: ValidationOutcome,
}

impl OutputValidated {
    /// Validate intrinsic command bounds before a record is allocated.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.schema.schema_version == 0 {
            return Err("schema_version_must_be_positive");
        }
        self.outcome.validate()
    }
}

/// Replay-complete invalid structured result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputValidationFailed {
    /// Model cycle that produced the candidate.
    pub cycle: u64,
    /// Originating turn.
    pub turn_id: TurnId,
    /// Originating model request.
    pub model_request_id: ModelRequestId,
    /// Originating model effect.
    pub effect_id: EffectId,
    /// Durable assistant message containing the candidate.
    pub message_id: MessageId,
    /// Exact schema used by the validator.
    pub schema: SchemaRef,
    /// Digest of the exact candidate.
    pub candidate_digest: Digest,
    /// Exact assistant content source.
    pub source: StructuredResultSource,
    /// Deterministically ordered normalized issues.
    pub issues: Arc<[ValidationIssue]>,
    /// Safe feedback for the next attempt.
    pub feedback: Arc<str>,
    /// Stable validation failure used by `before_finalize` retry/finalization.
    pub error: ErrorDescriptor,
    /// Application calls intentionally skipped for this invalid attempt.
    pub skipped_tool_call_ids: Arc<[ToolCallId]>,
}

impl OutputValidationFailed {
    /// Validate fields that do not require the surrounding journal state.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.schema.schema_version == 0 {
            return Err("schema_version_must_be_positive");
        }
        ValidationOutcome::Invalid {
            issues: self.issues.clone(),
            feedback: self.feedback.clone(),
        }
        .validate()?;
        if self.error.category != ErrorCategory::Validation || self.error.validate().is_err() {
            return Err("invalid_validation_error");
        }
        if self.skipped_tool_call_ids.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err("too_many_skipped_tool_calls");
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for OutputValidationFailed {
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
            candidate_digest: Digest,
            source: StructuredResultSource,
            issues: BoundedVec<ValidationIssue, SEMANTIC_ARRAY_MAX_ITEMS>,
            feedback: BoundedString<TEXT_MAX_BYTES>,
            error: ErrorDescriptor,
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
            candidate_digest: wire.candidate_digest,
            source: wire.source,
            issues: Arc::from(wire.issues.into_inner()),
            feedback: Arc::from(wire.feedback.into_inner()),
            error: wire.error,
            skipped_tool_call_ids: Arc::from(wire.skipped_tool_call_ids.into_inner()),
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
