use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, AppendBatchTag, AuthorizationEvidence, ContentBlock, Digest,
    EffectCompleted, EffectDeferred, EffectFailed, EffectId, EffectInput, ErrorCategory, EventId,
    EventTag, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    ExternalEffectCompletedInput, ExternalEffectCompletion, ExternalEffectOutcome, KernelInput,
    Message, MessageId, MessageRole, MessageTag, Metadata, ModelRef, ModelSettled, ModelSettlement,
    RawJson, RecordExternalCommandRejected, RecordId, RecordTag, RunPhase, ToolCallBlock,
    ToolCallId, ToolCallTag, TransitionEnv,
};

use crate::coordinator::{CommitCoordinator, CommitCoordinatorError, ModelDispatchSeed};
use crate::ids::{Clock, RandomSource};
use crate::ports::model::{
    CancellationSignal, Model, ModelDeferral, ModelError, ModelProgress, ModelReconcileResult,
    ModelRequestDraft, ModelResponse, ModelResumeAction, ModelTerminal, ReconcileContext,
    RunCallContext, map_model_reconcile_result, model_resume_action, model_retry_allowed,
};
use crate::run_types::RunHandleError;

use super::cancel::reconcile_cancelled_effect;
use super::ids::submit_resume_input;
use super::{ModelDriverResult, SettlementSources, model_handle_error};

pub(crate) async fn resume_pending_model_effect<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<ModelResumeAction, RunHandleError> {
    if coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .is_some_and(|pending| pending.requested.is_compaction_summary())
    {
        return Ok(ModelResumeAction::NoOutstanding);
    }
    let first_pass = model_resume_action(coordinator.state());
    match first_pass {
        ModelResumeAction::NoOutstanding
        | ModelResumeAction::UseRecorded
        | ModelResumeAction::WaitExternal => return Ok(first_pass),
        ModelResumeAction::Retry | ModelResumeAction::SuspendUncertain => {
            return Ok(first_pass);
        }
        ModelResumeAction::Reconcile => {}
    }
    let Some(seed) = coordinator.pending_model_seed() else {
        return Err(RunHandleError::ModelSettlement {
            code: "model_resume_seed_missing",
        });
    };
    let draft = pending_draft(&seed)?;
    let retry_allowed =
        model_retry_allowed(&seed.pending.requested, &model.capabilities(&draft.model));
    let context = ReconcileContext {
        run: RunCallContext {
            locator: seed.locator.clone(),
            authorization: seed.authorization.clone(),
            effect_id: seed.pending.requested.effect_id(),
            attempt: seed.attempt,
            deadline: seed.pending.requested.deadline(),
            budget_scope_id: seed.budget_scope_id,
            cancellation: cancellation.child(),
            relation_depth: coordinator.accepted_relation_depth(),
        },
        original_input_digest: seed.pending.requested.input_digest(),
    };
    let Ok(result) = model.reconcile(context, seed.pending.clone()).await else {
        return Ok(ModelResumeAction::SuspendUncertain);
    };
    Box::pin(apply_model_reconcile_result(
        coordinator,
        model,
        seed,
        draft,
        result,
        retry_allowed,
        sources,
    ))
    .await
}
async fn apply_model_reconcile_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    result: ModelReconcileResult,
    retry_allowed: bool,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    let action = map_model_reconcile_result(coordinator.state(), &result, retry_allowed);
    match result {
        ModelReconcileResult::Completed(response) => {
            settle_reconciled_completion(coordinator, model, seed, draft, response, sources).await
        }
        ModelReconcileResult::Deferred(deferral) | ModelReconcileResult::StillRunning(deferral) => {
            ensure_or_wait_deferred(coordinator, model, seed, draft, deferral, sources).await
        }
        ModelReconcileResult::NotStarted
        | ModelReconcileResult::RetrySafe
        | ModelReconcileResult::Unknown
        | ModelReconcileResult::NonRepeatable => Ok(action),
    }
}

async fn settle_reconciled_completion<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    response: ModelResponse,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    if coordinator
        .state()
        .model_settlements
        .contains_key(&seed.pending.requested.effect_id())
    {
        return Ok(ModelResumeAction::UseRecorded);
    }
    if coordinator.state().phase == Some(RunPhase::AwaitingExternal) {
        return settle_external_model(coordinator, model, seed, draft, response, sources).await;
    }
    let driver = ModelDriverResult {
        seed,
        draft,
        provider: model.descriptor().provider,
        result: Ok(ModelTerminal::Completed(response)),
    };
    process_model_result(coordinator, driver, sources).await?;
    Ok(ModelResumeAction::UseRecorded)
}

async fn ensure_or_wait_deferred<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    deferral: ModelDeferral,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    if let Some(existing) = seed.pending.deferred.as_ref() {
        if existing.handle == deferral.handle {
            return Ok(ModelResumeAction::WaitExternal);
        }
        return submit_fail_closed(
            coordinator,
            &seed,
            deferral.handle.handle(),
            "conflicting_or_invalid_completion",
            sources,
        )
        .await;
    }
    let driver = ModelDriverResult {
        seed,
        draft,
        provider: model.descriptor().provider,
        result: Ok(ModelTerminal::Deferred(deferral)),
    };
    process_model_result(coordinator, driver, sources).await?;
    Ok(ModelResumeAction::WaitExternal)
}

async fn settle_external_model<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    model: &dyn Model,
    seed: ModelDispatchSeed,
    draft: ModelRequestDraft,
    response: ModelResponse,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    let locator = seed.locator.clone();
    let driver = ModelDriverResult {
        seed: seed.clone(),
        draft,
        provider: model.descriptor().provider,
        result: Ok(ModelTerminal::Completed(response.clone())),
    };
    let now = sources.now()?;
    let allocation = allocate_settlement(&driver, sources)?;
    let settled = build_settlement(driver, now, &allocation)?;
    let ModelSettlement::Completed {
        completion,
        assistant_message,
    } = settled.outcome
    else {
        return Err(RunHandleError::ModelSettlement {
            code: "model_external_completion_invalid",
        });
    };
    let completion_id = completion
        .completion_id()
        .ok_or(RunHandleError::ModelSettlement {
            code: "model_completion_id_missing",
        })?
        .to_owned();
    let input = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            seed.pending.requested.effect_id(),
            &completion_id,
            ExternalEffectOutcome::Completed {
                output: completion.output().clone(),
                usage: completion.usage().cloned(),
                artifacts: Arc::from(completion.artifacts()),
            },
        )
        .map_err(|_| RunHandleError::ModelSettlement {
            code: "model_external_completion_invalid",
        })?,
        assistant_message: Some(assistant_message),
    });
    match coordinator
        .submit(
            TransitionEnv {
                now,
                ids: allocation.ids,
            },
            input,
        )
        .await
    {
        Ok(outcome) => {
            if let Some(fault) = outcome.fault {
                return Err(RunHandleError::Faulted { code: fault.code });
            }
            sources
                .pin_committed_artifacts(&locator, completion.artifacts())
                .await?;
            Ok(ModelResumeAction::UseRecorded)
        }
        Err(CommitCoordinatorError::Decision {
            code:
                "conflicting_completion_id" | "conflicting_settlement" | "settlement_digest_mismatch",
        }) => {
            submit_fail_closed(
                coordinator,
                &seed,
                &completion_id,
                "conflicting_or_invalid_completion",
                sources,
            )
            .await
        }
        Err(error) => Err(RunHandleError::Coordinator(error)),
    }
}

async fn submit_fail_closed<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: &ModelDispatchSeed,
    command_id: &str,
    reason_code: &'static str,
    sources: &SettlementSources<C, R>,
) -> Result<ModelResumeAction, RunHandleError> {
    let accepted =
        coordinator
            .state()
            .accepted
            .as_ref()
            .ok_or(RunHandleError::ModelSettlement {
                code: "model_resume_accepted_missing",
            })?;
    let security = accepted.security();
    let effect_id = seed.pending.requested.effect_id();
    let accepted_digest = coordinator
        .state()
        .model_settlements
        .get(&effect_id)
        .map(|fingerprint| fingerprint.digest);
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        command_id,
        ExternalCommandTarget::Effect(effect_id),
        seed.authorization.principal.clone(),
        AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .map_err(|_| RunHandleError::ModelSettlement {
            code: "model_resume_authorization_invalid",
        })?,
        reason_code,
        Digest::raw_json(command_id.as_bytes()),
        accepted_digest,
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_resume_rejection_invalid",
    })?;
    let input = KernelInput::RecordExternalCommandRejected(RecordExternalCommandRejected {
        locator: seed.locator.clone(),
        rejection,
    });
    submit_resume_input(coordinator, input, sources).await?;
    Ok(ModelResumeAction::SuspendUncertain)
}
fn pending_draft(seed: &ModelDispatchSeed) -> Result<ModelRequestDraft, RunHandleError> {
    let EffectInput::Model { request } = seed.pending.requested.input() else {
        return Err(RunHandleError::ModelSettlement {
            code: "model_request_invalid",
        });
    };
    serde_json::from_slice(request.as_bytes()).map_err(|_| RunHandleError::ModelSettlement {
        code: "model_request_invalid",
    })
}

pub(crate) async fn process_model_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    mut driver_result: ModelDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let effect_id = driver_result.seed.pending.requested.effect_id();
    let state = coordinator.state();
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(());
    };
    if let Some(cancellation) = state.cancellation.as_ref()
        && cancellation.outstanding_effects.contains(&effect_id)
    {
        let cancelled = driver_result
            .result
            .as_ref()
            .is_err_and(|error| error.category() == ErrorCategory::Cancellation);
        return reconcile_cancelled_effect(coordinator, effect_id, cancelled, sources).await;
    }
    if pending.requested.effect_id() != effect_id
        || pending.model_request_id != driver_result.seed.pending.model_request_id
        || pending.deferred.is_some()
        || state.terminal.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    if pending
        .requested
        .deadline()
        .is_some_and(|deadline| deadline <= now)
    {
        driver_result.result = Err(ModelError::try_new(
            "model_deadline_exceeded",
            ErrorCategory::Deadline,
            false,
            "model result arrived after the committed deadline",
            Metadata::empty(),
        )
        .map_err(|error| model_handle_error(&ModelError::from(error)))?);
    }
    let locator = driver_result.seed.locator.clone();
    let allocation = allocate_settlement(&driver_result, sources)?;
    let settled = build_settlement(driver_result, now, &allocation)?;
    let artifacts = match &settled.outcome {
        ModelSettlement::Completed { completion, .. } => completion.artifacts().to_vec(),
        ModelSettlement::Failed(_) | ModelSettlement::Deferred(_) => Vec::new(),
    };
    let outcome = coordinator
        .submit(
            TransitionEnv {
                now,
                ids: allocation.ids,
            },
            KernelInput::ModelSettled(settled),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    sources
        .pin_committed_artifacts(&locator, &artifacts)
        .await?;
    Ok(())
}

pub(crate) async fn process_model_progress<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    provider: &str,
    progress: ModelProgress,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let state = coordinator.state();
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return Ok(());
    };
    if pending.requested.effect_id() != effect_id
        || pending.deferred.is_some()
        || state.terminal.is_some()
        || state.cancellation.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    let event_id = sources.generate_progress_event()?;
    let event = coordinator
        .materialize_model_progress(&progress, event_id, provider, now)
        .map_err(|code| RunHandleError::ModelSettlement { code })?;
    coordinator
        .publish_events(Arc::from([event]))
        .await
        .map_err(RunHandleError::Coordinator)
}

struct SettlementAllocation {
    ids: AllocatedIds,
    message_id: Option<MessageId>,
    tool_call_ids: Vec<ToolCallId>,
}

fn allocate_settlement<C: Clock, R: RandomSource>(
    result: &ModelDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<SettlementAllocation, RunHandleError> {
    let completed = matches!(result.result, Ok(ModelTerminal::Completed(_)));
    let tool_count = match &result.result {
        Ok(value) => match value {
            ModelTerminal::Completed(response) => response.tool_calls.len(),
            ModelTerminal::Deferred(_) => 0,
        },
        Err(_) => 0,
    };
    let record_count = if completed { 2 } else { 1 };
    let event_count = if completed { 2 } else { 1 };
    let records = (0..record_count)
        .map(|_| sources.generate::<RecordTag>())
        .collect::<Result<Vec<RecordId>, _>>()?;
    let events = (0..event_count)
        .map(|_| sources.generate::<EventTag>())
        .collect::<Result<Vec<EventId>, _>>()?;
    let message_id = completed
        .then(|| sources.generate::<MessageTag>())
        .transpose()?;
    let tool_call_ids = (0..tool_count)
        .map(|_| sources.generate::<ToolCallTag>())
        .collect::<Result<Vec<ToolCallId>, _>>()?;
    let append_batch_id: AppendBatchId = sources.generate::<AppendBatchTag>()?;
    let ids = AllocatedIds::try_new(
        records,
        events,
        Vec::new(),
        Vec::new(),
        message_id.into_iter().collect(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        tool_call_ids.clone(),
        vec![append_batch_id],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_settlement_ids_invalid",
    })?;
    Ok(SettlementAllocation {
        ids,
        message_id,
        tool_call_ids,
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "terminal conversion keeps response, deferral, and failure identity continuity visible"
)]
fn build_settlement(
    result: ModelDriverResult,
    now: finstack_ai_kernel::Timestamp,
    allocation: &SettlementAllocation,
) -> Result<ModelSettled, RunHandleError> {
    let requested = &result.seed.pending.requested;
    let effect_id = requested.effect_id();
    let outcome = match result.result {
        Ok(terminal) => match terminal {
            ModelTerminal::Completed(response) => {
                let output_bytes = serde_json_canonicalizer::to_vec(&response).map_err(|_| {
                    RunHandleError::ModelSettlement {
                        code: "model_response_serialize_failed",
                    }
                })?;
                let output =
                    RawJson::parse(output_bytes).map_err(|_| RunHandleError::ModelSettlement {
                        code: "model_response_output_invalid",
                    })?;
                let completion = EffectCompleted::try_new(
                    effect_id,
                    requested.output_contract().clone(),
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
                let mut content = response.assistant_content.to_vec();
                if allocation.tool_call_ids.len() != response.tool_calls.len() {
                    return Err(RunHandleError::ModelSettlement {
                        code: "model_tool_call_id_cardinality",
                    });
                }
                for (call, tool_call_id) in
                    response.tool_calls.iter().zip(&allocation.tool_call_ids)
                {
                    content.push(ContentBlock::ToolCall(
                        ToolCallBlock::try_new_with_provider_call_id(
                            *tool_call_id,
                            &call.name,
                            call.arguments.clone(),
                            call.provider_call_id.as_deref(),
                        )
                        .map_err(|_| RunHandleError::ModelSettlement {
                            code: "model_tool_call_invalid",
                        })?,
                    ));
                }
                let model_ref = ModelRef::try_new(&result.provider, result.draft.model.as_str())
                    .map_err(|_| RunHandleError::ModelSettlement {
                        code: "model_reference_invalid",
                    })?;
                let message_id = allocation
                    .message_id
                    .ok_or(RunHandleError::ModelSettlement {
                        code: "model_message_id_missing",
                    })?;
                let assistant_message = Message::try_new(
                    message_id,
                    MessageRole::Assistant,
                    content,
                    now,
                    Some(model_ref),
                    response.provider_ids,
                    Metadata::empty(),
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_assistant_message_invalid",
                })?;
                ModelSettlement::Completed {
                    completion,
                    assistant_message,
                }
            }
            ModelTerminal::Deferred(deferral) => ModelSettlement::Deferred(EffectDeferred {
                effect_id,
                handle: deferral.handle,
                reconciliation: deferral.reconciliation,
                next_poll_at: deferral.next_poll_at,
                expires_at: deferral.expires_at,
                output_contract: requested.output_contract().clone(),
            }),
        },
        Err(error) => {
            let descriptor = error
                .to_descriptor()
                .map_err(|error| model_handle_error(&error))?;
            ModelSettlement::Failed(
                EffectFailed::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    descriptor,
                    None,
                    None::<&str>,
                )
                .map_err(|_| RunHandleError::ModelSettlement {
                    code: "model_effect_failure_invalid",
                })?,
            )
        }
    };
    Ok(ModelSettled {
        turn_id: result.seed.pending.turn_id,
        model_request_id: result.seed.pending.model_request_id,
        outcome,
    })
}
