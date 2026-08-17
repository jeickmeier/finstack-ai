use finstack_ai_kernel::{
    ComponentId, EffectCompleted, EffectInput, EffectKind, EffectOutputKind, EffectRequested,
    InvocationRecovery, RecordBody, RecordEnvelope, RetrySafety,
};

use crate::context::InvocationResumeAction;

use super::MIDDLEWARE_OUTCOME_NOT_ALLOWED;
use super::error::MiddlewareError;
use super::port::Middleware;
use super::types::{MiddlewareContext, MiddlewareDescriptor, StageInput, StageOutcome, stage_name};
use super::validate::validate_stage_outcome;
use super::{Stage, parse_stage};

/// Guard proving a middleware invocation is backed by an exact committed effect record.
///
/// # Deprecated in place: unconstructible in a run
///
/// [`Self::try_new`] validates an `EffectRequested` record of kind
/// `EffectKind::Middleware`. No `KernelInput` ever commits one, so outside this
/// module's own tests the guard can never be constructed and
/// [`Self::invoke`] is never reached. `crate::middleware_driver` invokes
/// components directly and folds their outcomes instead. Kept because it is
/// frozen 1.0 public API; see section 5 of the [`crate::middleware_driver`]
/// module contract and the `middleware-aggregate-fold` ADR under
/// `docs/implementation/adrs/`.
#[derive(Debug, Clone)]
pub struct CommittedMiddlewareCall {
    context: MiddlewareContext,
    input: StageInput,
    requested: EffectRequested,
}

impl CommittedMiddlewareCall {
    /// Validate the committed envelope against the exact component, locator, input, and cursor.
    ///
    /// # Errors
    ///
    /// Returns `middleware_commit_required` for any mismatch.
    pub fn try_new(
        envelope: &RecordEnvelope,
        context: MiddlewareContext,
        input: StageInput,
        descriptor: &MiddlewareDescriptor,
    ) -> Result<Self, MiddlewareError> {
        let RecordBody::EffectRequested(requested) = envelope.body() else {
            return Err(MiddlewareError::commit_required());
        };
        let pipeline = requested
            .pipeline()
            .ok_or_else(MiddlewareError::commit_required)?;
        let raw = input.to_raw_json()?;
        let stage = input.stage();
        let locator = &context.run.locator;
        if envelope.sequence() == 0
            || envelope.session_id() != locator.session_id
            || envelope.lane_id() != locator.lane_id
            || envelope.run_id() != Some(locator.run_id)
            || requested.effect_id() != context.run.effect_id
            || requested.kind() != EffectKind::Middleware
            || requested.component() != Some(&descriptor.invocation)
            || requested.output_contract().kind != EffectOutputKind::MiddlewareOutcome
            || requested.deadline() != context.run.deadline
            || requested.retry_safety() == RetrySafety::Unknown
            || pipeline.chain_digest() != context.chain_digest
            || pipeline.index() != context.chain_index
            || pipeline.stage() != stage_name(stage)
            || !matches!(requested.input(), EffectInput::Middleware { stage: committed_stage, input } if committed_stage.as_ref() == stage_name(stage) && input == &raw)
        {
            return Err(MiddlewareError::commit_required());
        }
        Ok(Self {
            context,
            input,
            requested: requested.clone(),
        })
    }

    /// Invoke the component and validate its normalized outcome for this exact stage/role.
    ///
    /// # Errors
    ///
    /// Returns a stable descriptor, port, stage-matrix, or compaction-integrity error.
    pub async fn invoke(
        self,
        middleware: &dyn Middleware,
    ) -> Result<StageOutcome, MiddlewareError> {
        let descriptor = middleware.descriptor();
        if self.requested.component() != Some(&descriptor.invocation) {
            return Err(MiddlewareError::commit_required());
        }
        let outcome = middleware.invoke(self.context, self.input.clone()).await?;
        validate_stage_outcome(&descriptor, &self.input, &outcome)?;
        Ok(outcome)
    }
}

/// A middleware output proven to have been committed after its exact request.
///
/// # Deprecated in place: reconstructs records that are never written
///
/// [`Self::try_from_records`] pairs an `EffectRequested` of kind
/// `EffectKind::Middleware` with its `EffectCompleted`. Neither record is ever
/// committed, so no journal contains the pair and nothing outside this module's
/// tests calls it. See [`CommittedMiddlewareCall`] for the same reason at the
/// request end, and section 5 of the [`crate::middleware_driver`] module
/// contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMiddlewareOutcome {
    /// Middleware component.
    pub component: ComponentId,
    /// Locked chain index.
    pub chain_index: u32,
    /// Frozen stage.
    pub stage: Stage,
    /// Normalized output.
    pub outcome: StageOutcome,
}

impl RecordedMiddlewareOutcome {
    /// Reconstruct one replay-safe outcome from request/completion envelopes.
    ///
    /// # Errors
    ///
    /// Returns a stable error for identity, sequence, contract, or payload mismatch.
    pub fn try_from_records(
        requested_envelope: &RecordEnvelope,
        completed_envelope: &RecordEnvelope,
    ) -> Result<Self, MiddlewareError> {
        let RecordBody::EffectRequested(requested) = requested_envelope.body() else {
            return Err(MiddlewareError::commit_required());
        };
        let RecordBody::EffectCompleted(completed) = completed_envelope.body() else {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware completion record is missing",
            ));
        };
        let invocation = requested
            .component()
            .ok_or_else(MiddlewareError::commit_required)?;
        let pipeline = requested
            .pipeline()
            .ok_or_else(MiddlewareError::commit_required)?;
        let stage = parse_stage(pipeline.stage()).ok_or_else(|| {
            MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "committed middleware stage is invalid",
            )
        })?;
        if requested.kind() != EffectKind::Middleware
            || requested.output_contract().kind != EffectOutputKind::MiddlewareOutcome
            || completed_envelope.sequence() <= requested_envelope.sequence()
            || completed_envelope.session_id() != requested_envelope.session_id()
            || completed_envelope.lane_id() != requested_envelope.lane_id()
            || completed_envelope.run_id() != requested_envelope.run_id()
            || completed.validate_against(requested).is_err()
        {
            return Err(MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "middleware completion does not match its committed request",
            ));
        }
        let outcome = serde_json::from_slice(completed.output().as_bytes()).map_err(|_| {
            MiddlewareError::stable(
                MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                "committed middleware outcome is invalid",
            )
        })?;
        Ok(Self {
            component: invocation.component.clone(),
            chain_index: pipeline.index(),
            stage,
            outcome,
        })
    }
}

/// Determine recovery behavior from committed request/output state.
///
/// # Deprecated in place: never called on a recovery path
///
/// This is the function that would honour a descriptor's
/// `InvocationRecovery::NonRepeatable` marker, and nothing calls it: the only
/// callers are this crate's tests and `crates/finstack-ai-test/tests/crash_prefix.rs`.
/// Under the aggregate-fold design there is no committed middleware
/// `EffectRequested` to pass in, and recovery re-runs a cursor's whole chain
/// unconditionally when its `StageOutcomeRecorded` is absent — so a
/// `NonRepeatable` middleware is re-invoked rather than suspended. That gap is
/// section 2 of the [`crate::middleware_driver`] module contract; this function
/// is not the mitigation for it.
#[must_use]
pub fn middleware_resume_action(
    requested: &EffectRequested,
    completed: Option<&EffectCompleted>,
) -> InvocationResumeAction {
    if completed.is_some_and(|value| value.validate_against(requested).is_ok()) {
        return InvocationResumeAction::UseRecorded;
    }
    match requested.component().map(|value| value.recovery) {
        Some(InvocationRecovery::RecomputeSafe) => InvocationResumeAction::Recompute,
        Some(InvocationRecovery::Reconcile) => InvocationResumeAction::Reconcile,
        Some(InvocationRecovery::NonRepeatable) | None => InvocationResumeAction::SuspendUncertain,
    }
}
