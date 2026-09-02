use finstack_ai_kernel::{
    ComponentId, EffectInput, EffectKind, EffectOutputKind, EffectRequested, RecordBody,
    RecordEnvelope, RetrySafety,
};

use crate::ports::model::ReconcileContext;

use super::error::{
    CONTEXT_BUDGET_EXCEEDED, CONTEXT_CONTRIBUTION_INVALID, CONTEXT_RECOVERY_UNCERTAIN, ContextError,
};
use super::port::{
    CONTEXT_STAGE, ContextCallContext, ContextProvider, ContextProviderDescriptor,
    ContextReconcileResult, PendingContextEffect,
};
use super::types::{
    ContextAuthority, ContextBudget, ContextContribution, ContextItemKind, ContextOverflowPolicy,
    ContextRequest,
};

/// Guard proving a context invocation is backed by a committed effect record.
#[derive(Debug, Clone)]
pub struct CommittedContextCall {
    context: ContextCallContext,
    request: ContextRequest,
    requested: EffectRequested,
    sequence: u64,
}

impl CommittedContextCall {
    /// Validate a committed envelope against the exact provider, request, locator, and cursor.
    ///
    /// # Errors
    ///
    /// Returns `context_commit_required` when any committed identity or digest differs.
    pub fn try_new(
        envelope: &RecordEnvelope,
        context: ContextCallContext,
        request: ContextRequest,
        descriptor: &ContextProviderDescriptor,
    ) -> Result<Self, ContextError> {
        let RecordBody::EffectRequested(requested) = envelope.body() else {
            return Err(ContextError::commit_required());
        };
        let raw = request.to_raw_json()?;
        let expected_pipeline = requested
            .pipeline()
            .ok_or_else(ContextError::commit_required)?;
        let locator = &context.run.locator;
        if envelope.sequence() == 0
            || envelope.session_id() != locator.session_id
            || envelope.lane_id() != locator.lane_id
            || envelope.run_id() != Some(locator.run_id)
            || request.session_id != locator.session_id
            || request.lane_id != locator.lane_id
            || request.run_id != locator.run_id
            || requested.effect_id() != context.run.effect_id
            || requested.kind() != EffectKind::Context
            || requested.component() != Some(&descriptor.invocation)
            || expected_pipeline.chain_digest() != context.chain_digest
            || expected_pipeline.stage() != CONTEXT_STAGE
            || expected_pipeline.index() != context.provider_index
            || requested.deadline() != context.run.deadline
            || requested.retry_safety() == RetrySafety::Unknown
            || requested.output_contract().kind != EffectOutputKind::ContextContribution
            || !matches!(requested.input(), EffectInput::Context { request, .. } if request == &raw)
        {
            return Err(ContextError::commit_required());
        }
        Ok(Self {
            context,
            request,
            requested: requested.clone(),
            sequence: envelope.sequence(),
        })
    }

    /// Invoke the exact provider only after the committed guard has been constructed.
    ///
    /// # Errors
    ///
    /// Returns a stable error when the descriptor drifts, the provider fails, or output is invalid.
    pub async fn invoke(
        self,
        provider: &dyn ContextProvider,
    ) -> Result<ContextContribution, ContextError> {
        let descriptor = provider.descriptor();
        if self.requested.component() != Some(&descriptor.invocation) {
            return Err(ContextError::commit_required());
        }
        let contribution = provider
            .collect(self.context.clone(), self.request.clone())
            .await?;
        self.accept_contribution(&descriptor, contribution)
    }

    /// Reconcile after a committed request and before a first collect.
    ///
    /// `Completed` reuses the contribution. `NotStarted` / `RetrySafe` collect
    /// with the same effect identity. `Unknown` / `NonRepeatable` fail closed.
    ///
    /// # Errors
    ///
    /// Returns `context_recovery_uncertain` when the provider cannot classify
    /// the outstanding invocation, or a collect/validation failure.
    pub async fn resume(
        self,
        provider: &dyn ContextProvider,
    ) -> Result<ContextContribution, ContextError> {
        let descriptor = provider.descriptor();
        if self.requested.component() != Some(&descriptor.invocation) {
            return Err(ContextError::commit_required());
        }
        let pipeline = self
            .requested
            .pipeline()
            .cloned()
            .ok_or_else(ContextError::commit_required)?;
        let result = provider
            .reconcile(
                ReconcileContext {
                    run: self.context.run.clone(),
                    original_input_digest: self.requested.input_digest(),
                },
                PendingContextEffect {
                    request: self.request.clone(),
                    invocation: descriptor.invocation.clone(),
                    pipeline,
                },
            )
            .await?;
        match map_context_reconcile_result(&result) {
            InvocationResumeAction::UseRecorded => match result {
                ContextReconcileResult::Completed(contribution) => {
                    self.accept_contribution(&descriptor, contribution)
                }
                _ => Err(ContextError::stable(
                    CONTEXT_RECOVERY_UNCERTAIN,
                    "completed resume missing contribution",
                )),
            },
            InvocationResumeAction::Recompute => {
                let contribution = provider
                    .collect(self.context.clone(), self.request.clone())
                    .await?;
                self.accept_contribution(&descriptor, contribution)
            }
            InvocationResumeAction::Reconcile | InvocationResumeAction::SuspendUncertain => {
                Err(ContextError::stable(
                    CONTEXT_RECOVERY_UNCERTAIN,
                    "context resume is non-repeatable or unknown",
                ))
            }
        }
    }

    fn accept_contribution(
        &self,
        descriptor: &ContextProviderDescriptor,
        mut contribution: ContextContribution,
    ) -> Result<ContextContribution, ContextError> {
        contribution.validate()?;
        normalize_instruction_authority(
            &mut contribution,
            descriptor.trusted_application_instructions,
        );
        contribution.validate()?;
        if self.request.budget.overflow == ContextOverflowPolicy::Reject
            && !contribution_fits_budget(&contribution, self.request.budget)
        {
            return Err(ContextError::stable(
                CONTEXT_BUDGET_EXCEEDED,
                "context provider contribution exceeds its explicit budget",
            ));
        }
        Ok(contribution)
    }

    /// Store-assigned request sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
}

/// A context output proven to have been committed after its request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedContextContribution {
    /// Provider component.
    pub component: ComponentId,
    /// Locked provider index.
    pub provider_index: u32,
    /// Normalized contribution.
    pub contribution: ContextContribution,
}

impl RecordedContextContribution {
    /// Reconstruct a replay-safe contribution from request and completion envelopes.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for identity, order, or payload mismatch.
    pub fn try_from_records(
        requested_envelope: &RecordEnvelope,
        completed_envelope: &RecordEnvelope,
    ) -> Result<Self, ContextError> {
        let RecordBody::EffectRequested(requested) = requested_envelope.body() else {
            return Err(ContextError::commit_required());
        };
        let RecordBody::EffectCompleted(completed) = completed_envelope.body() else {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context completion record is missing",
            ));
        };
        let component = requested
            .component()
            .ok_or_else(ContextError::commit_required)?
            .component
            .clone();
        let pipeline = requested
            .pipeline()
            .ok_or_else(ContextError::commit_required)?;
        if requested.kind() != EffectKind::Context
            || requested.output_contract().kind != EffectOutputKind::ContextContribution
            || pipeline.stage() != CONTEXT_STAGE
            || completed_envelope.sequence() <= requested_envelope.sequence()
            || completed_envelope.session_id() != requested_envelope.session_id()
            || completed_envelope.lane_id() != requested_envelope.lane_id()
            || completed_envelope.run_id() != requested_envelope.run_id()
            || completed.validate_against(requested).is_err()
        {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context completion does not match its committed request",
            ));
        }
        let contribution: ContextContribution =
            serde_json::from_slice(completed.output().as_bytes()).map_err(|_| {
                ContextError::stable(
                    CONTEXT_CONTRIBUTION_INVALID,
                    "committed context contribution is invalid",
                )
            })?;
        contribution.validate()?;
        Ok(Self {
            component,
            provider_index: pipeline.index(),
            contribution,
        })
    }
}

/// Replay decision for a committed but unsettled component invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationResumeAction {
    /// Use the already committed output.
    UseRecorded,
    /// Rerun with the same effect identity.
    Recompute,
    /// Call the component reconciliation hook.
    Reconcile,
    /// Suspend because repeating the invocation could duplicate a non-repeatable effect.
    SuspendUncertain,
}

/// Map one provider reconcile result onto the existing context resume taxonomy.
///
/// Mirrors the model reconcile mapper: `Completed` reuses the
/// contribution, `NotStarted` / `RetrySafe` retry the same effect identity,
/// and `Unknown` / `NonRepeatable` fail closed.
#[must_use]
pub(crate) fn map_context_reconcile_result(
    result: &ContextReconcileResult,
) -> InvocationResumeAction {
    match result {
        ContextReconcileResult::Completed(_) => InvocationResumeAction::UseRecorded,
        ContextReconcileResult::NotStarted | ContextReconcileResult::RetrySafe => {
            InvocationResumeAction::Recompute
        }
        ContextReconcileResult::Unknown | ContextReconcileResult::NonRepeatable => {
            InvocationResumeAction::SuspendUncertain
        }
    }
}

fn normalize_instruction_authority(
    contribution: &mut ContextContribution,
    trusted_instructions: bool,
) {
    let items = contribution
        .items
        .iter()
        .cloned()
        .map(|mut item| {
            if item.kind == ContextItemKind::Instruction {
                if trusted_instructions && !item.provenance.external {
                    item.authority = ContextAuthority::TrustedApplication;
                } else {
                    item.kind = ContextItemKind::QuotedSource;
                    item.authority = ContextAuthority::Untrusted;
                }
            }
            item
        })
        .collect::<Vec<_>>();
    contribution.items = items.into();
}

fn contribution_fits_budget(contribution: &ContextContribution, budget: ContextBudget) -> bool {
    contribution.items.len() <= budget.max_items
        && contribution.estimated_tokens <= budget.max_tokens
        && contribution.bytes <= budget.max_bytes
}
