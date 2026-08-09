//! PR-009 reducer cursors and durable record payloads.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bounds::{BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::digest::Digest;
use crate::error::ErrorDescriptor;
use crate::ids::{EffectId, MessageId, ModelRequestId, TurnId};
use crate::message::Message;

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
    /// The current terminal candidate was accepted.
    FinalizeAccepted,
    /// Begin a fresh model cycle.
    ContinueModel {
        /// Checked next cycle number.
        next_cycle: u64,
    },
    /// The aggregate stage failed.
    Failed {
        /// Safe, source-free failure descriptor.
        error: ErrorDescriptor,
    },
}

/// Durable aggregate stage-settlement record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageOutcomeRecorded {
    /// Settled cursor.
    pub cursor: StageCursor,
    /// Replay-complete disposition.
    pub disposition: StageDisposition,
    /// `stage-settlement` schema-1 digest of the normalized input.
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
    /// `model-context` schema-1 digest of `messages`.
    pub context_digest: Digest,
}

impl ContextPrepared {
    pub(crate) fn try_from_messages(
        cycle: u64,
        turn_id: TurnId,
        messages: Vec<Message>,
    ) -> Result<Self, ContextPreparedError> {
        if messages.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(ContextPreparedError::TooManyMessages);
        }
        let context_digest = context_digest_for(&messages)?;
        Ok(Self {
            cycle,
            turn_id,
            messages: messages.into(),
            context_digest,
        })
    }

    /// Construct a bounded prepared context and validate its canonical digest.
    ///
    /// # Errors
    ///
    /// Returns [`ContextPreparedError`] when the message ceiling is exceeded,
    /// canonicalization fails, or `context_digest` does not match the messages.
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

fn context_digest_for(messages: &[Message]) -> Result<Digest, ContextPreparedError> {
    let canonical = serde_json_canonicalizer::to_vec(&messages)
        .map_err(|_| ContextPreparedError::CanonicalizationFailed)?;
    Digest::domain_separated("model-context", 1, &canonical)
        .map_err(|_| ContextPreparedError::CanonicalizationFailed)
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
