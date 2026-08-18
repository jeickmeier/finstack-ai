//! Reducer stage cursors and durable lifecycle record payloads.
//!
//! Defines the seven normalized middleware [`Stage`] values, [`StageCursor`],
//! [`StageDisposition`], and the durable records that move a run through
//! context preparation, retry, suspension, cancellation, completion, and
//! failure ([`ContextPrepared`], [`EntryAppended`], [`RetryScheduled`],
//! [`RunSuspended`], [`RunCancelled`], [`RunCompleted`], [`RunFailed`]).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::conversation::Message;
use crate::primitives::Digest;
use crate::primitives::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::primitives::{
    CancellationRequestId, EffectId, MessageId, ModelRequestId, ToolBatchId, TurnId,
};
use crate::primitives::{Duration, Timestamp};
use crate::primitives::{ErrorCode, ErrorDescriptor};

/// One of the seven normalized middleware boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Before a run begins.
    BeforeRun,
    /// Aggregate context preparation.
    PrepareContext,
    /// Before a model request.
    BeforeModel,
    /// After a model response.
    AfterModel,
    /// Before a tool batch.
    BeforeToolBatch,
    /// After a tool batch.
    AfterToolBatch,
    /// Final behavior-changing boundary.
    BeforeFinalize,
}

/// Exact cycle and stage expected by an aggregate stage settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageCursor {
    /// Zero-based model cycle.
    pub cycle: u64,
    /// Normalized middleware stage.
    pub stage: Stage,
}

/// Replay-complete disposition of an aggregate stage settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "the frozen public payload stores ErrorDescriptor by value"
)]
pub enum StageDisposition {
    /// Continue to the next stage.
    Continued,
    /// Context was prepared and a fresh turn was allocated.
    ContextPrepared {
        /// Fresh turn identity.
        turn_id: TurnId,
        /// Digest of the prepared message array.
        context_digest: Digest,
    },
    /// A model effect was requested.
    ModelRequested {
        /// Current turn identity.
        turn_id: TurnId,
        /// Fresh model request identity.
        model_request_id: ModelRequestId,
        /// Fresh effect identity.
        effect_id: EffectId,
    },
    /// A complete tool batch was prepared.
    ToolBatchPrepared {
        /// Stable batch identity.
        tool_batch_id: ToolBatchId,
        /// Domain-separated `tool-batch-plan` digest.
        plan_digest: Digest,
    },
    /// The current terminal candidate was accepted.
    FinalizeAccepted,
    /// Begin a fresh model cycle.
    ContinueModel {
        /// Checked next cycle number.
        next_cycle: u64,
    },
    /// A bounded semantic retry was scheduled.
    RetryScheduled {
        /// One-based additional attempt number.
        attempt: u32,
        /// Timer effect that gates the next cycle.
        timer_effect_id: EffectId,
        /// Semantic due time.
        due_at: Timestamp,
    },
    /// The aggregate stage failed.
    Failed {
        /// Safe, source-free failure descriptor.
        error: ErrorDescriptor,
    },
}

/// Stable semantic retry family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClassification {
    /// Model failure retry.
    Model,
    /// Tool failure retry.
    Tool,
    /// Structured-output validation retry.
    Validation,
    /// Framework failure retry.
    Framework,
}

/// Normalized retry decision accepted at `before_finalize`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetryDirective {
    /// Failure family.
    pub classification: RetryClassification,
    /// Deterministic backoff before the next cycle.
    pub backoff: Duration,
    /// Versioned policy identity.
    pub policy_version: Arc<str>,
}

impl RetryDirective {
    /// Construct a retry directive with a bounded non-empty policy version.
    ///
    /// # Arguments
    ///
    /// * `classification` - Failure family that selected this retry.
    /// * `backoff` - Semantic delay before the next attempt.
    /// * `policy_version` - Non-empty policy-version label.
    ///
    /// # Errors
    ///
    /// Returns [`EntryError::InvalidLabel`] for an invalid policy version.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{Duration, RetryClassification, RetryDirective};
    ///
    /// let directive = RetryDirective::try_new(
    ///     RetryClassification::Model,
    ///     Duration::from_millis(250),
    ///     "policy-v1",
    /// )
    /// .expect("directive");
    /// assert_eq!(directive.policy_version.as_ref(), "policy-v1");
    /// ```
    pub fn try_new(
        classification: RetryClassification,
        backoff: Duration,
        policy_version: impl AsRef<str>,
    ) -> Result<Self, EntryError> {
        let policy_version = policy_version.as_ref();
        if policy_version.is_empty()
            || policy_version.len() > LABEL_MAX_BYTES
            || policy_version.as_bytes().contains(&0)
        {
            return Err(EntryError::InvalidLabel);
        }
        Ok(Self {
            classification,
            backoff,
            policy_version: Arc::from(policy_version),
        })
    }
}

impl<'de> Deserialize<'de> for RetryDirective {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            classification: RetryClassification,
            backoff: Duration,
            policy_version: BoundedString<LABEL_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.classification,
            wire.backoff,
            wire.policy_version.into_inner(),
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Durable semantic retry and timer intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetryScheduled {
    /// Failed model cycle.
    pub cycle: u64,
    /// One-based additional attempt number.
    pub attempt: u32,
    /// Failure family.
    pub classification: RetryClassification,
    /// Versioned policy identity.
    pub policy_version: Arc<str>,
    /// Timer effect identity.
    pub timer_effect_id: EffectId,
    /// Semantic due time.
    pub due_at: Timestamp,
    /// Safe failure being retried.
    pub prior_error: ErrorDescriptor,
}

impl RetryScheduled {
    /// Construct validated durable retry intent.
    ///
    /// # Arguments
    ///
    /// * `cycle` - Failed model cycle.
    /// * `attempt` - One-based additional attempt number. Zero is rejected.
    /// * `classification` - Failure family being retried.
    /// * `policy_version` - Non-empty policy-version label.
    /// * `timer_effect_id` - Timer effect that will fire at `due_at`.
    /// * `due_at` - Semantic due time for the retry timer.
    /// * `prior_error` - Safe descriptor of the failure being retried.
    ///
    /// # Errors
    ///
    /// Returns [`EntryError`] when the attempt, policy version, or error is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     EffectId, ErrorCategory, ErrorDescriptor, RetryClassification, RetryScheduled,
    ///     Timestamp,
    /// };
    ///
    /// # let timer = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let error = ErrorDescriptor::new(
    /// #     "provider_failed", "provider failed", ErrorCategory::Model, true,
    /// # ).expect("error");
    /// let scheduled = RetryScheduled::try_new(
    ///     0,
    ///     1,
    ///     RetryClassification::Model,
    ///     "policy-v1",
    ///     timer,
    ///     Timestamp::from_unix_ms(1_000).expect("due"),
    ///     error,
    /// )
    /// .expect("scheduled");
    /// assert_eq!(scheduled.attempt, 1);
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        cycle: u64,
        attempt: u32,
        classification: RetryClassification,
        policy_version: impl AsRef<str>,
        timer_effect_id: EffectId,
        due_at: Timestamp,
        prior_error: ErrorDescriptor,
    ) -> Result<Self, EntryError> {
        if attempt == 0 {
            return Err(EntryError::InvalidAttempt);
        }
        let directive = RetryDirective::try_new(classification, Duration::ZERO, policy_version)?;
        prior_error
            .validate()
            .map_err(|_| EntryError::InvalidError)?;
        Ok(Self {
            cycle,
            attempt,
            classification,
            policy_version: directive.policy_version,
            timer_effect_id,
            due_at,
            prior_error,
        })
    }
}

impl<'de> Deserialize<'de> for RetryScheduled {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            cycle: u64,
            attempt: u32,
            classification: RetryClassification,
            policy_version: BoundedString<LABEL_MAX_BYTES>,
            timer_effect_id: EffectId,
            due_at: Timestamp,
            prior_error: ErrorDescriptor,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.cycle,
            wire.attempt,
            wire.classification,
            wire.policy_version.into_inner(),
            wire.timer_effect_id,
            wire.due_at,
            wire.prior_error,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Durable semantic timer firing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimerFired {
    /// Matching timer effect.
    pub effect_id: EffectId,
    /// Frozen due time.
    pub due_at: Timestamp,
    /// Runtime-observed semantic firing time.
    pub fired_at: Timestamp,
}

/// Durable non-terminal suspension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSuspended {
    /// Stable safe reason code.
    pub reason_code: ErrorCode,
    /// Winning cancellation request when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_request_id: Option<CancellationRequestId>,
}

/// Durable cancelled terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCancelled {
    /// Winning cancellation request.
    pub request_id: CancellationRequestId,
    /// Stable safe reason code.
    pub reason_code: ErrorCode,
}

/// Entry and control payload construction failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EntryError {
    /// A bounded label was empty, oversized, or contained NUL.
    #[error("invalid label")]
    InvalidLabel,
    /// Retry attempt zero is reserved for the original execution.
    #[error("invalid retry attempt")]
    InvalidAttempt,
    /// Embedded durable error is invalid.
    #[error("invalid durable error")]
    InvalidError,
}

/// Durable aggregate stage-settlement record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageOutcomeRecorded {
    /// Settled cursor.
    pub cursor: StageCursor,
    /// Replay-complete disposition.
    pub disposition: StageDisposition,
    /// Domain-separated `stage-settlement` digest of the normalized input.
    pub settlement_digest: Digest,
}

/// Durable prepared model context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextPrepared {
    /// Model cycle.
    pub cycle: u64,
    /// Fresh turn identity.
    pub turn_id: TurnId,
    /// Prepared provider-neutral messages.
    pub messages: Arc<[Message]>,
    /// Domain-separated `model-context` digest of `messages`.
    pub context_digest: Digest,
}

impl ContextPrepared {
    pub(crate) fn try_from_messages(
        cycle: u64,
        turn_id: TurnId,
        messages: Vec<Message>,
    ) -> Result<Self, ContextPreparedError> {
        let (context_digest, _) = context_digest_and_len(&messages)?;
        Ok(Self {
            cycle,
            turn_id,
            messages: messages.into(),
            context_digest,
        })
    }

    /// Build from an already-shared message slice and a precomputed digest.
    ///
    /// The caller has canonicalized these messages once already; recomputing
    /// the digest here would double the per-turn context cost.
    pub(crate) fn from_shared(
        cycle: u64,
        turn_id: TurnId,
        messages: Arc<[Message]>,
        context_digest: Digest,
    ) -> Self {
        Self {
            cycle,
            turn_id,
            messages,
            context_digest,
        }
    }

    /// Construct a bounded prepared context and validate its canonical digest.
    ///
    /// # Arguments
    ///
    /// * `cycle` - Model cycle that owns this prepared context.
    /// * `turn_id` - Fresh turn identity allocated for the context.
    /// * `messages` - Prepared provider-neutral messages. Length must stay within
    ///   the semantic array ceiling.
    /// * `context_digest` - Caller-supplied `model-context` digest. It must match
    ///   the canonical digest of `messages`.
    ///
    /// # Errors
    ///
    /// Returns [`ContextPreparedError`] when the message ceiling is exceeded,
    /// canonicalization fails, or `context_digest` does not match the messages.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{ContextPrepared, Digest, TurnId};
    ///
    /// let digest = Digest::domain_separated("model-context", 1, b"[]").expect("digest");
    /// let prepared = ContextPrepared::try_new(
    ///     0,
    ///     TurnId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
    ///     vec![],
    ///     digest,
    /// )
    /// .expect("context");
    /// assert_eq!(prepared.cycle, 0);
    /// ```
    pub fn try_new(
        cycle: u64,
        turn_id: TurnId,
        messages: Vec<Message>,
        context_digest: Digest,
    ) -> Result<Self, ContextPreparedError> {
        let context = Self::try_from_messages(cycle, turn_id, messages)?;
        if context_digest != context.context_digest {
            return Err(ContextPreparedError::DigestMismatch);
        }
        Ok(context)
    }
}

impl<'de> Deserialize<'de> for ContextPrepared {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            cycle: u64,
            turn_id: TurnId,
            messages: BoundedVec<Message, SEMANTIC_ARRAY_MAX_ITEMS>,
            context_digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.cycle,
            wire.turn_id,
            wire.messages.into_inner(),
            wire.context_digest,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Prepared-context construction failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ContextPreparedError {
    /// The context contains too many messages.
    #[error("context messages exceed semantic array maximum")]
    TooManyMessages,
    /// Canonical message serialization failed.
    #[error("context messages could not be canonicalized")]
    CanonicalizationFailed,
    /// The supplied digest does not match the messages.
    #[error("context digest mismatch")]
    DigestMismatch,
}

/// Canonical `model-context` digest and canonical byte length of `messages`.
///
/// The length is returned alongside the digest because limit accounting needs
/// exactly the byte count this canonicalization already produced. Computing
/// them separately canonicalized the whole conversation context twice, and the
/// context grows with every turn.
pub(crate) fn context_digest_and_len(
    messages: &[Message],
) -> Result<(Digest, usize), ContextPreparedError> {
    if messages.len() > SEMANTIC_ARRAY_MAX_ITEMS {
        return Err(ContextPreparedError::TooManyMessages);
    }
    let mut writer = crate::primitives::DigestWriter::new("model-context", 1)
        .map_err(|_| ContextPreparedError::CanonicalizationFailed)?;
    serde_json_canonicalizer::to_writer(&messages, &mut writer)
        .map_err(|_| ContextPreparedError::CanonicalizationFailed)?;
    Ok(writer.finish())
}

/// Durable final assistant message append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryAppended {
    /// Model cycle.
    pub cycle: u64,
    /// Turn identity.
    pub turn_id: TurnId,
    /// Model request identity.
    pub model_request_id: ModelRequestId,
    /// Original model effect identity.
    pub effect_id: EffectId,
    /// Prior durable assistant message, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_message_id: Option<MessageId>,
    /// Final assistant message.
    pub message: Message,
}

/// Durable successful run terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCompleted {
    /// Final model cycle.
    pub cycle: u64,
    /// Final turn identity.
    pub turn_id: TurnId,
    /// Final model request identity.
    pub model_request_id: ModelRequestId,
    /// Final model effect identity.
    pub effect_id: EffectId,
    /// Durable result message identity.
    pub result_message_id: MessageId,
    /// Digest of the normalized model output.
    pub result_digest: Digest,
}

/// Durable failed run terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFailed {
    /// Final model cycle.
    pub cycle: u64,
    /// Turn identity when a turn existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// Model request identity when a request existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_request_id: Option<ModelRequestId>,
    /// Model effect identity when an effect existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<EffectId>,
    /// Accepted safe failure descriptor.
    pub error: ErrorDescriptor,
}
