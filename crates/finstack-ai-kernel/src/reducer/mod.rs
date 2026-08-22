//! Pure decision and transactional committed-record application.

mod allocated_ids;
mod apply;
mod capacity;
mod decide;
mod decision;
pub(crate) mod extension;
mod fingerprint;
mod input;
mod interaction;
mod tool;
mod validation;

use std::sync::Arc;

use serde::Serialize;

pub use decision::{CommittedBatch, Decision, KernelError, PostCommitAction};
pub use input::{
    AcceptRun, CancelRequested, CancellationReconciledInput, ExtensionEffectSettled,
    ExtensionSettlement, ExternalEffectCompletedInput, ExternalEffectCompletion,
    ExternalEffectOutcome, InteractionSettled, KernelInput, ModelSettled, ModelSettlement,
    ReducerStageOutcome, RequestCompactionModel, RequestExtensionEffect, RequestInteraction,
    StageSettled, TimerFiredInput, ToolBatchSettled, ToolSettlement,
};

use crate::events::RunEvent;
use crate::records::lifecycle::RunFailed;
use crate::state::{KernelState, TransitionEnv};
use crate::{Digest, ErrorDescriptor};

pub(super) fn canonical_digest<T: Serialize>(
    domain: &'static str,
    value: &T,
) -> Result<Digest, KernelError> {
    Ok(canonical_digest_and_len(domain, value)?.0)
}

/// Canonical digest plus the canonical byte length, in a single pass.
///
/// Streams into the hasher instead of materializing the canonical bytes, so
/// no output buffer is allocated or grown per digest.
fn canonical_digest_and_len<T: Serialize>(
    domain: &'static str,
    value: &T,
) -> Result<(Digest, usize), KernelError> {
    let mut writer = crate::primitives::DigestWriter::new(domain, 1)
        .map_err(|_| KernelError::InvariantViolation)?;
    serde_json_canonicalizer::to_writer(value, &mut writer)
        .map_err(|_| KernelError::InvariantViolation)?;
    Ok(writer.finish())
}

fn failure_from_state(state: &KernelState, error: ErrorDescriptor) -> RunFailed {
    let turn = state.current_turn.as_ref();
    RunFailed {
        cycle: state.cycle,
        turn_id: turn.map(|value| value.turn_id),
        model_request_id: turn.and_then(|value| value.model_request_id),
        effect_id: turn.and_then(|value| value.effect_id),
        error,
    }
}

/// Deterministic, synchronous owner of authoritative semantic state.
#[derive(Debug, Clone, Default)]
pub struct Kernel {
    state: KernelState,
}

impl Kernel {
    /// Borrow authoritative replay-derived state.
    #[must_use]
    pub const fn state(&self) -> &KernelState {
        &self.state
    }

    /// Produce a pure deterministic decision from current state and normalized input.
    ///
    /// # Arguments
    ///
    /// * `env` - Caller-supplied time and preallocated identities for this
    ///   transition. The kernel does not allocate IDs or read a clock.
    /// * `input` - Normalized command or settlement. The legal family depends on
    ///   the current [`KernelState::phase`].
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] for invalid phase/input combinations, settlement
    /// conflicts, semantic payload mismatches, or allocated-ID mismatches.
    ///
    /// # Examples
    ///
    /// See the crate-level "Model-only decide, commit, and apply" example.
    pub fn decide(&self, env: &TransitionEnv, input: KernelInput) -> Result<Decision, KernelError> {
        decide::decide(&self.state, env, input)
    }

    /// Transactionally apply an atomic committed batch and derive public events.
    ///
    /// The complete batch is preflighted, then applied to a working copy. The
    /// authoritative state changes only if every record succeeds. Full-state
    /// validate stays on restore, deserialize, and [`KernelState::state_hash`].
    /// `first_transient_sequence` is the runtime sequencer's next unused value;
    /// derived events consume contiguous values from it.
    ///
    /// # Arguments
    ///
    /// * `committed` - Atomic store result whose sequences and record identities
    ///   must match the preceding [`Decision`].
    /// * `first_transient_sequence` - Next unused runtime sequencer value.
    ///   Derived events consume contiguous values from it.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] for invalid ranges, sequences, identities,
    /// sibling ordering, settlement digests, or state transitions.
    ///
    /// # Examples
    ///
    /// See the crate-level "Model-only decide, commit, and apply" example.
    pub fn apply(
        &mut self,
        committed: &CommittedBatch,
        first_transient_sequence: u64,
    ) -> Result<Arc<[RunEvent]>, KernelError> {
        let (state, events) = apply::apply(&self.state, committed, first_transient_sequence)?;
        self.state = state;
        Ok(events)
    }

    /// Install already-replayed state after snapshot validation.
    ///
    /// The caller must treat the snapshot as a disposable cache: this only
    /// hydrates validated `KernelState`. It does not consult a journal.
    ///
    /// # Arguments
    ///
    /// * `state` - Already-replayed snapshot. It is validated and hashed before
    ///   the kernel accepts it; the journal is not consulted.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] when the state fails [`KernelState::validate`]
    /// or cannot produce a `kernel-state` digest.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{Kernel, KernelState};
    ///
    /// let kernel = Kernel::try_restore(KernelState::default()).expect("restore");
    /// assert_eq!(kernel.state().last_applied_sequence(), 0);
    /// ```
    pub fn try_restore(state: KernelState) -> Result<Self, KernelError> {
        state.validate()?;
        let _ = state.hash_projection()?;
        Ok(Self { state })
    }

    /// Adopt a filtered session head without replaying the skipped records.
    ///
    /// Recovery paths that replay a filtered journal view use this to move
    /// `last_applied_sequence` forward to the durable head so subsequent
    /// appends continue from the store's sequence. The sequence never moves
    /// backwards.
    ///
    /// # Arguments
    ///
    /// * `sequence` - Durable session head reported by the store.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvariantViolation`] when `sequence` is behind
    /// the replayed head.
    pub fn adopt_session_head(&mut self, sequence: u64) -> Result<(), KernelError> {
        if self.state.last_applied_sequence > sequence {
            return Err(KernelError::InvariantViolation);
        }
        self.state.last_applied_sequence = sequence;
        Ok(())
    }
}

#[cfg(test)]
mod restore_tests {
    use super::{Kernel, KernelState};

    #[test]
    fn try_restore_accepts_default_state() {
        let restored = Kernel::try_restore(KernelState::default()).expect("restore");
        assert_eq!(restored.state().last_applied_sequence, 0);
        assert_eq!(
            restored.state().state_hash().expect("hash"),
            KernelState::default().state_hash().expect("hash")
        );
    }
}
