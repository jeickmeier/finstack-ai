use std::sync::Arc;

use finstack_ai_kernel::{
    ActiveToolCallStatus, AllocatedIds, AppendBatchTag, AuthorizationEvidence, Digest,
    EffectCompleted, EffectDeferred, EffectFailed, EffectId, ErrorCategory, EventTag,
    ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    ExternalEffectCompletedInput, ExternalEffectCompletion, ExternalEffectOutcome, Id, IdTag,
    KernelInput, MessageTag, Metadata, ProviderIds, RawJson, RecordExternalCommandRejected,
    RecordTag, ToolBatchSettled, ToolCallPlan, ToolFailurePolicy, ToolSettlement, TransitionEnv,
};

use crate::coordinator::{CommitCoordinator, CommitCoordinatorError, ToolDispatchSeed};
use crate::run_types::RunHandleError;
use crate::tool::AssembledToolTerminal;
use crate::{
    CancellationSignal, Clock, PendingToolEffect, RandomSource, ReconcileContext,
    ResolvedToolCatalog, RunCallContext, ToolDeferral, ToolError, ToolProgress,
    ToolReconcileResult, ToolResult, ToolResumeAction, map_tool_reconcile_result,
    normalize_tool_result, tool_resume_action, tool_retry_allowed,
};

use super::cancel::reconcile_cancelled_effect;
use super::ids::submit_resume_input;
use super::{SettlementSources, ToolDriverResult, tool_handle_error};

pub(crate) async fn process_tool_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    mut driver_result: ToolDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let effect_id = driver_result.seed.requested.effect_id();
    let state = coordinator.state();
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(());
    };
    let Some(active) = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
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
    if batch.opened.tool_batch_id != driver_result.seed.tool_batch_id
        || !matches!(
            active.status,
            finstack_ai_kernel::ActiveToolCallStatus::Requested { deferred: None, .. }
        )
        || state.terminal.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    if driver_result
        .seed
        .requested
        .deadline()
        .is_some_and(|deadline| deadline <= now)
    {
        driver_result.result = Err(ToolError::try_new(
            crate::TOOL_DEADLINE_EXCEEDED,
            ErrorCategory::Deadline,
            false,
            "tool result arrived after the committed deadline",
            Metadata::empty(),
        )
        .map_err(|error| tool_handle_error(&ToolError::from(error)))?);
    }
    let settled = build_tool_settlement(driver_result)?;
    let input = KernelInput::ToolBatchSettled(settled.clone());
    let allocation = allocate_tool_settlement(coordinator.state(), &settled, sources)?;
    let env = TransitionEnv {
        now,
        ids: allocation,
    };
    coordinator
        .classify(&env, input.clone())
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_settlement_allocation_mismatch",
        })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

pub(crate) async fn process_tool_progress<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    progress: ToolProgress,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let state = coordinator.state();
    let Some(batch) = state.active_tool_batch.as_ref() else {
        return Ok(());
    };
    let Some(active) = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)
    else {
        return Ok(());
    };
    if !matches!(
        active.status,
        finstack_ai_kernel::ActiveToolCallStatus::Requested { deferred: None, .. }
    ) || state.terminal.is_some()
        || state.cancellation.is_some()
    {
        return Ok(());
    }
    let now = sources.now()?;
    let event_id = sources.generate_progress_event()?;
    let event = coordinator
        .materialize_tool_progress(&progress, event_id, effect_id, now)
        .map_err(|code| RunHandleError::ToolSettlement { code })?;
    coordinator
        .publish_events(Arc::from([event]))
        .await
        .map_err(RunHandleError::Coordinator)
}

fn build_tool_settlement(result: ToolDriverResult) -> Result<ToolBatchSettled, RunHandleError> {
    let requested = &result.seed.requested;
    let effect_id = requested.effect_id();
    let outcome = match result.result {
        Ok(assembled) => {
            let block = normalize_tool_result(result.seed.tool_call_id, assembled.result)
                .map_err(|error| tool_handle_error(&error))?;
            let bytes = serde_json_canonicalizer::to_vec(&block).map_err(|_| {
                RunHandleError::ToolSettlement {
                    code: "tool_result_serialize_failed",
                }
            })?;
            let output = RawJson::parse(bytes).map_err(|_| RunHandleError::ToolSettlement {
                code: "tool_result_output_invalid",
            })?;
            ToolSettlement::Completed(
                EffectCompleted::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    output,
                    assembled.usage,
                    Vec::new(),
                    ProviderIds::empty(),
                    None::<&str>,
                    None,
                )
                .map_err(|_| RunHandleError::ToolSettlement {
                    code: "tool_effect_completion_invalid",
                })?,
            )
        }
        Err(error) => {
            let descriptor = error
                .to_descriptor()
                .map_err(|error| tool_handle_error(&error))?;
            ToolSettlement::Failed(
                EffectFailed::try_new(
                    effect_id,
                    requested.output_contract().clone(),
                    descriptor,
                    None,
                    None::<&str>,
                )
                .map_err(|_| RunHandleError::ToolSettlement {
                    code: "tool_effect_failure_invalid",
                })?,
            )
        }
    };
    Ok(ToolBatchSettled {
        tool_batch_id: result.seed.tool_batch_id,
        outcome,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredictedToolStatus {
    Undispatched,
    Requested,
    Buffered,
    Settled,
}

#[expect(
    clippy::too_many_lines,
    reason = "exact allocation mirrors the kernel's contiguous-prefix and next-group cardinalities"
)]
fn allocate_tool_settlement<C: Clock, R: RandomSource>(
    state: &finstack_ai_kernel::KernelState,
    settled: &ToolBatchSettled,
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let batch = state
        .active_tool_batch
        .as_ref()
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_settlement_batch_missing",
        })?;
    let effect_id = match &settled.outcome {
        ToolSettlement::Completed(value) => value.effect_id(),
        ToolSettlement::Failed(value) => value.effect_id(),
        ToolSettlement::Deferred(value) => value.effect_id,
    };
    let target = batch
        .calls
        .iter()
        .position(|call| call.assigned.effect_id == effect_id)
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_settlement_call_missing",
        })?;
    let mut statuses = batch
        .calls
        .iter()
        .map(|call| match call.status {
            finstack_ai_kernel::ActiveToolCallStatus::Undispatched => {
                PredictedToolStatus::Undispatched
            }
            finstack_ai_kernel::ActiveToolCallStatus::Requested { .. } => {
                PredictedToolStatus::Requested
            }
            finstack_ai_kernel::ActiveToolCallStatus::Buffered { .. } => {
                PredictedToolStatus::Buffered
            }
            finstack_ai_kernel::ActiveToolCallStatus::Settled { .. } => {
                PredictedToolStatus::Settled
            }
        })
        .collect::<Vec<_>>();
    statuses[target] = PredictedToolStatus::Buffered;
    let target_plan = &batch.calls[target].assigned.plan;
    let fatal = batch.fatal_error.is_some()
        || (matches!(settled.outcome, ToolSettlement::Failed(_))
            && target_plan.failure_policy() == ToolFailurePolicy::FailRun);
    let current_complete = batch.calls.iter().enumerate().all(|(index, call)| {
        call.assigned.group_index != batch.current_group
            || matches!(
                statuses[index],
                PredictedToolStatus::Buffered | PredictedToolStatus::Settled
            )
    });
    if fatal && current_complete {
        for status in &mut statuses {
            if *status == PredictedToolStatus::Undispatched {
                *status = PredictedToolStatus::Buffered;
            }
        }
    }
    let mut messages = 0_usize;
    let start =
        usize::try_from(batch.next_source_index).map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_source_index_invalid",
        })?;
    for status in statuses.iter_mut().skip(start) {
        if *status != PredictedToolStatus::Buffered {
            break;
        }
        *status = PredictedToolStatus::Settled;
        messages += 1;
    }
    let requests = if !fatal && current_complete {
        let next_group = batch.calls.iter().enumerate().find_map(|(index, call)| {
            (statuses[index] == PredictedToolStatus::Undispatched
                && matches!(call.assigned.plan, ToolCallPlan::Execute(_)))
            .then_some(call.assigned.group_index)
        });
        next_group.map_or(0, |group| {
            batch
                .calls
                .iter()
                .enumerate()
                .filter(|(index, call)| {
                    statuses[*index] == PredictedToolStatus::Undispatched
                        && call.assigned.group_index == group
                        && matches!(call.assigned.plan, ToolCallPlan::Execute(_))
                })
                .count()
        })
    } else {
        0
    };
    let close = usize::from(
        statuses
            .iter()
            .all(|status| *status == PredictedToolStatus::Settled),
    );
    let records = 1 + messages + requests + close;
    let events = 1 + 2 * messages + requests;
    AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(records, sources)?,
        generate_tool_ids::<EventTag, _, _>(events, sources)?,
        Vec::new(),
        Vec::new(),
        generate_tool_ids::<MessageTag, _, _>(messages, sources)?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_settlement_ids_invalid",
    })
}

pub(super) fn generate_tool_ids<T: IdTag, C: Clock, R: RandomSource>(
    count: usize,
    sources: &SettlementSources<C, R>,
) -> Result<Vec<Id<T>>, RunHandleError> {
    (0..count)
        .map(|_| generate_tool_id::<T, _, _>(sources))
        .collect()
}

pub(super) fn generate_tool_id<T: IdTag, C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<Id<T>, RunHandleError> {
    sources
        .generate::<T>()
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_settlement_id_source_failed",
        })
}

pub(crate) async fn resume_pending_tool_effects<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<ToolResumeAction, RunHandleError> {
    let Some(batch) = coordinator.state().active_tool_batch.clone() else {
        return Ok(ToolResumeAction::NoOutstanding);
    };
    let mut first_pass = Vec::new();
    let mut to_reconcile = Vec::new();
    for call in batch.calls.iter() {
        let effect_id = call.assigned.effect_id;
        let action = tool_resume_action(coordinator.state(), effect_id);
        first_pass.push(action);
        if action == ToolResumeAction::Reconcile {
            to_reconcile.push(effect_id);
        }
    }
    if to_reconcile.is_empty() {
        return Ok(aggregate_tool_resume_actions(&first_pass));
    }
    let seeds = coordinator.pending_tool_seeds();
    let mut mapped = Vec::new();
    for effect_id in to_reconcile {
        let Some(seed) = seeds
            .iter()
            .find(|seed| seed.requested.effect_id() == effect_id)
            .cloned()
            .or_else(|| tool_seed_for_deferred(coordinator, effect_id))
        else {
            return Ok(ToolResumeAction::SuspendUncertain);
        };
        let Some(resolved) = catalog.by_id(&seed.call.tool_id) else {
            return Ok(ToolResumeAction::SuspendUncertain);
        };
        let retry_allowed = tool_retry_allowed(&seed.requested, &resolved.spec);
        let context = ReconcileContext {
            run: RunCallContext {
                locator: seed.locator.clone(),
                authorization: seed.authorization.clone(),
                effect_id,
                attempt: seed.attempt,
                deadline: seed.requested.deadline(),
                budget_scope_id: seed.budget_scope_id,
                cancellation: cancellation.child(),
            },
            original_input_digest: seed.requested.input_digest(),
        };
        let Ok(result) = resolved
            .toolset
            .reconcile(
                context,
                PendingToolEffect {
                    call: seed.call.clone(),
                },
            )
            .await
        else {
            return Ok(ToolResumeAction::SuspendUncertain);
        };
        let action =
            map_tool_reconcile_result(coordinator.state(), effect_id, &result, retry_allowed);
        mapped.push((seed, result, action));
    }
    if mapped
        .iter()
        .any(|(_, _, action)| *action == ToolResumeAction::SuspendUncertain)
    {
        return Ok(ToolResumeAction::SuspendUncertain);
    }
    let mut applied = Vec::new();
    for (seed, result, mapped_action) in &mapped {
        let action = apply_tool_reconcile_result(coordinator, seed, result, sources).await?;
        if action == ToolResumeAction::SuspendUncertain {
            return Ok(ToolResumeAction::SuspendUncertain);
        }
        applied.push(*mapped_action);
    }
    let mut actions = first_pass
        .into_iter()
        .filter(|action| *action != ToolResumeAction::Reconcile)
        .collect::<Vec<_>>();
    actions.extend(applied);
    Ok(aggregate_tool_resume_actions(&actions))
}

fn aggregate_tool_resume_actions(actions: &[ToolResumeAction]) -> ToolResumeAction {
    if actions.contains(&ToolResumeAction::SuspendUncertain) {
        return ToolResumeAction::SuspendUncertain;
    }
    if actions.contains(&ToolResumeAction::Retry) {
        return ToolResumeAction::Retry;
    }
    if actions.contains(&ToolResumeAction::WaitExternal) {
        return ToolResumeAction::WaitExternal;
    }
    if actions.contains(&ToolResumeAction::UseRecorded) {
        return ToolResumeAction::UseRecorded;
    }
    if actions.contains(&ToolResumeAction::Reconcile) {
        return ToolResumeAction::Reconcile;
    }
    ToolResumeAction::NoOutstanding
}

fn tool_seed_for_deferred(
    coordinator: &CommitCoordinator,
    effect_id: EffectId,
) -> Option<ToolDispatchSeed> {
    coordinator
        .pending_tool_seeds()
        .into_iter()
        .find(|seed| seed.requested.effect_id() == effect_id)
        .or_else(|| deferred_tool_seed(coordinator, effect_id))
}

fn deferred_tool_seed(
    coordinator: &CommitCoordinator,
    effect_id: EffectId,
) -> Option<ToolDispatchSeed> {
    let state = coordinator.state();
    let batch = state.active_tool_batch.as_ref()?;
    let call = batch
        .calls
        .iter()
        .find(|call| call.assigned.effect_id == effect_id)?;
    let ActiveToolCallStatus::Requested { requested, .. } = &call.status else {
        return None;
    };
    let ToolCallPlan::Execute(validated) = &call.assigned.plan else {
        return None;
    };
    let (locator, authorization, budget_scope_id) = dispatch_security_from_state(state)?;
    Some(ToolDispatchSeed {
        requested: requested.clone(),
        tool_batch_id: batch.opened.tool_batch_id,
        tool_call_id: *validated.call.tool_call_id(),
        call: validated.clone(),
        locator,
        authorization,
        budget_scope_id,
        attempt: 1,
        requested_at: state.accepted_at?,
    })
}

fn dispatch_security_from_state(
    state: &finstack_ai_kernel::KernelState,
) -> Option<(
    finstack_ai_kernel::OperationLocator,
    crate::AuthorizationContext,
    Option<finstack_ai_kernel::BudgetScopeId>,
)> {
    let accepted = state.accepted.as_ref()?;
    let security = accepted.security();
    let locator = finstack_ai_kernel::OperationLocator::try_new(
        security.tenant_scope(),
        state.session_id?,
        state.lane_id?,
        accepted.run_id(),
    )
    .ok()?;
    Some((
        locator,
        crate::AuthorizationContext {
            principal: security.principal().clone(),
            authentication_method: Arc::from(security.authentication_method()),
            assurance_level: Arc::from(security.assurance_level()),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([Arc::from(security.tenant_scope())]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from(security.authorization_policy_version()),
            decision_id: Arc::from(security.authorization_decision_id()),
        },
        accepted.relation().budget_scope_id(),
    ))
}

async fn apply_tool_reconcile_result<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: &ToolDispatchSeed,
    result: &ToolReconcileResult,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    match result {
        ToolReconcileResult::Completed(tool_result) => {
            settle_reconciled_tool(coordinator, seed.clone(), tool_result.clone(), sources).await
        }
        ToolReconcileResult::Deferred(deferral) | ToolReconcileResult::StillRunning(deferral) => {
            ensure_or_wait_tool_deferred(coordinator, seed, deferral, sources).await
        }
        ToolReconcileResult::NotStarted
        | ToolReconcileResult::RetrySafe
        | ToolReconcileResult::Unknown
        | ToolReconcileResult::NonRepeatable => Ok(ToolResumeAction::NoOutstanding),
    }
}

async fn settle_reconciled_tool<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: ToolDispatchSeed,
    result: ToolResult,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    let effect_id = seed.requested.effect_id();
    if coordinator
        .state()
        .tool_settlements
        .contains_key(&effect_id)
    {
        return Ok(ToolResumeAction::UseRecorded);
    }
    let deferred = coordinator
        .state()
        .active_tool_batch
        .as_ref()
        .is_some_and(|batch| {
            batch.calls.iter().any(|call| {
                call.assigned.effect_id == effect_id
                    && matches!(
                        call.status,
                        ActiveToolCallStatus::Requested {
                            deferred: Some(_),
                            ..
                        }
                    )
            })
        });
    let driver = ToolDriverResult {
        seed,
        result: Ok(AssembledToolTerminal {
            usage: None,
            result,
        }),
    };
    if deferred {
        return settle_external_tool(coordinator, driver, sources).await;
    }
    process_tool_result(coordinator, driver, sources).await?;
    Ok(ToolResumeAction::UseRecorded)
}

async fn settle_external_tool<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver: ToolDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    let effect_id = driver.seed.requested.effect_id();
    let settled = build_tool_settlement(driver)?;
    let ToolSettlement::Completed(completion) = &settled.outcome else {
        return Err(RunHandleError::ToolSettlement {
            code: "tool_external_completion_invalid",
        });
    };
    let completion_id = effect_id.to_canonical_string();
    let input = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            effect_id,
            &completion_id,
            ExternalEffectOutcome::Completed {
                output: completion.output().clone(),
                usage: completion.usage().cloned(),
                artifacts: Arc::from(completion.artifacts()),
            },
        )
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_external_completion_invalid",
        })?,
        assistant_message: None,
    });
    let now = sources.now()?;
    let allocation = allocate_tool_settlement(coordinator.state(), &settled, sources)?;
    match coordinator
        .submit(
            TransitionEnv {
                now,
                ids: allocation,
            },
            input,
        )
        .await
    {
        Ok(outcome) => {
            if let Some(fault) = outcome.fault {
                return Err(RunHandleError::Faulted { code: fault.code });
            }
            Ok(ToolResumeAction::UseRecorded)
        }
        Err(CommitCoordinatorError::Decision {
            code:
                "conflicting_completion_id" | "conflicting_settlement" | "settlement_digest_mismatch",
        }) => {
            submit_tool_fail_closed(coordinator, effect_id, &completion_id, sources).await?;
            Ok(ToolResumeAction::SuspendUncertain)
        }
        Err(error) => Err(RunHandleError::Coordinator(error)),
    }
}

async fn ensure_or_wait_tool_deferred<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    seed: &ToolDispatchSeed,
    deferral: &ToolDeferral,
    sources: &SettlementSources<C, R>,
) -> Result<ToolResumeAction, RunHandleError> {
    let effect_id = seed.requested.effect_id();
    let existing = coordinator
        .state()
        .active_tool_batch
        .as_ref()
        .and_then(|batch| {
            batch.calls.iter().find_map(|call| {
                (call.assigned.effect_id == effect_id)
                    .then_some(call.status.clone())
                    .and_then(|status| match status {
                        ActiveToolCallStatus::Requested {
                            deferred: Some(deferred),
                            ..
                        } => Some(deferred),
                        _ => None,
                    })
            })
        });
    if let Some(existing) = existing {
        if existing.handle == deferral.handle {
            return Ok(ToolResumeAction::WaitExternal);
        }
        submit_tool_fail_closed(coordinator, effect_id, deferral.handle.handle(), sources).await?;
        return Ok(ToolResumeAction::SuspendUncertain);
    }
    let settled = ToolBatchSettled {
        tool_batch_id: seed.tool_batch_id,
        outcome: ToolSettlement::Deferred(EffectDeferred {
            effect_id,
            handle: deferral.handle.clone(),
            reconciliation: deferral.reconciliation,
            next_poll_at: deferral.next_poll_at,
            expires_at: deferral.expires_at,
            output_contract: seed.requested.output_contract().clone(),
        }),
    };
    submit_resume_input(coordinator, KernelInput::ToolBatchSettled(settled), sources).await?;
    Ok(ToolResumeAction::WaitExternal)
}

async fn submit_tool_fail_closed<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    effect_id: EffectId,
    command_id: &str,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let accepted = coordinator
        .state()
        .accepted
        .as_ref()
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_resume_accepted_missing",
        })?;
    let security = accepted.security();
    let locator = coordinator
        .pending_tool_seeds()
        .into_iter()
        .find(|seed| seed.requested.effect_id() == effect_id)
        .or_else(|| deferred_tool_seed(coordinator, effect_id))
        .ok_or(RunHandleError::ToolSettlement {
            code: "tool_resume_seed_missing",
        })?
        .locator;
    let accepted_digest = coordinator
        .state()
        .tool_settlements
        .get(&effect_id)
        .map(|fingerprint| fingerprint.digest);
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        command_id,
        ExternalCommandTarget::Effect(effect_id),
        accepted.security().principal().clone(),
        AuthorizationEvidence::try_new(
            security.authorization_policy_version(),
            security.authorization_decision_id(),
        )
        .map_err(|_| RunHandleError::ToolSettlement {
            code: "tool_resume_authorization_invalid",
        })?,
        "conflicting_or_invalid_completion",
        Digest::raw_json(command_id.as_bytes()),
        accepted_digest,
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_resume_rejection_invalid",
    })?;
    submit_resume_input(
        coordinator,
        KernelInput::RecordExternalCommandRejected(RecordExternalCommandRejected {
            locator,
            rejection,
        }),
        sources,
    )
    .await
}
