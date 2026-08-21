//! Durable context-provider and middleware effect decisions.

use super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::canonical_digest;
use super::capacity::{self, StateGrowth};
use super::decide::{draft_for_state, duplicate_decision, expected_stage_cursor, next_sequence};
use super::decision::{Decision, KernelError, PostCommitAction};
use super::input::{ExtensionEffectSettled, ExtensionSettlement, RequestExtensionEffect};
use crate::effects::{EffectInput, EffectKind, EffectOutputKind, InvocationRecovery};
use crate::records::{RECORD_KIND_VERSION, RecordBody};
use crate::state::{
    ExtensionSettlementFingerprint, ExtensionSettlementKind, KernelState, TransitionEnv,
};

pub(super) fn decide_request(
    state: &KernelState,
    env: &TransitionEnv,
    input: &RequestExtensionEffect,
) -> Result<Decision, KernelError> {
    if let Some(pending) = &state.pending_extension_effect {
        return if pending.requested == input.requested {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if state.terminal.is_some() || state.cancellation.is_some() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "request_extension_effect",
        });
    }
    let requested = &input.requested;
    if state
        .extension_settlements
        .contains_key(&requested.effect_id())
    {
        return Err(KernelError::ConflictingSettlement);
    }
    let cursor = request_cursor(requested)?;
    let expected = expected_stage_cursor(state).ok_or(KernelError::InvalidPhaseInput {
        phase: state.phase,
        input: "request_extension_effect",
    })?;
    if cursor != expected {
        return Err(KernelError::StageCursorMismatch {
            expected,
            actual: cursor,
        });
    }
    validate_request_shape(requested)?;
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 1, 1, 0, 0, 0))?;
    if env.ids.effect_ids().first().copied() != Some(requested.effect_id()) {
        return Err(KernelError::UnusedAllocatedIds { kind: "effect_ids" });
    }
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(
            state,
            env,
            vec![RecordBody::EffectRequested(requested.clone())],
        )?,
        actions: vec![PostCommitAction::ExecuteEffect {
            effect_id: requested.effect_id(),
        }],
        diagnostics: Vec::new(),
    })
}

pub(super) fn decide_settled(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ExtensionEffectSettled,
) -> Result<Decision, KernelError> {
    let digest = settlement_digest(input)?;
    let kind = settlement_kind(&input.outcome);
    let effect_id = input.outcome.effect_id();
    let completion = match &input.outcome {
        ExtensionSettlement::Completed(value) => value.completion_id(),
        ExtensionSettlement::Failed(value) => value.completion_id(),
    };
    if let Some(completion_id) = completion
        && let Some(existing) = state.completion_identities.get(completion_id)
    {
        return if existing.effect_id == effect_id && existing.settlement_digest == digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.extension_settlements.get(&effect_id) {
        return if existing.kind == kind && existing.digest == digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if state.cancellation.is_some() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "extension_effect_settled",
        });
    }
    let pending = state
        .pending_extension_effect
        .as_ref()
        .ok_or(KernelError::EffectNotPending { effect_id })?;
    if pending.cursor != input.cursor || pending.requested.effect_id() != effect_id {
        return Err(KernelError::ConflictingSettlement);
    }
    match &input.outcome {
        ExtensionSettlement::Completed(value) => value
            .validate_against(&pending.requested)
            .map_err(|_| KernelError::ConflictingSettlement)?,
        ExtensionSettlement::Failed(value) => value
            .validate_against(&pending.requested)
            .map_err(|_| KernelError::ConflictingSettlement)?,
    }
    capacity::preflight_decision(
        state,
        StateGrowth {
            extension: Some(effect_id),
            completion,
            ..StateGrowth::default()
        },
    )?;
    let body = match &input.outcome {
        ExtensionSettlement::Completed(value) => RecordBody::EffectCompleted(value.clone()),
        ExtensionSettlement::Failed(value) => RecordBody::EffectFailed(value.clone()),
    };
    let event_count = body
        .derived_event_count(RECORD_KIND_VERSION)
        .map_err(|_| KernelError::InvariantViolation)?;
    validate_allocated_ids(&env.ids, IdRequirements::new(1, event_count, 0, 0, 0, 0))?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(state, env, vec![body])?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

pub(super) fn request_cursor(
    requested: &crate::EffectRequested,
) -> Result<crate::StageCursor, KernelError> {
    match requested.input() {
        EffectInput::Context { cursor, .. } | EffectInput::Middleware { cursor, .. } => Ok(*cursor),
        _ => Err(KernelError::InvalidInputPayload {
            field: "requested.input",
            reason_code: "not_extension_effect",
        }),
    }
}

pub(super) fn validate_request_shape(
    requested: &crate::EffectRequested,
) -> Result<(), KernelError> {
    if requested.relation().is_some() || requested.component().is_none() {
        return Err(KernelError::InvalidInputPayload {
            field: "requested",
            reason_code: "invalid_extension_contract",
        });
    }
    let valid = match requested.input() {
        EffectInput::Context { cursor, .. } => {
            requested.kind() == EffectKind::Context
                && cursor.stage == crate::Stage::PrepareContext
                && requested
                    .pipeline()
                    .is_some_and(|pipeline| pipeline.stage() == "prepare_context")
                && requested.output_contract().kind == EffectOutputKind::ContextContribution
        }
        EffectInput::Middleware { cursor, stage, .. } => {
            requested.kind() == EffectKind::Middleware
                && stage.as_ref() == stage_name(cursor.stage)
                && requested.output_contract().kind == EffectOutputKind::MiddlewareOutcome
                && requested.component().is_some_and(|component| {
                    component.recovery == InvocationRecovery::RecomputeSafe
                })
                && requested
                    .pipeline()
                    .is_some_and(|pipeline| pipeline.stage() == stage.as_ref())
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(KernelError::InvalidInputPayload {
            field: "requested",
            reason_code: "invalid_extension_contract",
        })
    }
}

pub(crate) const fn stage_name(stage: crate::Stage) -> &'static str {
    match stage {
        crate::Stage::BeforeRun => "before_run",
        crate::Stage::PrepareContext => "prepare_context",
        crate::Stage::BeforeModel => "before_model",
        crate::Stage::AfterModel => "after_model",
        crate::Stage::BeforeToolBatch => "before_tool_batch",
        crate::Stage::AfterToolBatch => "after_tool_batch",
        crate::Stage::BeforeFinalize => "before_finalize",
    }
}

pub(super) fn settlement_digest(
    input: &ExtensionEffectSettled,
) -> Result<crate::Digest, KernelError> {
    canonical_digest("extension-settlement", input)
}

pub(super) const fn settlement_kind(outcome: &ExtensionSettlement) -> ExtensionSettlementKind {
    match outcome {
        ExtensionSettlement::Completed(_) => ExtensionSettlementKind::Completed,
        ExtensionSettlement::Failed(_) => ExtensionSettlementKind::Failed,
    }
}

pub(super) fn fingerprint(
    input: &ExtensionEffectSettled,
) -> Result<ExtensionSettlementFingerprint, KernelError> {
    Ok(ExtensionSettlementFingerprint {
        kind: settlement_kind(&input.outcome),
        digest: settlement_digest(input)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ComponentId, ComponentInvocation, Digest, EffectId, EffectOutputContract,
        InvocationRecovery, PipelinePosition, RawJson, RetrySafety, Stage, StageCursor, Version,
    };

    #[test]
    fn context_request_requires_prepare_context_pipeline() {
        let cursor = StageCursor {
            cycle: 0,
            stage: Stage::PrepareContext,
        };
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
        let component = ComponentInvocation {
            component: ComponentId::parse("finstack.context.fixture").expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"config"),
            recovery: InvocationRecovery::Reconcile,
        };
        let contract = EffectOutputContract {
            kind: EffectOutputKind::ContextContribution,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"context-output"),
        };
        let build = |pipeline| {
            crate::EffectRequested::try_new(
                effect_id,
                EffectKind::Context,
                None,
                Some(component.clone()),
                pipeline,
                contract.clone(),
                EffectInput::Context {
                    cursor,
                    request: RawJson::parse("{}").expect("request"),
                },
                RetrySafety::SafeToRetry,
                None,
            )
            .expect("effect request")
        };
        let valid = build(Some(
            PipelinePosition::try_new(Digest::raw_json(b"chain"), "prepare_context", 0)
                .expect("pipeline"),
        ));
        assert_eq!(validate_request_shape(&valid), Ok(()));
        assert!(validate_request_shape(&build(None)).is_err());
        let wrong = build(Some(
            PipelinePosition::try_new(Digest::raw_json(b"chain"), "before_model", 0)
                .expect("pipeline"),
        ));
        assert!(validate_request_shape(&wrong).is_err());
    }
}
