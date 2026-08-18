//! Runtime-owned model-assisted compaction phase (ADR-042).
//!
//! Runs between context collect and the BeforeModel middleware fold. Commits a
//! child `EffectRequested(Model)` under `EffectPurpose::CompactionSummary`,
//! executes it through [`validate_model_request`], and re-enters the chain
//! with [`CompactionModelResume`]. Middleware stays non-effect-bearing.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EffectCompleted, EffectId, EffectKind, EffectOutputKind,
    EffectPurpose, EffectRelation, EffectTag, EventTag, KernelInput, Message, MessageRole,
    MessageTag, Metadata, ModelRef, ModelRequestId, ModelSettled, ModelSettlement, RecordBody,
    RecordTag, RequestCompactionModel, Stage, TransitionEnv,
};

use crate::coordinator::CommitCoordinator;
use crate::middleware::{CompactionModelRequest, CompactionModelResume, StageOutcome};
use crate::middleware_driver::derived_stage_effect_id;
use crate::model::{
    Model, ModelCallContext, ModelRequest, ModelResponse, ModelStreamAssembler, ModelStreamLimits,
    ModelTerminal, validate_model_request,
};
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;
use crate::{
    CancellationSignal, Clock, LoadRequest, LockedModelContextProfile, RandomSource, RunCallContext,
};

const COMPACTION_PHASE_UNAVAILABLE: &str = "compaction_phase_unavailable";

/// Fulfill one [`CompactionModelRequest`] with commit-before-effect.
///
/// # Errors
///
/// Returns a stable middleware or model-settlement error when the request
/// cannot be committed, validated, executed, or settled.
pub(crate) async fn fulfill_compaction_model<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    model: &dyn Model,
    request: &CompactionModelRequest,
    cycle: u64,
    cancellation: &CancellationSignal,
) -> Result<CompactionModelResume, RunHandleError> {
    if coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .is_some_and(|pending| pending.requested.is_compaction_summary())
    {
        return execute_and_settle(coordinator, sources, profile, model, request, cancellation)
            .await;
    }
    validate_model_request(model, &request.request, profile).map_err(model_handle_error)?;
    let seed = coordinator
        .stage_dispatch_seed()
        .ok_or_else(|| stage_error(COMPACTION_PHASE_UNAVAILABLE))?;
    let parent_effect_id = derived_stage_effect_id(&seed.locator, cycle, Stage::BeforeModel);
    let request_json = request
        .request
        .canonical_bytes()
        .map_err(|_| stage_error(crate::MODEL_REQUEST_INVALID))?;
    let request_json = finstack_ai_kernel::RawJson::parse(request_json)
        .map_err(|_| stage_error(crate::MODEL_REQUEST_INVALID))?;
    let middleware_component_id =
        finstack_ai_kernel::ComponentId::parse("finstack.middleware.compaction")
            .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?;
    let component =
        request
            .model
            .version()
            .map(|version| finstack_ai_kernel::ComponentInvocation {
                component: request.model.id().clone(),
                version,
                configuration_digest: finstack_ai_kernel::Digest::raw_json(b"compaction-summary"),
                recovery: finstack_ai_kernel::InvocationRecovery::Reconcile,
            });
    let env = compaction_request_env(sources)?;
    coordinator
        .submit(
            env,
            KernelInput::RequestCompactionModel(RequestCompactionModel {
                request: request_json,
                component,
                relation: EffectRelation {
                    parent_effect_id,
                    purpose: EffectPurpose::CompactionSummary {
                        middleware_component_id,
                    },
                },
                output_contract: finstack_ai_kernel::EffectOutputContract {
                    kind: EffectOutputKind::ModelResponse,
                    schema_version: 1,
                    schema_digest: finstack_ai_kernel::Digest::raw_json(b"model-response-v1"),
                },
                retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
                deadline: seed.deadline,
            }),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    execute_and_settle(coordinator, sources, profile, model, request, cancellation).await
}

/// Resume a committed compaction-summary model effect without a new user turn.
///
/// # Errors
///
/// Returns a stable middleware or model-settlement error when the pending
/// compaction effect cannot be reconstructed or settled.
pub(crate) async fn resume_pending_compaction_model<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    model: &dyn Model,
    cancellation: &CancellationSignal,
) -> Result<bool, RunHandleError> {
    let Some(pending) = coordinator.state().pending_model_effect.clone() else {
        return Ok(false);
    };
    if !pending.requested.is_compaction_summary() {
        return Ok(false);
    }
    let finstack_ai_kernel::EffectInput::Model { request: raw } = pending.requested.input() else {
        return Err(stage_error(COMPACTION_PHASE_UNAVAILABLE));
    };
    let draft: crate::ModelRequestDraft = serde_json::from_slice(raw.as_bytes())
        .map_err(|_| stage_error(crate::MODEL_REQUEST_INVALID))?;
    let model_ref = match pending.requested.component() {
        Some(invocation) => finstack_ai_kernel::ComponentRef::new(
            invocation.component.clone(),
            Some(invocation.version),
        ),
        None => finstack_ai_kernel::ComponentRef::new(
            finstack_ai_kernel::ComponentId::parse("finstack.model.compaction")
                .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?,
            None,
        ),
    };
    let budget_scope_id = coordinator
        .stage_dispatch_seed()
        .and_then(|seed| seed.budget_scope_id)
        .unwrap_or_else(|| {
            finstack_ai_kernel::BudgetScopeId::from_bytes(*pending.requested.effect_id().as_bytes())
        });
    let request = CompactionModelRequest {
        model: model_ref,
        request: draft,
        budget_scope_id,
        source_sensitivity: finstack_ai_kernel::Sensitivity::Internal,
        residency_policy_digest: finstack_ai_kernel::Digest::raw_json(b"compaction-resume"),
        resume_state: finstack_ai_kernel::RawJson::parse(b"{}")
            .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?,
    };
    fulfill_compaction_model(
        coordinator,
        sources,
        profile,
        model,
        &request,
        pending.cycle,
        cancellation,
    )
    .await?;
    Ok(true)
}

async fn execute_and_settle<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    profile: &LockedModelContextProfile,
    model: &dyn Model,
    request: &CompactionModelRequest,
    cancellation: &CancellationSignal,
) -> Result<CompactionModelResume, RunHandleError> {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .ok_or_else(|| stage_error(COMPACTION_PHASE_UNAVAILABLE))?
        .clone();
    if !pending.requested.is_compaction_summary() {
        return Err(stage_error(COMPACTION_PHASE_UNAVAILABLE));
    }
    validate_model_request(model, &request.request, profile).map_err(model_handle_error)?;
    let seed = coordinator
        .stage_dispatch_seed()
        .ok_or_else(|| stage_error(COMPACTION_PHASE_UNAVAILABLE))?;
    let run = RunCallContext {
        effect_id: pending.requested.effect_id(),
        locator: seed.locator,
        authorization: seed.authorization,
        attempt: seed.attempt,
        deadline: pending.requested.deadline(),
        budget_scope_id: Some(request.budget_scope_id),
        cancellation: cancellation.child(),
    };
    let model_request = ModelRequest {
        call: ModelCallContext {
            run,
            request_id: pending.model_request_id,
        },
        draft: request.request.clone(),
        continuation_state: None,
    };
    let stream = model
        .request(model_request)
        .await
        .map_err(model_handle_error)?;
    let assembled = ModelStreamAssembler::new(ModelStreamLimits::default())
        .map_err(model_handle_error)?
        .assemble(stream)
        .await
        .map_err(model_handle_error)?;
    let ModelTerminal::Completed(response) = assembled.terminal else {
        return Err(stage_error(COMPACTION_PHASE_UNAVAILABLE));
    };
    let resume = CompactionModelResume {
        request_id: pending.model_request_id,
        effect_id: pending.requested.effect_id(),
        result: response.clone(),
        resume_state: request.resume_state.clone(),
    };
    submit_compaction_settlement(coordinator, sources, &pending, response).await?;
    Ok(resume)
}

async fn submit_compaction_settlement<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    pending: &finstack_ai_kernel::PendingModelEffect,
    response: ModelResponse,
) -> Result<(), RunHandleError> {
    let output_bytes = serde_json_canonicalizer::to_vec(&response).map_err(|_| {
        RunHandleError::ModelSettlement {
            code: "model_response_serialize_failed",
        }
    })?;
    let output = finstack_ai_kernel::RawJson::parse(output_bytes).map_err(|_| {
        RunHandleError::ModelSettlement {
            code: "model_response_output_invalid",
        }
    })?;
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        pending.requested.output_contract().clone(),
        output,
        Some(response.usage.clone()),
        Vec::new(),
        response.provider_ids.clone(),
        Some(response.completion_id.as_ref()),
        None,
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_effect_completion_invalid",
    })?;
    let message_id = sources.generate::<MessageTag>()?;
    let now = sources.now()?;
    let model_ref = ModelRef::try_new("compaction", "summary").map_err(|_| {
        RunHandleError::ModelSettlement {
            code: "model_reference_invalid",
        }
    })?;
    let assistant_message = Message::try_new(
        message_id,
        MessageRole::Assistant,
        response.assistant_content.to_vec(),
        now,
        Some(model_ref),
        response.provider_ids,
        Metadata::empty(),
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_assistant_message_invalid",
    })?;
    let env = compaction_settlement_env(sources)?;
    coordinator
        .submit(
            env,
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message,
                },
            }),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    Ok(())
}

fn compaction_request_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<TransitionEnv, RunHandleError> {
    Ok(TransitionEnv {
        now: sources.now()?,
        ids: AllocatedIds::try_new(
            vec![sources.generate::<RecordTag>()?],
            vec![sources.generate::<EventTag>()?],
            vec![sources.generate::<EffectTag>()?],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sources.generate::<AppendBatchTag>()?],
            Vec::new(),
        )
        .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?,
    })
}

fn compaction_settlement_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<TransitionEnv, RunHandleError> {
    Ok(TransitionEnv {
        now: sources.now()?,
        ids: AllocatedIds::try_new(
            vec![sources.generate::<RecordTag>()?],
            vec![sources.generate::<EventTag>()?],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sources.generate::<AppendBatchTag>()?],
            Vec::new(),
        )
        .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?,
    })
}

pub(crate) fn first_compaction_request(
    outcomes: &[StageOutcome],
) -> Option<&CompactionModelRequest> {
    outcomes.iter().find_map(|outcome| match outcome {
        StageOutcome::RequestCompactionModel(request) => Some(request.as_ref()),
        _ => None,
    })
}

fn model_handle_error(error: crate::ModelError) -> RunHandleError {
    RunHandleError::Middleware {
        code: Arc::from(error.code()),
    }
}

fn stage_error(code: &'static str) -> RunHandleError {
    RunHandleError::Middleware {
        code: Arc::from(code),
    }
}

/// Reconstruct a resume from a completed compaction-summary effect in the journal.
///
/// Used when the child completed but BeforeModel has not settled yet, so the
/// chain can re-enter without a second model call.
pub(crate) async fn load_completed_compaction_resume(
    coordinator: &CommitCoordinator,
    cycle: u64,
) -> Result<Option<CompactionModelResume>, RunHandleError> {
    let Some(session_id) = coordinator.state().session_id else {
        return Ok(None);
    };
    let Some(seed) = coordinator.stage_dispatch_seed() else {
        return Ok(None);
    };
    let parent = derived_stage_effect_id(&seed.locator, cycle, Stage::BeforeModel);
    let loaded = coordinator
        .journal_store()
        .load(LoadRequest { session_id })
        .await
        .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?;
    let mut completed_by_effect = std::collections::BTreeMap::new();
    for batch in loaded.committed_batches.iter() {
        for record in batch.records.iter() {
            match record.body() {
                RecordBody::EffectCompleted(completed) => {
                    completed_by_effect.insert(completed.effect_id(), completed.clone());
                }
                RecordBody::EffectRequested(requested)
                    if requested.is_compaction_summary()
                        && requested.kind() == EffectKind::Model
                        && requested
                            .relation()
                            .is_some_and(|relation| relation.parent_effect_id == parent)
                        && coordinator
                            .state()
                            .model_settlements
                            .contains_key(&requested.effect_id()) =>
                {
                    if let Some(completed) = completed_by_effect.get(&requested.effect_id()) {
                        let result: ModelResponse =
                            serde_json::from_slice(completed.output().as_bytes())
                                .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?;
                        return Ok(Some(CompactionModelResume {
                            request_id: ModelRequestId::from_bytes(
                                *requested.effect_id().as_bytes(),
                            ),
                            effect_id: requested.effect_id(),
                            result,
                            resume_state: finstack_ai_kernel::RawJson::parse(b"{}")
                                .map_err(|_| stage_error(COMPACTION_PHASE_UNAVAILABLE))?,
                        }));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(None)
}

/// Parent linkage recorded on the child effect.
pub(crate) fn compaction_parent_effect_id(
    locator: &finstack_ai_kernel::OperationLocator,
    cycle: u64,
) -> EffectId {
    derived_stage_effect_id(locator, cycle, Stage::BeforeModel)
}
