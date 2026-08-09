//! Pure decision and transactional committed-record application.

mod allocated_ids;
mod apply;
mod capacity;
mod decide;
mod decision;
mod fingerprint;
mod input;
mod validation;

use std::sync::Arc;

use serde::Serialize;

pub use decision::{CommittedBatch, Decision, KernelError, PostCommitAction};
pub use input::{
    AcceptRun, ExternalEffectCompletedInput, ExternalEffectCompletion, ExternalEffectOutcome,
    KernelInput, ModelSettled, ModelSettlement, ReducerStageOutcome, StageSettled,
};

use crate::entries::RunFailed;
use crate::events::RunEvent;
use crate::state::{KernelState, TransitionEnv};
use crate::{Digest, ErrorDescriptor};

fn canonical_digest<T: Serialize>(domain: &'static str, value: &T) -> Result<Digest, KernelError> {
    let canonical =
        serde_json_canonicalizer::to_vec(value).map_err(|_| KernelError::InvariantViolation)?;
    Digest::domain_separated(domain, 1, &canonical).map_err(|_| KernelError::InvariantViolation)
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
    /// # Errors
    ///
    /// Returns [`KernelError`] for invalid phase/input combinations, settlement
    /// conflicts, semantic payload mismatches, or allocated-ID mismatches.
    pub fn decide(&self, env: &TransitionEnv, input: KernelInput) -> Result<Decision, KernelError> {
        decide::decide(&self.state, env, input)
    }

    /// Transactionally apply an atomic committed batch and derive public events.
    ///
    /// The complete batch is validated and applied to a temporary state. The
    /// authoritative state changes only if every record succeeds.
    /// `first_transient_sequence` is the runtime sequencer's next unused value;
    /// derived events consume contiguous values from it.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] for invalid ranges, sequences, identities,
    /// sibling ordering, settlement digests, or state transitions.
    pub fn apply(
        &mut self,
        committed: &CommittedBatch,
        first_transient_sequence: u64,
    ) -> Result<Arc<[RunEvent]>, KernelError> {
        let (state, events) = apply::apply(&self.state, committed, first_transient_sequence)?;
        self.state = state;
        Ok(events)
    }
}
