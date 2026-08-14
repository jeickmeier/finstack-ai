//! Pure PR-009 transition decisions.

use std::sync::Arc;

use super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::capacity::{self, StateGrowth};
use super::decision::{Decision, KernelError, PostCommitAction};
use super::failure_from_state;
use super::fingerprint::{direct_digest, external_digest, stage_digest};
use super::input::{
    AcceptRun, CancelRequested, CancellationReconciledInput, ExternalEffectCompletedInput,
    ExternalEffectOutcome, KernelInput, ModelSettled, ModelSettlement, ReducerStageOutcome,
    StageSettled, TimerFiredInput,
};
use super::validation::{
    assistant_tool_calls, validate_assistant_message_id, validate_assistant_semantics,
    validate_assistant_tool_call_ids, validate_completion_identity, validate_error_descriptor,
};
use crate::bounds::SEMANTIC_ARRAY_MAX_ITEMS;
use crate::content::{ContentBlock, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::digest::Digest;
use crate::effects::{
    EffectCancelled, EffectCompleted, EffectFailed, EffectInput, EffectKind, EffectOutputKind,
    EffectRequested,
};
use crate::entries::{
    ContextPrepared, EntryAppended, RetryClassification, RetryScheduled, RunCancelled,
    RunCompleted, RunFailed, RunSuspended, Stage, StageCursor, StageDisposition,
    StageOutcomeRecorded, TimerFired,
};
use crate::error::ErrorCode;
use crate::error::{ErrorCategory, ErrorDescriptor};
use crate::limits::{LimitDimension, LimitReached, LimitUsage, LimitValue};
use crate::records::{
    APPEND_BATCH_MAX_RECORDS, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft,
};
use crate::refs::{Diagnostic, DiagnosticSeverity};
use crate::state::{KernelState, RunPhase, TerminalCandidate, TransitionEnv};
use crate::{
    CancellationInitiator, CancellationReconciled, CancellationRequest, CancellationRequested,
    CapabilitiesActivated, EffectOutputContract, FinalResultRecorded, OutputConfiguration,
    OutputEndStrategy, OutputSpec, OutputValidated, OutputValidationFailed, RetrySafety,
    SUBMIT_FINAL_OUTPUT_TOOL, StructuredResultSource,
};

pub(super) fn decide(
    state: &KernelState,
    env: &TransitionEnv,
    input: KernelInput,
) -> Result<Decision, KernelError> {
    if let KernelInput::RecordExternalCommandRejected(input) = &input {
        return decide_external_command_rejected(state, env, input);
    }
    if let KernelInput::CancelRequested(cancel) = &input
        && state.cancellation.is_none()
        && state.terminal.is_none()
        && let Some(accepted) = state.accepted.as_ref()
    {
        validate_cancel_authorization(accepted, env, cancel)?;
    }
    // The prepared context is the largest payload the kernel canonicalizes and
    // it grows with every turn, so it is canonicalized once here and reused by
    // limit accounting and by the record it produces.
    let context_canonical = match &input {
        KernelInput::StageSettled(StageSettled {
            outcome: ReducerStageOutcome::ContextPrepared { messages },
            ..
        }) => Some(
            crate::entries::context_digest_and_len(messages)
                .map_err(|_| KernelError::ContextDigestMismatch)?,
        ),
        _ => None,
    };
    if let Some(decision) = decide_limit(state, env, &input, context_canonical)? {
        return Ok(decision);
    }
    match input {
        KernelInput::AcceptRun(input) => decide_accept(state, env, &input),
        KernelInput::StageSettled(input) => decide_stage(state, env, &input, context_canonical),
        KernelInput::ModelSettled(input) => decide_model(state, env, &input),
        KernelInput::ExternalEffectCompleted(input) => decide_external(state, env, input),
        KernelInput::ToolBatchSettled(input) => {
            super::tool::decide_tool_settled(state, env, &input)
        }
        KernelInput::CancelRequested(input) => decide_cancel(state, env, &input),
        KernelInput::CancellationReconciled(input) => decide_reconciliation(state, env, &input),
        KernelInput::TimerFired(input) => decide_timer_fired(state, env, &input),
        KernelInput::ConfigureOutput(input) => decide_configure_output(state, env, input),
        KernelInput::CapabilitiesActivated(input) => {
            decide_capabilities_activated(state, env, input)
        }
        KernelInput::OutputValidated(input) => decide_output_validated(state, env, input),
        KernelInput::RecordExternalCommandRejected(_) => {
            unreachable!("external rejection returns before limit processing")
        }
        KernelInput::RequestInteraction(input) => {
            super::interaction::decide_request(state, env, &input)
        }
        KernelInput::InteractionSettled(input) => {
            super::interaction::decide_settled(state, env, &input)
        }
    }
}

fn decide_external_command_rejected(
    state: &KernelState,
    env: &TransitionEnv,
    input: &crate::RecordExternalCommandRejected,
) -> Result<Decision, KernelError> {
    let accepted = state
        .accepted
        .as_ref()
        .ok_or(KernelError::InvalidRunAcceptance)?;
    let security = accepted.security();
    if state.session_id != Some(input.locator.session_id)
        || state.lane_id != Some(input.locator.lane_id)
        || accepted.run_id() != input.locator.run_id
        || security.tenant_scope() != input.locator.tenant_scope.as_ref()
        || security.principal() != &input.rejection.principal
        || security.authorization_policy_version() != input.rejection.authorization.policy_version()
        || security.authorization_decision_id() != input.rejection.authorization.decision_id()
    {
        return Err(KernelError::InvalidRunAcceptance);
    }
    match input.rejection.target {
        crate::ExternalCommandTarget::Effect(effect_id) if !known_effect(state, effect_id) => {
            return Err(KernelError::EffectNotPending { effect_id });
        }
        crate::ExternalCommandTarget::Interaction(interaction_id)
            if state
                .pending_interaction
                .as_ref()
                .is_none_or(|pending| pending.request.interaction_id() != interaction_id)
                && state
                    .last_interaction_terminal
                    .as_ref()
                    .is_none_or(|terminal| terminal.interaction_id != interaction_id) =>
        {
            return Err(KernelError::InvalidPhaseInput {
                phase: state.phase,
                input: "record_external_command_rejected",
            });
        }
        _ => {}
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(
            state,
            env,
            vec![RecordBody::ExternalCommandRejected(input.rejection.clone())],
        )?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn known_effect(state: &KernelState, effect_id: crate::EffectId) -> bool {
    state
        .pending_model_effect
        .as_ref()
        .is_some_and(|pending| pending.requested.effect_id() == effect_id)
        || state.model_settlements.contains_key(&effect_id)
        || state.tool_settlements.contains_key(&effect_id)
        || state
            .tool_calls
            .values()
            .any(|identity| identity.effect_id == Some(effect_id))
        || state
            .completion_identities
            .values()
            .any(|identity| identity.effect_id == effect_id)
        || state
            .pending_interaction
            .as_ref()
            .is_some_and(|pending| pending.request.effect_id() == effect_id)
}

fn decide_configure_output(
    state: &KernelState,
    env: &TransitionEnv,
    configuration: OutputConfiguration,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    configuration
        .validate()
        .map_err(|reason_code| KernelError::InvalidInputPayload {
            field: "output.schema.schema_version",
            reason_code,
        })?;
    if state.output_configuration.as_ref() == Some(&configuration) {
        return duplicate_decision(state);
    }
    if state.output_configuration.is_some() {
        return Err(KernelError::ConflictingSettlement);
    }
    if state.phase != Some(RunPhase::BeforeRun) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "configure_output",
        });
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(
            state,
            env,
            vec![RecordBody::OutputConfigured(configuration)],
        )?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn decide_capabilities_activated(
    state: &KernelState,
    env: &TransitionEnv,
    activation: CapabilitiesActivated,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    activation
        .validate()
        .map_err(|reason_code| KernelError::InvalidInputPayload {
            field: "capabilities.active",
            reason_code,
        })?;
    if state.active_capabilities.as_ref() == activation.active.as_ref()
        && state.resolved_plan_digest == Some(activation.resolved_plan_digest)
    {
        return duplicate_decision(state);
    }
    if state.phase != Some(RunPhase::BeforeRun) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "capabilities_activated",
        });
    }
    if activation.prior_plan_digest != state.resolved_plan_digest {
        return Err(KernelError::ConflictingSettlement);
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(
            state,
            env,
            vec![RecordBody::CapabilitiesActivated(activation)],
        )?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn decide_output_validated(
    state: &KernelState,
    env: &TransitionEnv,
    input: OutputValidated,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    input
        .validate()
        .map_err(|reason_code| KernelError::InvalidInputPayload {
            field: "output_validation",
            reason_code,
        })?;
    if state.phase != Some(RunPhase::AfterModel) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "output_validated",
        });
    }
    let Some(OutputConfiguration {
        output: OutputSpec::JsonSchema { schema },
        end_strategy,
    }) = state.output_configuration.as_ref()
    else {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "output_validated",
        });
    };
    if schema != &input.schema {
        return Err(KernelError::ConflictingSettlement);
    }
    match (
        &input.outcome,
        state.final_result.as_ref(),
        state.validation_failure.as_ref(),
    ) {
        (crate::ValidationOutcome::Valid, Some(result), None)
            if result.message_id == input.message_id
                && result.schema == input.schema
                && result.value == input.candidate
                && result.source == input.source =>
        {
            return duplicate_decision(state);
        }
        (crate::ValidationOutcome::Invalid { issues, feedback }, None, Some(failure))
            if failure.message_id == input.message_id
                && failure.schema == input.schema
                && failure.candidate_digest == input.candidate.digest()
                && failure.source == input.source
                && &failure.issues == issues
                && &failure.feedback == feedback =>
        {
            return duplicate_decision(state);
        }
        (_, Some(_), _) | (_, _, Some(_)) => return Err(KernelError::ConflictingSettlement),
        _ => {}
    }
    let TerminalCandidate::Completed {
        cycle,
        turn_id,
        model_request_id,
        effect_id,
        message_id,
        ..
    } = state
        .terminal_candidate
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?
    else {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "output_validated",
        });
    };
    if *message_id != input.message_id {
        return Err(KernelError::AssistantMessageMismatch);
    }
    let message = state
        .messages
        .last()
        .filter(|message| message.id() == message_id)
        .ok_or(KernelError::AssistantMessageMismatch)?;
    validate_structured_source(message, &input.source, &input.candidate)?;
    let application_calls = application_tool_call_ids(message);
    let identity = OutputCandidateIdentity {
        cycle: *cycle,
        turn_id: *turn_id,
        model_request_id: *model_request_id,
        effect_id: *effect_id,
        message_id: *message_id,
    };
    let body = output_validation_body(state, input, identity, *end_strategy, application_calls)?;
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: draft_for_state(state, env, vec![body])?,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn application_tool_call_ids(message: &crate::Message) -> Vec<crate::ToolCallId> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) if !crate::is_internal_tool_name(call.tool_name()) => {
                Some(*call.tool_call_id())
            }
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy)]
struct OutputCandidateIdentity {
    cycle: u64,
    turn_id: crate::TurnId,
    model_request_id: crate::ModelRequestId,
    effect_id: crate::EffectId,
    message_id: crate::MessageId,
}

fn output_validation_body(
    state: &KernelState,
    input: OutputValidated,
    identity: OutputCandidateIdentity,
    end_strategy: OutputEndStrategy,
    application_calls: Vec<crate::ToolCallId>,
) -> Result<RecordBody, KernelError> {
    Ok(match input.outcome {
        crate::ValidationOutcome::Valid => RecordBody::FinalResultRecorded(FinalResultRecorded {
            cycle: identity.cycle,
            turn_id: identity.turn_id,
            model_request_id: identity.model_request_id,
            effect_id: identity.effect_id,
            message_id: identity.message_id,
            schema: input.schema,
            value_digest: input.candidate.digest(),
            value: input.candidate,
            source: input.source,
            end_strategy,
            skipped_tool_call_ids: if end_strategy == OutputEndStrategy::Early {
                Arc::from(application_calls)
            } else {
                Arc::from([])
            },
        }),
        crate::ValidationOutcome::Invalid { issues, feedback } => {
            if issues.is_empty() {
                return Err(KernelError::InvalidInputPayload {
                    field: "outcome.issues",
                    reason_code: "must_not_be_empty",
                });
            }
            let error = crate::validation::expected_validation_error(
                state.retry.attempts,
                state
                    .accepted
                    .as_ref()
                    .and_then(|accepted| accepted.limits().max_retries),
            )
            .map_err(|_| KernelError::InvariantViolation)?;
            RecordBody::OutputValidationFailed(OutputValidationFailed {
                cycle: identity.cycle,
                turn_id: identity.turn_id,
                model_request_id: identity.model_request_id,
                effect_id: identity.effect_id,
                message_id: identity.message_id,
                schema: input.schema,
                candidate_digest: input.candidate.digest(),
                source: input.source,
                issues,
                feedback,
                error,
                skipped_tool_call_ids: Arc::from(application_calls),
            })
        }
    })
}

fn validate_structured_source(
    message: &crate::Message,
    source: &StructuredResultSource,
    candidate: &crate::RawJson,
) -> Result<(), KernelError> {
    let valid = match source {
        StructuredResultSource::JsonBlock { content_index } => usize::try_from(*content_index)
            .ok()
            .and_then(|index| message.content().get(index))
            .is_some_and(
                |block| matches!(block, ContentBlock::Json(value) if value.value() == candidate),
            ),
        StructuredResultSource::InternalTool { tool_call_id } => {
            message.content().iter().any(|block| {
                matches!(block, ContentBlock::ToolCall(call)
                    if call.tool_call_id() == tool_call_id
                        && call.tool_name() == SUBMIT_FINAL_OUTPUT_TOOL
                        && call.arguments() == candidate)
            })
        }
    };
    if valid {
        Ok(())
    } else {
        Err(KernelError::AssistantMessageMismatch)
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "limit precedence and all frozen observation points stay visible in one pure decision"
)]
fn decide_limit(
    state: &KernelState,
    env: &TransitionEnv,
    input: &KernelInput,
    context_canonical: Option<(Digest, usize)>,
) -> Result<Option<Decision>, KernelError> {
    // Close an outstanding interaction before converting the command into a
    // run-limit failure. Approval `expires_at` is copied from the run
    // deadline, so expire-if-due would otherwise emit LimitReached+RunFailed
    // and fail apply with `pending_interaction` still set.
    if matches!(
        input,
        KernelInput::AcceptRun(_) | KernelInput::InteractionSettled(_)
    ) || state.accepted.is_none()
        || state.cancellation.is_some()
        || state.terminal.is_some()
        || state.pending_interaction.is_some()
        || matches!(
            state.phase,
            Some(
                RunPhase::AwaitingInteraction
                    | RunPhase::Completed
                    | RunPhase::Failed
                    | RunPhase::Cancelled
            )
        )
    {
        return Ok(None);
    }
    let accepted = state
        .accepted
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    if let Some(policy) = unknown_cost_policy(accepted, input) {
        match policy {
            crate::UnknownUsagePolicy::FailClosed => {
                return Ok(Some(control_failure_decision(
                    state,
                    env,
                    "unknown_cost_usage",
                    "completion omitted required cost usage",
                    ErrorCategory::Limit,
                )?));
            }
            crate::UnknownUsagePolicy::SuspendForDecision => {
                validate_allocated_ids(&env.ids, IdRequirements::new(1, 1, 0, 0, 0, 0))?;
                let records = draft_for_state(
                    state,
                    env,
                    vec![RecordBody::RunSuspended(RunSuspended {
                        reason_code: ErrorCode::new("unknown_cost_usage")
                            .map_err(|_| KernelError::InvariantViolation)?,
                        cancellation_request_id: None,
                    })],
                )?;
                return Ok(Some(Decision {
                    expected_sequence: next_sequence(state)?,
                    records,
                    actions: Vec::new(),
                    diagnostics: Vec::new(),
                }));
            }
            crate::UnknownUsagePolicy::AllowWithinReservedMaximum => {}
        }
    }
    let mut usage = state.limit_usage.clone();
    if let Some(accepted_at) = state.accepted_at {
        let elapsed = env
            .now
            .as_unix_ms()
            .checked_sub(accepted_at.as_unix_ms())
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(KernelError::InvalidInputPayload {
                field: "wall_time",
                reason_code: "overflow",
            })?;
        usage.wall_time = crate::Duration::from_millis(elapsed);
    }
    match input {
        KernelInput::StageSettled(StageSettled {
            outcome: ReducerStageOutcome::ContextPrepared { .. },
            ..
        }) => {
            usage.turns = usage
                .turns
                .checked_add(1)
                .ok_or(KernelError::InvalidInputPayload {
                    field: "turns",
                    reason_code: "overflow",
                })?;
            let (_, bytes) = context_canonical.ok_or(KernelError::InvariantViolation)?;
            usage.context_bytes = usage
                .context_bytes
                .checked_add(u64::try_from(bytes).map_err(|_| KernelError::InvariantViolation)?)
                .ok_or(KernelError::InvalidInputPayload {
                    field: "context_bytes",
                    reason_code: "overflow",
                })?;
        }
        KernelInput::StageSettled(StageSettled {
            outcome: ReducerStageOutcome::ModelRequestPrepared { .. },
            ..
        }) => {
            usage.model_requests =
                usage
                    .model_requests
                    .checked_add(1)
                    .ok_or(KernelError::InvalidInputPayload {
                        field: "model_requests",
                        reason_code: "overflow",
                    })?;
        }
        KernelInput::StageSettled(StageSettled {
            outcome: ReducerStageOutcome::ToolBatchPrepared { calls, .. },
            ..
        }) => {
            usage.tool_calls = usage
                .tool_calls
                .checked_add(
                    u64::try_from(calls.len()).map_err(|_| KernelError::InvariantViolation)?,
                )
                .ok_or(KernelError::InvalidInputPayload {
                    field: "tool_calls",
                    reason_code: "overflow",
                })?;
            let largest = largest_tool_group(calls)?;
            usage.max_parallel_tools = usage.max_parallel_tools.max(largest);
        }
        KernelInput::StageSettled(StageSettled {
            outcome: ReducerStageOutcome::Retry(_),
            ..
        }) => {
            usage.retries =
                usage
                    .retries
                    .checked_add(1)
                    .ok_or(KernelError::InvalidInputPayload {
                        field: "retries",
                        reason_code: "overflow",
                    })?;
        }
        KernelInput::ModelSettled(ModelSettled {
            outcome: ModelSettlement::Completed { completion, .. },
            ..
        })
        | KernelInput::ToolBatchSettled(super::input::ToolBatchSettled {
            outcome: super::input::ToolSettlement::Completed(completion),
            ..
        }) => {
            if let Err(failure) = project_completed_usage(
                &mut usage,
                completion.output(),
                completion.usage(),
                accepted.limits(),
            ) {
                return Ok(Some(control_failure_decision(
                    state,
                    env,
                    failure.code,
                    failure.message,
                    ErrorCategory::Limit,
                )?));
            }
        }
        KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
            completion, ..
        }) => {
            if let ExternalEffectOutcome::Completed {
                output,
                usage: completion_usage,
                ..
            } = &completion.outcome
                && let Err(failure) = project_completed_usage(
                    &mut usage,
                    output,
                    completion_usage.as_ref(),
                    accepted.limits(),
                )
            {
                return Ok(Some(control_failure_decision(
                    state,
                    env,
                    failure.code,
                    failure.message,
                    ErrorCategory::Limit,
                )?));
            }
        }
        _ => {}
    }

    let deadline_crossing = match (state.accepted_at, accepted.effective_deadline()) {
        (Some(accepted_at), Some(deadline)) if env.now >= deadline => {
            let maximum_ms = deadline
                .as_unix_ms()
                .checked_sub(accepted_at.as_unix_ms())
                .and_then(|value| u64::try_from(value).ok())
                .ok_or(KernelError::InvalidInputPayload {
                    field: "deadline",
                    reason_code: "overflow",
                })?;
            Some((
                LimitDimension::WallTime,
                LimitValue::Duration(usage.wall_time),
                LimitValue::Duration(crate::Duration::from_millis(maximum_ms)),
                "deadline_exceeded",
                ErrorCategory::Deadline,
            ))
        }
        _ => None,
    };
    let crossing = deadline_crossing.or(first_limit_crossing(accepted, &usage)?);
    let Some((dimension, observed, maximum, code, category)) = crossing else {
        return Ok(None);
    };
    validate_allocated_ids(&env.ids, IdRequirements::new(2, 2, 0, 0, 0, 0))?;
    let reached = LimitReached {
        dimension,
        observed,
        maximum,
        usage: usage.clone(),
        usage_digest: usage
            .digest()
            .map_err(|_| KernelError::InvariantViolation)?,
    };
    let error = ErrorDescriptor::new(code, "configured run limit reached", category, false)
        .map_err(|_| KernelError::InvariantViolation)?;
    let records = draft_for_state(
        state,
        env,
        vec![
            RecordBody::LimitReached(reached),
            RecordBody::RunFailed(failure_from_state(state, error)),
        ],
    )?;
    Ok(Some(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    }))
}

fn unknown_cost_policy(
    accepted: &crate::RunAccepted,
    input: &KernelInput,
) -> Option<crate::UnknownUsagePolicy> {
    let maximum = accepted.limits().max_cost.as_ref()?;
    let usage = match input {
        KernelInput::ModelSettled(ModelSettled {
            outcome: ModelSettlement::Completed { completion, .. },
            ..
        })
        | KernelInput::ToolBatchSettled(super::input::ToolBatchSettled {
            outcome: super::input::ToolSettlement::Completed(completion),
            ..
        }) => completion.usage(),
        KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
            completion, ..
        }) => match &completion.outcome {
            ExternalEffectOutcome::Completed { usage, .. } => usage.as_ref(),
            ExternalEffectOutcome::Failed { .. } => return None,
        },
        _ => return None,
    };
    usage
        .and_then(crate::Usage::cost)
        .is_none()
        .then_some(maximum.unknown_usage())
}

fn control_failure_decision(
    state: &KernelState,
    env: &TransitionEnv,
    code: &str,
    message: &str,
    category: ErrorCategory,
) -> Result<Decision, KernelError> {
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 1, 0, 0, 0, 0))?;
    let error = ErrorDescriptor::new(code, message, category, false)
        .map_err(|_| KernelError::InvariantViolation)?;
    let records = draft_for_state(
        state,
        env,
        vec![RecordBody::RunFailed(failure_from_state(state, error))],
    )?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn largest_tool_group(calls: &[crate::ToolCallPlan]) -> Result<u32, KernelError> {
    let mut largest = 0_u32;
    let mut current = 0_u32;
    for (index, call) in calls.iter().enumerate() {
        if index == 0
            || (calls[index - 1].execution() == crate::ToolExecutionMode::Parallel
                && call.execution() == crate::ToolExecutionMode::Parallel)
        {
            current = current
                .checked_add(1)
                .ok_or(KernelError::InvalidInputPayload {
                    field: "parallel_tools",
                    reason_code: "overflow",
                })?;
        } else {
            current = 1;
        }
        largest = largest.max(current);
    }
    Ok(largest)
}

fn project_completed_usage(
    usage: &mut LimitUsage,
    output: &crate::RawJson,
    completion: Option<&crate::Usage>,
    limits: &crate::RunLimits,
) -> Result<(), UsageProjectionFailure> {
    usage.output_bytes = usage
        .output_bytes
        .checked_add(u64::try_from(output.as_bytes().len()).map_err(|_| {
            UsageProjectionFailure {
                code: "output_bytes_overflow",
                message: "output byte accounting overflowed",
            }
        })?)
        .ok_or(UsageProjectionFailure {
            code: "output_bytes_overflow",
            message: "output byte accounting overflowed",
        })?;
    let Some(completion) = completion else {
        return Ok(());
    };
    if let Some(value) = completion.input_tokens() {
        usage.input_tokens =
            usage
                .input_tokens
                .checked_add(value)
                .ok_or(UsageProjectionFailure {
                    code: "input_tokens_overflow",
                    message: "input token accounting overflowed",
                })?;
    }
    if let Some(value) = completion.output_tokens() {
        usage.output_tokens =
            usage
                .output_tokens
                .checked_add(value)
                .ok_or(UsageProjectionFailure {
                    code: "output_tokens_overflow",
                    message: "output token accounting overflowed",
                })?;
    }
    if let Some(value) = completion.cost() {
        let Some(maximum) = limits.max_cost.as_ref() else {
            return Err(UsageProjectionFailure {
                code: "cost_policy_mismatch",
                message: "completion reported cost without an accepted cost policy",
            });
        };
        if value.unit() != maximum.unit()
            || value.pricing_policy_version() != maximum.pricing_policy_version()
        {
            return Err(UsageProjectionFailure {
                code: "cost_policy_mismatch",
                message: "completion cost does not match the accepted pricing policy",
            });
        }
        let current = usage.cost.as_ref().map_or(0, crate::CostAmount::micros);
        let micros = current
            .checked_add(value.micros())
            .ok_or(UsageProjectionFailure {
                code: "cost_overflow",
                message: "cost accounting overflowed",
            })?;
        usage.cost = Some(
            crate::CostAmount::try_new(value.unit(), micros, value.pricing_policy_version())
                .map_err(|_| UsageProjectionFailure {
                    code: "cost_policy_mismatch",
                    message: "completion cost does not match the accepted pricing policy",
                })?,
        );
    }
    for (key, delta) in completion.extension_counters() {
        if !limits.extension_counters.contains_key(key) {
            return Err(UsageProjectionFailure {
                code: "unregistered_extension_counter",
                message: "completion reported an unregistered extension counter",
            });
        }
        let current = usage.extension_counters.get(key).copied().unwrap_or(0);
        usage.extension_counters.insert(
            key.clone(),
            current.checked_add(*delta).ok_or(UsageProjectionFailure {
                code: "counter_overflow",
                message: "extension counter accounting overflowed",
            })?,
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct UsageProjectionFailure {
    code: &'static str,
    message: &'static str,
}

type LimitCrossing = (
    LimitDimension,
    LimitValue,
    LimitValue,
    &'static str,
    ErrorCategory,
);

#[expect(
    clippy::too_many_lines,
    reason = "the ordered limit vocabulary defines deterministic first-crossing precedence"
)]
fn first_limit_crossing(
    accepted: &crate::RunAccepted,
    usage: &LimitUsage,
) -> Result<Option<LimitCrossing>, KernelError> {
    let limits = accepted.limits();
    macro_rules! count_limit {
        ($field:expr, $maximum:expr, $dimension:expr) => {
            if let Some(maximum) = $maximum
                && $field > maximum
            {
                return Ok(Some((
                    $dimension,
                    LimitValue::Count(u64::from($field)),
                    LimitValue::Count(u64::from(maximum)),
                    "limit_reached",
                    ErrorCategory::Limit,
                )));
            }
        };
    }
    count_limit!(
        usage.model_requests,
        limits.max_model_requests,
        LimitDimension::ModelRequests
    );
    count_limit!(usage.turns, limits.max_turns, LimitDimension::Turns);
    count_limit!(
        usage.tool_calls,
        limits.max_tool_calls,
        LimitDimension::ToolCalls
    );
    count_limit!(
        usage.max_parallel_tools,
        limits.max_parallel_tools,
        LimitDimension::ParallelTools
    );
    count_limit!(
        usage.input_tokens,
        limits.max_input_tokens,
        LimitDimension::InputTokens
    );
    count_limit!(
        usage.output_tokens,
        limits.max_output_tokens,
        LimitDimension::OutputTokens
    );
    if let Some(maximum) = limits.max_context_bytes
        && usage.context_bytes > maximum
    {
        return Ok(Some((
            LimitDimension::ContextBytes,
            LimitValue::Bytes(usage.context_bytes),
            LimitValue::Bytes(maximum),
            "limit_reached",
            ErrorCategory::Limit,
        )));
    }
    if let Some(maximum) = limits.max_output_bytes
        && usage.output_bytes > maximum
    {
        return Ok(Some((
            LimitDimension::OutputBytes,
            LimitValue::Bytes(usage.output_bytes),
            LimitValue::Bytes(maximum),
            "limit_reached",
            ErrorCategory::Limit,
        )));
    }
    count_limit!(usage.retries, limits.max_retries, LimitDimension::Retries);
    if let Some(maximum) = limits.max_wall_time
        && usage.wall_time > maximum
    {
        return Ok(Some((
            LimitDimension::WallTime,
            LimitValue::Duration(usage.wall_time),
            LimitValue::Duration(maximum),
            "limit_reached",
            ErrorCategory::Limit,
        )));
    }
    if let (Some(maximum), Some(observed)) = (limits.max_cost.as_ref(), usage.cost.as_ref()) {
        debug_assert_eq!(observed.unit(), maximum.unit());
        debug_assert_eq!(
            observed.pricing_policy_version(),
            maximum.pricing_policy_version()
        );
        if observed.micros() > maximum.micros() {
            let max = crate::CostAmount::try_new(
                maximum.unit(),
                maximum.micros(),
                maximum.pricing_policy_version(),
            )
            .map_err(|_| KernelError::InvariantViolation)?;
            return Ok(Some((
                LimitDimension::Cost,
                LimitValue::Cost(observed.clone()),
                LimitValue::Cost(max),
                "limit_reached",
                ErrorCategory::Limit,
            )));
        }
    }
    for (key, observed) in &usage.extension_counters {
        let Some(maximum) = limits.extension_counters.get(key) else {
            return Err(KernelError::InvariantViolation);
        };
        if observed > maximum {
            return Ok(Some((
                LimitDimension::Extension { key: key.clone() },
                LimitValue::Count(*observed),
                LimitValue::Count(*maximum),
                "limit_reached",
                ErrorCategory::Limit,
            )));
        }
    }
    Ok(None)
}

fn decide_accept(
    state: &KernelState,
    env: &TransitionEnv,
    input: &AcceptRun,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    if state.phase.is_some() || state.accepted.is_some() {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "accept_run",
        });
    }
    if input.accepted.run_id() != input.accepted.relation().root_run_id()
        && input.accepted.relation().parent_run_id().is_none()
    {
        return Err(KernelError::InvalidRunAcceptance);
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 1, 0, 0, 0, 0))?;
    let body = RecordBody::RunAccepted(input.accepted.clone());
    let record = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        required(env.ids.record_ids(), 0, "record_ids")?,
        input.session_id,
        input.lane_id,
        Some(input.accepted.run_id()),
        env.now,
        vec![required(env.ids.event_ids(), 0, "event_ids")?],
        body,
    )
    .map_err(|_| KernelError::InvalidRunAcceptance)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records: vec![record],
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn decide_stage(
    state: &KernelState,
    env: &TransitionEnv,
    input: &StageSettled,
    context_canonical: Option<(Digest, usize)>,
) -> Result<Decision, KernelError> {
    validate_stage_input(input)?;
    let settlement_digest = stage_digest(input)?;
    if let Some(existing) = state.stage_settlements.get(&input.cursor) {
        return if *existing == settlement_digest {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    reject_terminal(state)?;
    let expected = expected_stage_cursor(state).ok_or(KernelError::InvalidPhaseInput {
        phase: state.phase,
        input: "stage_settled",
    })?;
    if input.cursor != expected {
        return Err(KernelError::StageCursorMismatch {
            expected,
            actual: input.cursor,
        });
    }

    if matches!(input.outcome, ReducerStageOutcome::ToolBatchPrepared { .. }) {
        return super::tool::decide_batch_prepared(state, env, input, settlement_digest);
    }

    let requirements = stage_id_requirements(state, input)?;
    capacity::preflight_decision(
        state,
        StateGrowth {
            stage: Some(input.cursor),
            ..StateGrowth::default()
        },
    )?;
    validate_allocated_ids(&env.ids, requirements)?;
    let (bodies, action) = stage_bodies(state, env, input, settlement_digest, context_canonical)?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: action.into_iter().collect(),
        diagnostics: Vec::new(),
    })
}

fn stage_id_requirements(
    state: &KernelState,
    input: &StageSettled,
) -> Result<IdRequirements, KernelError> {
    let cursor = input.cursor;
    match &input.outcome {
        ReducerStageOutcome::Continue
            if matches!(
                cursor.stage,
                Stage::BeforeRun | Stage::AfterModel | Stage::AfterToolBatch
            ) =>
        {
            if cursor.stage == Stage::AfterModel
                && matches!(
                    state.output_configuration,
                    Some(OutputConfiguration {
                        output: OutputSpec::JsonSchema { .. },
                        ..
                    })
                )
                && state.final_result.is_none()
                && state.validation_failure.is_none()
            {
                return Err(KernelError::InvalidPhaseInput {
                    phase: state.phase,
                    input: "stage_settled",
                });
            }
            Ok(IdRequirements::new(1, 0, 0, 0, 0, 0))
        }
        ReducerStageOutcome::ContextPrepared { .. } if cursor.stage == Stage::PrepareContext => {
            Ok(IdRequirements::new(2, 0, 0, 1, 0, 0))
        }
        ReducerStageOutcome::ModelRequestPrepared {
            output_contract, ..
        } if cursor.stage == Stage::BeforeModel => {
            if output_contract.kind != EffectOutputKind::ModelResponse {
                return Err(KernelError::ModelRequestContractMismatch);
            }
            Ok(IdRequirements::new(2, 1, 1, 0, 1, 0))
        }
        ReducerStageOutcome::FinalizeAccepted if cursor.stage == Stage::BeforeFinalize => {
            terminal_body_from_candidate(state)?;
            Ok(IdRequirements::new(2, 1, 0, 0, 0, 0))
        }
        ReducerStageOutcome::ContinueModel { .. }
            if cursor.stage == Stage::BeforeFinalize
                && matches!(
                    state.terminal_candidate,
                    Some(TerminalCandidate::Completed { .. })
                ) =>
        {
            state
                .cycle
                .checked_add(1)
                .ok_or(KernelError::CycleOverflow)?;
            Ok(IdRequirements::new(1, 0, 0, 0, 0, 0))
        }
        ReducerStageOutcome::Fail(_) if cursor.stage == Stage::BeforeFinalize => {
            Ok(IdRequirements::new(2, 1, 0, 0, 0, 0))
        }
        ReducerStageOutcome::Retry(_) if cursor.stage == Stage::BeforeFinalize => {
            Ok(IdRequirements::new(3, 1, 1, 0, 0, 0))
        }
        ReducerStageOutcome::Fail(_)
            if matches!(
                cursor.stage,
                Stage::BeforeRun
                    | Stage::PrepareContext
                    | Stage::BeforeModel
                    | Stage::AfterModel
                    | Stage::BeforeToolBatch
                    | Stage::AfterToolBatch
            ) =>
        {
            Ok(IdRequirements::new(1, 0, 0, 0, 0, 0))
        }
        _ => Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        }),
    }
}

fn validate_stage_input(input: &StageSettled) -> Result<(), KernelError> {
    match &input.outcome {
        ReducerStageOutcome::ContextPrepared { messages }
            if messages.len() > SEMANTIC_ARRAY_MAX_ITEMS =>
        {
            Err(KernelError::InvalidInputPayload {
                field: "messages",
                reason_code: "too_many_items",
            })
        }
        ReducerStageOutcome::ContinueModel {
            reason: Some(reason),
        } if reason.is_empty()
            || reason.len() > TEXT_MAX_BYTES
            || reason.as_bytes().contains(&0) =>
        {
            Err(KernelError::InvalidInputPayload {
                field: "reason",
                reason_code: "invalid_text",
            })
        }
        ReducerStageOutcome::Fail(error) => validate_error_descriptor(error),
        _ => Ok(()),
    }
}

fn stage_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    input: &StageSettled,
    settlement_digest: Digest,
    context_canonical: Option<(Digest, usize)>,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let cursor = input.cursor;
    match &input.outcome {
        ReducerStageOutcome::Continue
            if matches!(
                cursor.stage,
                Stage::BeforeRun | Stage::AfterModel | Stage::AfterToolBatch
            ) =>
        {
            Ok((
                vec![stage_record(
                    cursor,
                    StageDisposition::Continued,
                    settlement_digest,
                )],
                None,
            ))
        }
        ReducerStageOutcome::ContextPrepared { messages }
            if cursor.stage == Stage::PrepareContext =>
        {
            prepared_context_bodies(
                env,
                cursor,
                messages,
                settlement_digest,
                context_canonical.ok_or(KernelError::InvariantViolation)?.0,
            )
        }
        ReducerStageOutcome::ModelRequestPrepared {
            request: _,
            component: _,
            output_contract: _,
            retry_safety: _,
            deadline: _,
        } if cursor.stage == Stage::BeforeModel => {
            requested_model_bodies(state, env, cursor, settlement_digest, &input.outcome)
        }
        ReducerStageOutcome::FinalizeAccepted if cursor.stage == Stage::BeforeFinalize => {
            accepted_finalize_bodies(state, cursor, settlement_digest)
        }
        ReducerStageOutcome::ContinueModel { .. }
            if cursor.stage == Stage::BeforeFinalize
                && matches!(
                    state.terminal_candidate,
                    Some(TerminalCandidate::Completed { .. })
                ) =>
        {
            continued_model_bodies(state, cursor, settlement_digest)
        }
        ReducerStageOutcome::Fail(error) => {
            Ok(failed_stage_bodies(state, cursor, settlement_digest, error))
        }
        ReducerStageOutcome::Retry(directive) if cursor.stage == Stage::BeforeFinalize => {
            retry_bodies(state, env, cursor, settlement_digest, directive)
        }
        _ => Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        }),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "retry admission and its atomic timer-intent record batch are one fail-closed transition"
)]
fn retry_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    cursor: StageCursor,
    settlement_digest: Digest,
    directive: &crate::RetryDirective,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    if directive.classification == RetryClassification::Validation
        && state.validation_failure.is_none()
    {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    let TerminalCandidate::Failed { error, .. } =
        state
            .terminal_candidate
            .as_ref()
            .ok_or(KernelError::InvalidPhaseInput {
                phase: state.phase,
                input: "stage_settled",
            })?
    else {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    };
    if state.validation_failure.is_some()
        && directive.classification != RetryClassification::Validation
    {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    if !error.retryable {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    let attempt = state
        .retry
        .attempts
        .checked_add(1)
        .ok_or(KernelError::InvalidInputPayload {
            field: "retry.attempt",
            reason_code: "overflow",
        })?;
    if state
        .accepted
        .as_ref()
        .and_then(|accepted| accepted.limits().max_retries)
        .is_some_and(|maximum| attempt > maximum)
    {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "retry_limit_reached",
        });
    }
    let due_at =
        env.now
            .checked_add(directive.backoff)
            .map_err(|_| KernelError::InvalidInputPayload {
                field: "retry.backoff",
                reason_code: "overflow",
            })?;
    let timer_effect_id = required(env.ids.effect_ids(), 0, "effect_ids")?;
    let retry = RetryScheduled::try_new(
        state.cycle,
        attempt,
        directive.classification,
        directive.policy_version.as_ref(),
        timer_effect_id,
        due_at,
        error.clone(),
    )
    .map_err(|_| KernelError::InvalidInputPayload {
        field: "retry",
        reason_code: "invalid",
    })?;
    let requested = crate::EffectRequested::try_new(
        timer_effect_id,
        EffectKind::Timer,
        None,
        None,
        None,
        EffectOutputContract {
            kind: EffectOutputKind::TimerFiring,
            schema_version: 1,
            schema_digest: Digest::effect_output(br#"{"type":"timer_firing"}"#),
        },
        EffectInput::Timer { due_at },
        RetrySafety::IdempotentWithKey,
        Some(due_at),
    )
    .map_err(|_| KernelError::InvariantViolation)?;
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::RetryScheduled {
                    attempt,
                    timer_effect_id,
                    due_at,
                },
                settlement_digest,
            ),
            RecordBody::RetryScheduled(retry),
            RecordBody::EffectRequested(requested),
        ],
        Some(PostCommitAction::ExecuteEffect {
            effect_id: timer_effect_id,
        }),
    ))
}

fn decide_cancel(
    state: &KernelState,
    env: &TransitionEnv,
    input: &CancelRequested,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    if let Some(cancellation) = &state.cancellation {
        return if cancellation.request.initiator == input.initiator
            && cancellation.request.reason.as_deref() == input.reason.as_deref()
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    let accepted = state
        .accepted
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "cancel_requested",
        })?;
    validate_cancel_authorization(accepted, env, input)?;
    validate_allocated_ids(
        &env.ids,
        IdRequirements::new(1, 0, 0, 0, 0, 0).with_cancellations(1),
    )?;
    let request_id = required(
        env.ids.cancellation_request_ids(),
        0,
        "cancellation_request_ids",
    )?;
    let request =
        CancellationRequest::try_new(request_id, input.initiator.clone(), input.reason.as_deref())
            .map_err(|_| KernelError::InvalidInputPayload {
                field: "reason",
                reason_code: "invalid_label",
            })?;
    let records = draft_for_state(
        state,
        env,
        vec![RecordBody::CancellationRequested(CancellationRequested {
            request,
        })],
    )?;
    let actions = outstanding_requested_effects(state)
        .into_iter()
        .map(|effect_id| PostCommitAction::CancelEffect { effect_id })
        .collect();
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions,
        diagnostics: Vec::new(),
    })
}

fn validate_cancel_authorization(
    accepted: &crate::RunAccepted,
    env: &TransitionEnv,
    input: &CancelRequested,
) -> Result<(), KernelError> {
    let authorized = match &input.initiator {
        CancellationInitiator::Principal {
            principal,
            authorization,
        } => {
            principal == accepted.security().principal()
                && authorization.policy_version()
                    == accepted.security().authorization_policy_version()
                && authorization.decision_id() == accepted.security().authorization_decision_id()
        }
        CancellationInitiator::ParentRun { parent_run_id } => {
            accepted.relation().parent_run_id() == Some(*parent_run_id)
                && match accepted.propagation().cancellation {
                    crate::CancellationPropagation::Cascade => true,
                    crate::CancellationPropagation::DetachOnlyIfPreauthorized => !accepted
                        .security()
                        .authorization_decision_id()
                        .starts_with("detach:"),
                }
        }
        CancellationInitiator::Deadline => accepted
            .effective_deadline()
            .is_some_and(|deadline| env.now >= deadline),
        CancellationInitiator::RuntimeShutdown => true,
    };
    if !authorized {
        return Err(KernelError::InvalidInputPayload {
            field: "initiator",
            reason_code: "unauthorized",
        });
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "incremental classification and deterministic effect closure form one atomic decision"
)]
fn decide_reconciliation(
    state: &KernelState,
    env: &TransitionEnv,
    input: &CancellationReconciledInput,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    let cancellation = state
        .cancellation
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "cancellation_reconciled",
        })?;
    if input.request_id != cancellation.request.request_id {
        return Err(KernelError::ConflictingSettlement);
    }
    validate_reconciliation_input(input, cancellation)?;
    let duplicate = input
        .completed_effects
        .iter()
        .all(|id| cancellation.completed_effects.contains(id))
        && input
            .cancelled_effects
            .iter()
            .all(|id| cancellation.cancelled_effects.contains(id))
        && input
            .uncertain_effects
            .iter()
            .all(|id| cancellation.uncertain_effects.contains(id));
    if duplicate
        && (!input.completed_effects.is_empty()
            || !input.cancelled_effects.is_empty()
            || !input.uncertain_effects.is_empty())
    {
        return duplicate_decision(state);
    }
    let mut completed = cancellation.completed_effects.to_vec();
    let mut cancelled = cancellation.cancelled_effects.to_vec();
    let mut uncertain = cancellation.uncertain_effects.to_vec();
    completed.extend_from_slice(&input.completed_effects);
    cancelled.extend_from_slice(&input.cancelled_effects);
    uncertain.extend_from_slice(&input.uncertain_effects);
    completed.sort_unstable();
    completed.dedup();
    cancelled.sort_unstable();
    cancelled.dedup();
    uncertain.sort_unstable();
    uncertain.dedup();
    let reconciled = CancellationReconciled {
        request_id: input.request_id,
        completed_effects: completed.clone().into(),
        cancelled_effects: cancelled.clone().into(),
        uncertain_effects: uncertain.clone().into(),
    };
    let classified = completed
        .iter()
        .chain(cancelled.iter())
        .chain(uncertain.iter())
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let remaining = cancellation
        .outstanding_effects
        .iter()
        .filter(|effect_id| !classified.contains(effect_id))
        .count();
    let newly_completed = input
        .completed_effects
        .iter()
        .filter(|effect_id| !cancellation.completed_effects.contains(effect_id))
        .copied()
        .collect::<Vec<_>>();
    let newly_cancelled = input
        .cancelled_effects
        .iter()
        .filter(|effect_id| !cancellation.cancelled_effects.contains(effect_id))
        .copied()
        .collect::<Vec<_>>();
    let mut bodies = Vec::new();
    for effect_id in &newly_cancelled {
        if let Some(pending) = state
            .pending_model_effect
            .as_ref()
            .filter(|pending| pending.requested.effect_id() == *effect_id)
        {
            bodies.push(RecordBody::EffectCancelled(
                EffectCancelled::try_new(
                    *effect_id,
                    pending.requested.output_contract().clone(),
                    Some("cancelled"),
                    Option::<&str>::None,
                )
                .map_err(|_| KernelError::InvariantViolation)?,
            ));
        }
    }
    let mut tool_followups =
        super::tool::cancellation_followups(state, env, &newly_completed, &newly_cancelled)?;
    let first_non_cancelled = tool_followups
        .bodies
        .iter()
        .position(|body| !matches!(body, RecordBody::EffectCancelled(_)))
        .unwrap_or(tool_followups.bodies.len());
    bodies.extend(tool_followups.bodies.drain(..first_non_cancelled));
    bodies.push(RecordBody::CancellationReconciled(reconciled));
    bodies.append(&mut tool_followups.bodies);
    if !uncertain.is_empty() {
        bodies.push(RecordBody::RunSuspended(RunSuspended {
            reason_code: ErrorCode::new("cancellation_uncertain")
                .map_err(|_| KernelError::InvariantViolation)?,
            cancellation_request_id: Some(input.request_id),
        }));
    } else if remaining == 0 {
        bodies.push(RecordBody::RunCancelled(RunCancelled {
            request_id: input.request_id,
            reason_code: ErrorCode::new("cancelled")
                .map_err(|_| KernelError::InvariantViolation)?,
        }));
    }
    let event_count = bodies.iter().try_fold(0_usize, |count, body| {
        count
            .checked_add(
                body.derived_event_count(RECORD_KIND_VERSION)
                    .map_err(|_| KernelError::InvariantViolation)?,
            )
            .ok_or(KernelError::InvariantViolation)
    })?;
    if bodies.len() > APPEND_BATCH_MAX_RECORDS {
        return Err(KernelError::InvalidInputPayload {
            field: "records",
            reason_code: "too_many_items",
        });
    }
    capacity::preflight_decision(
        state,
        StateGrowth {
            messages: tool_followups.messages,
            tool_settlements: &tool_followups.settlements,
            ..StateGrowth::default()
        },
    )?;
    validate_allocated_ids(
        &env.ids,
        IdRequirements::new(bodies.len(), event_count, 0, 0, 0, tool_followups.messages),
    )?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn decide_timer_fired(
    state: &KernelState,
    env: &TransitionEnv,
    input: &TimerFiredInput,
) -> Result<Decision, KernelError> {
    reject_terminal(state)?;
    if let Some(existing) = state.retry.timer_firings.get(&input.effect_id) {
        return if existing.effect_id == input.effect_id
            && existing.due_at == input.due_at
            && existing.fired_at == input.fired_at
        {
            duplicate_decision(state)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    let pending = state
        .retry
        .pending
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "timer_fired",
        })?;
    if pending.timer_effect_id != input.effect_id
        || pending.due_at != input.due_at
        || input.fired_at < input.due_at
    {
        return Err(KernelError::ConflictingSettlement);
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    let records = draft_for_state(
        state,
        env,
        vec![RecordBody::TimerFired(TimerFired {
            effect_id: input.effect_id,
            due_at: input.due_at,
            fired_at: input.fired_at,
        })],
    )?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn outstanding_requested_effects(state: &KernelState) -> Vec<crate::EffectId> {
    let mut effects = state
        .pending_model_effect
        .as_ref()
        .map(|pending| vec![pending.requested.effect_id()])
        .unwrap_or_default();
    if let Some(batch) = &state.active_tool_batch {
        effects.extend(batch.calls.iter().filter_map(|call| match call.status {
            crate::ActiveToolCallStatus::Requested { .. } => Some(call.assigned.effect_id),
            _ => None,
        }));
    }
    effects.sort_unstable();
    effects.dedup();
    effects
}

fn validate_reconciliation_input(
    input: &CancellationReconciledInput,
    cancellation: &crate::CancellationState,
) -> Result<(), KernelError> {
    for (field, values) in [
        ("completed_effects", input.completed_effects.as_ref()),
        ("cancelled_effects", input.cancelled_effects.as_ref()),
        ("uncertain_effects", input.uncertain_effects.as_ref()),
    ] {
        if values.len() > SEMANTIC_ARRAY_MAX_ITEMS
            || values.windows(2).any(|pair| pair[0] >= pair[1])
            || values.iter().any(|id| {
                !cancellation.outstanding_effects.contains(id)
                    && !cancellation.completed_effects.contains(id)
                    && !cancellation.cancelled_effects.contains(id)
                    && !cancellation.uncertain_effects.contains(id)
            })
        {
            return Err(KernelError::InvalidInputPayload {
                field,
                reason_code: "invalid_effect_set",
            });
        }
    }
    if input
        .completed_effects
        .iter()
        .any(|id| input.cancelled_effects.contains(id) || input.uncertain_effects.contains(id))
        || input
            .cancelled_effects
            .iter()
            .any(|id| input.uncertain_effects.contains(id))
    {
        return Err(KernelError::InvalidInputPayload {
            field: "reconciliation",
            reason_code: "overlapping_effect_sets",
        });
    }
    if input.completed_effects.iter().any(|id| {
        cancellation.cancelled_effects.contains(id) || cancellation.uncertain_effects.contains(id)
    }) || input.cancelled_effects.iter().any(|id| {
        cancellation.completed_effects.contains(id) || cancellation.uncertain_effects.contains(id)
    }) || input.uncertain_effects.iter().any(|id| {
        cancellation.completed_effects.contains(id) || cancellation.cancelled_effects.contains(id)
    }) {
        return Err(KernelError::ConflictingSettlement);
    }
    Ok(())
}

fn prepared_context_bodies(
    env: &TransitionEnv,
    cursor: StageCursor,
    messages: &Arc<[crate::Message]>,
    settlement_digest: Digest,
    context_digest: Digest,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let turn_id = required(env.ids.turn_ids(), 0, "turn_ids")?;
    // The messages are already a shared slice and already canonicalized; taking
    // a `Vec` here and collecting back into an `Arc` copied the whole context
    // twice for no change in value.
    let context =
        ContextPrepared::from_shared(cursor.cycle, turn_id, Arc::clone(messages), context_digest);
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::ContextPrepared {
                    turn_id,
                    context_digest,
                },
                settlement_digest,
            ),
            RecordBody::ContextPrepared(context),
        ],
        None,
    ))
}

fn requested_model_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    cursor: StageCursor,
    settlement_digest: Digest,
    outcome: &ReducerStageOutcome,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let ReducerStageOutcome::ModelRequestPrepared {
        request,
        component,
        output_contract,
        retry_safety,
        deadline,
    } = outcome
    else {
        return Err(KernelError::InvariantViolation);
    };
    if output_contract.kind != EffectOutputKind::ModelResponse {
        return Err(KernelError::ModelRequestContractMismatch);
    }
    let turn_id = state
        .current_turn
        .as_ref()
        .map(|turn| turn.turn_id)
        .ok_or(KernelError::InvariantViolation)?;
    let model_request_id = required(env.ids.model_request_ids(), 0, "model_request_ids")?;
    let effect_id = required(env.ids.effect_ids(), 0, "effect_ids")?;
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Model,
        None,
        component.clone(),
        None,
        output_contract.clone(),
        EffectInput::Model {
            request: request.clone(),
        },
        *retry_safety,
        *deadline,
    )
    .map_err(|_| KernelError::ModelRequestContractMismatch)?;
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::ModelRequested {
                    turn_id,
                    model_request_id,
                    effect_id,
                },
                settlement_digest,
            ),
            RecordBody::EffectRequested(requested),
        ],
        Some(PostCommitAction::ExecuteEffect { effect_id }),
    ))
}

fn accepted_finalize_bodies(
    state: &KernelState,
    cursor: StageCursor,
    settlement_digest: Digest,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let terminal = terminal_body_from_candidate(state)?;
    Ok((
        vec![
            stage_record(
                cursor,
                StageDisposition::FinalizeAccepted,
                settlement_digest,
            ),
            terminal,
        ],
        None,
    ))
}

fn continued_model_bodies(
    state: &KernelState,
    cursor: StageCursor,
    settlement_digest: Digest,
) -> Result<(Vec<RecordBody>, Option<PostCommitAction>), KernelError> {
    let next_cycle = state
        .cycle
        .checked_add(1)
        .ok_or(KernelError::CycleOverflow)?;
    Ok((
        vec![stage_record(
            cursor,
            StageDisposition::ContinueModel { next_cycle },
            settlement_digest,
        )],
        None,
    ))
}

fn failed_stage_bodies(
    state: &KernelState,
    cursor: StageCursor,
    settlement_digest: Digest,
    error: &crate::ErrorDescriptor,
) -> (Vec<RecordBody>, Option<PostCommitAction>) {
    match cursor.stage {
        Stage::BeforeRun
        | Stage::PrepareContext
        | Stage::BeforeModel
        | Stage::AfterModel
        | Stage::BeforeToolBatch
        | Stage::AfterToolBatch => (
            vec![stage_record(
                cursor,
                StageDisposition::Failed {
                    error: error.clone(),
                },
                settlement_digest,
            )],
            None,
        ),
        Stage::BeforeFinalize => (
            vec![
                stage_record(
                    cursor,
                    StageDisposition::Failed {
                        error: error.clone(),
                    },
                    settlement_digest,
                ),
                RecordBody::RunFailed(failure_from_state(state, error.clone())),
            ],
            None,
        ),
    }
}

fn decide_model(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ModelSettled,
) -> Result<Decision, KernelError> {
    if let ModelSettlement::Failed(failed) = &input.outcome {
        validate_error_descriptor(failed.error())?;
    }
    let settlement_digest = direct_digest(input)?;
    decide_normalized_model(state, env, input, "model_settled", settlement_digest)
}

fn validate_external_completion_input(
    input: &ExternalEffectCompletedInput,
) -> Result<(), KernelError> {
    if input.completion.completion_id.is_empty()
        || input.completion.completion_id.len() > LABEL_MAX_BYTES
        || input.completion.completion_id.as_bytes().contains(&0)
    {
        return Err(KernelError::InvalidInputPayload {
            field: "completion_id",
            reason_code: "invalid_label",
        });
    }
    if matches!(
        &input.completion.outcome,
        ExternalEffectOutcome::Completed { artifacts, .. }
            if artifacts.len() > SEMANTIC_ARRAY_MAX_ITEMS
    ) {
        return Err(KernelError::InvalidInputPayload {
            field: "artifacts",
            reason_code: "too_many_items",
        });
    }
    if let ExternalEffectOutcome::Failed { error } = &input.completion.outcome {
        validate_error_descriptor(error)?;
    }
    Ok(())
}

fn decide_external(
    state: &KernelState,
    env: &TransitionEnv,
    input: ExternalEffectCompletedInput,
) -> Result<Decision, KernelError> {
    validate_external_completion_input(&input)?;
    if super::tool::is_known_tool_effect(state, input.completion.effect_id) {
        return super::tool::decide_external_tool(state, env, input);
    }
    let settlement_digest = external_digest(&input)?;
    if let Some(decision) = classify_model_duplicate(
        state,
        Some(input.completion.completion_id.as_ref()),
        input.completion.effect_id,
        settlement_digest,
        None,
    )? {
        return Ok(decision);
    }
    if state.phase != Some(RunPhase::AwaitingExternal) {
        reject_terminal(state)?;
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "external_effect_completed",
        });
    }
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        })?;
    if pending.requested.effect_id() != input.completion.effect_id {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    }
    if pending.deferred.is_none() {
        return Err(KernelError::EffectNotPending {
            effect_id: input.completion.effect_id,
        });
    }
    let completion_id = input.completion.completion_id;
    let outcome = match input.completion.outcome {
        ExternalEffectOutcome::Completed {
            output,
            usage,
            artifacts,
        } => {
            let assistant_message = input
                .assistant_message
                .ok_or(KernelError::AssistantMessagePresenceMismatch)?;
            let completion = EffectCompleted::try_new(
                pending.requested.effect_id(),
                pending.requested.output_contract().clone(),
                output,
                usage,
                artifacts.to_vec(),
                assistant_message.provider_ids().clone(),
                Some(completion_id.as_ref()),
                None,
            )
            .map_err(|_| KernelError::ModelSettlementMismatch)?;
            ModelSettlement::Completed {
                completion,
                assistant_message,
            }
        }
        ExternalEffectOutcome::Failed { error } => {
            if input.assistant_message.is_some() {
                return Err(KernelError::AssistantMessagePresenceMismatch);
            }
            ModelSettlement::Failed(
                EffectFailed::try_new(
                    pending.requested.effect_id(),
                    pending.requested.output_contract().clone(),
                    error,
                    None,
                    Some(completion_id.as_ref()),
                )
                .map_err(|_| KernelError::ModelSettlementMismatch)?,
            )
        }
    };
    let normalized = ModelSettled {
        turn_id: pending.turn_id,
        model_request_id: pending.model_request_id,
        outcome,
    };
    decide_normalized_model(
        state,
        env,
        &normalized,
        "external_effect_completed",
        settlement_digest,
    )
}

fn decide_normalized_model(
    state: &KernelState,
    env: &TransitionEnv,
    input: &ModelSettled,
    input_name: &'static str,
    settlement_digest: Digest,
) -> Result<Decision, KernelError> {
    let effect_id = settlement_effect_id(&input.outcome);
    if let Some(decision) = classify_model_duplicate(
        state,
        settlement_completion_id(&input.outcome),
        effect_id,
        settlement_digest,
        Some(input),
    )? {
        return Ok(decision);
    }
    reject_terminal(state)?;
    let required_phase = match input_name {
        "model_settled" => RunPhase::AwaitingModel,
        "external_effect_completed" => RunPhase::AwaitingExternal,
        _ => return Err(KernelError::InvariantViolation),
    };
    if state.phase != Some(required_phase) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: input_name,
        });
    }
    let pending = state
        .pending_model_effect
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    if pending.turn_id != input.turn_id
        || pending.model_request_id != input.model_request_id
        || pending.requested.effect_id() != effect_id
    {
        return Err(KernelError::ModelSettlementMismatch);
    }
    if pending.requested.kind() != EffectKind::Model {
        return Err(KernelError::ModelSettlementMismatch);
    }

    let (requirements, bodies) =
        model_settlement_bodies(state, env, pending, input, effect_id, settlement_digest)?;
    validate_allocated_ids(&env.ids, requirements)?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions: Vec::new(),
        diagnostics: Vec::new(),
    })
}

fn classify_model_duplicate(
    state: &KernelState,
    completion_id: Option<&str>,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
    input: Option<&ModelSettled>,
) -> Result<Option<Decision>, KernelError> {
    if let Some(completion_id) = completion_id
        && let Some(existing) = state.completion_identities.get(completion_id)
    {
        return if existing.effect_id == effect_id && existing.settlement_digest == settlement_digest
        {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingCompletionId)
        };
    }
    if let Some(existing) = state.model_settlements.get(&effect_id) {
        return if existing.digest == settlement_digest {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    if let Some(ModelSettled {
        outcome: ModelSettlement::Deferred(deferred),
        ..
    }) = input
        && let Some(pending) = state.pending_model_effect.as_ref()
        && pending.requested.effect_id() == deferred.effect_id
        && let Some(existing) = pending.deferred.as_ref()
    {
        let existing_digest = direct_digest(&ModelSettled {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            outcome: ModelSettlement::Deferred(existing.clone()),
        })?;
        return if existing_digest == settlement_digest {
            duplicate_decision(state).map(Some)
        } else {
            Err(KernelError::ConflictingSettlement)
        };
    }
    Ok(None)
}

fn model_settlement_bodies(
    state: &KernelState,
    env: &TransitionEnv,
    pending: &crate::PendingModelEffect,
    input: &ModelSettled,
    effect_id: crate::EffectId,
    settlement_digest: Digest,
) -> Result<(IdRequirements, Vec<RecordBody>), KernelError> {
    match &input.outcome {
        ModelSettlement::Completed {
            completion,
            assistant_message,
        } => {
            completion
                .validate_against(&pending.requested)
                .map_err(|_| KernelError::ModelSettlementMismatch)?;
            if completion.output_contract().kind != EffectOutputKind::ModelResponse {
                return Err(KernelError::ModelSettlementMismatch);
            }
            validate_completion_identity(
                state,
                completion.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            validate_assistant_semantics(state, env, assistant_message, completion)?;
            let tool_call_ids = assistant_tool_calls(assistant_message)
                .iter()
                .map(|call| *call.tool_call_id())
                .collect::<Vec<_>>();
            capacity::preflight_decision(
                state,
                StateGrowth {
                    messages: 1,
                    model: Some(effect_id),
                    completion: completion.completion_id(),
                    tool_calls: &tool_call_ids,
                    ..StateGrowth::default()
                },
            )?;
            let requirements =
                IdRequirements::new(2, 2, 0, 0, 0, 1).with_tools(0, tool_call_ids.len());
            validate_allocated_ids(&env.ids, requirements)?;
            let message_id = required(env.ids.message_ids(), 0, "message_ids")?;
            validate_assistant_message_id(message_id, assistant_message)?;
            validate_assistant_tool_call_ids(env.ids.tool_call_ids(), assistant_message)?;
            let parent_message_id = state.messages.last().map(|message| *message.id());
            Ok((
                requirements,
                vec![
                    RecordBody::EffectCompleted(completion.clone()),
                    RecordBody::EntryAppended(EntryAppended {
                        cycle: pending.cycle,
                        turn_id: pending.turn_id,
                        model_request_id: pending.model_request_id,
                        effect_id,
                        parent_message_id,
                        message: assistant_message.clone(),
                    }),
                ],
            ))
        }
        ModelSettlement::Deferred(deferred) => {
            deferred
                .validate_against(&pending.requested)
                .map_err(|_| KernelError::ModelSettlementMismatch)?;
            Ok((
                IdRequirements::new(1, 1, 0, 0, 0, 0),
                vec![RecordBody::EffectDeferred(deferred.clone())],
            ))
        }
        ModelSettlement::Failed(failed) => {
            failed
                .validate_against(&pending.requested)
                .map_err(|_| KernelError::ModelSettlementMismatch)?;
            validate_completion_identity(
                state,
                failed.completion_id(),
                effect_id,
                settlement_digest,
            )?;
            capacity::preflight_decision(
                state,
                StateGrowth {
                    model: Some(effect_id),
                    completion: failed.completion_id(),
                    ..StateGrowth::default()
                },
            )?;
            Ok((
                IdRequirements::new(1, 1, 0, 0, 0, 0),
                vec![RecordBody::EffectFailed(failed.clone())],
            ))
        }
    }
}

pub(super) fn draft_for_state(
    state: &KernelState,
    env: &TransitionEnv,
    bodies: Vec<RecordBody>,
) -> Result<Vec<RecordDraft>, KernelError> {
    let session_id = state.session_id.ok_or(KernelError::InvariantViolation)?;
    let lane_id = state.lane_id.ok_or(KernelError::InvariantViolation)?;
    let run_id = state
        .accepted
        .as_ref()
        .map(crate::run::RunAccepted::run_id)
        .ok_or(KernelError::InvariantViolation)?;
    let mut event_index = 0;
    let mut records = Vec::with_capacity(bodies.len());
    for (record_index, body) in bodies.into_iter().enumerate() {
        let count = body
            .derived_event_count(RECORD_KIND_VERSION)
            .map_err(|_| KernelError::InvariantViolation)?;
        let mut event_ids = Vec::with_capacity(count);
        for _ in 0..count {
            event_ids.push(required(env.ids.event_ids(), event_index, "event_ids")?);
            event_index += 1;
        }
        records.push(
            RecordDraft::try_new(
                RECORD_FORMAT_VERSION,
                RECORD_KIND_VERSION,
                required(env.ids.record_ids(), record_index, "record_ids")?,
                session_id,
                lane_id,
                Some(run_id),
                env.now,
                event_ids,
                body,
            )
            .map_err(|_| KernelError::InvariantViolation)?,
        );
    }
    Ok(records)
}

fn stage_record(
    cursor: StageCursor,
    disposition: StageDisposition,
    settlement_digest: Digest,
) -> RecordBody {
    RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
        cursor,
        disposition,
        settlement_digest,
    })
}

fn terminal_body_from_candidate(state: &KernelState) -> Result<RecordBody, KernelError> {
    match state
        .terminal_candidate
        .as_ref()
        .ok_or(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        })? {
        TerminalCandidate::Completed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            result_digest,
        } => Ok(RecordBody::RunCompleted(RunCompleted {
            cycle: *cycle,
            turn_id: *turn_id,
            model_request_id: *model_request_id,
            effect_id: *effect_id,
            result_message_id: *message_id,
            result_digest: *result_digest,
        })),
        TerminalCandidate::Failed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            error,
        } => Ok(RecordBody::RunFailed(RunFailed {
            cycle: *cycle,
            turn_id: *turn_id,
            model_request_id: *model_request_id,
            effect_id: *effect_id,
            error: error.clone(),
        })),
    }
}

pub(super) fn expected_stage_cursor(state: &KernelState) -> Option<StageCursor> {
    let expected_stage = match state.phase? {
        RunPhase::BeforeRun => Stage::BeforeRun,
        RunPhase::PreparingContext => Stage::PrepareContext,
        RunPhase::BeforeModel => Stage::BeforeModel,
        RunPhase::AfterModel => Stage::AfterModel,
        RunPhase::BeforeToolBatch => Stage::BeforeToolBatch,
        RunPhase::AfterToolBatch => Stage::AfterToolBatch,
        RunPhase::BeforeFinalize => Stage::BeforeFinalize,
        _ => return None,
    };
    Some(StageCursor {
        cycle: state.cycle,
        stage: expected_stage,
    })
}

fn settlement_effect_id(outcome: &ModelSettlement) -> crate::EffectId {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.effect_id(),
        ModelSettlement::Deferred(deferred) => deferred.effect_id,
        ModelSettlement::Failed(failed) => failed.effect_id(),
    }
}

fn settlement_completion_id(outcome: &ModelSettlement) -> Option<&str> {
    match outcome {
        ModelSettlement::Completed { completion, .. } => completion.completion_id(),
        ModelSettlement::Deferred(_) => None,
        ModelSettlement::Failed(failed) => failed.completion_id(),
    }
}

pub(super) fn duplicate_decision(state: &KernelState) -> Result<Decision, KernelError> {
    let diagnostic = Diagnostic::try_new(
        "duplicate_settlement",
        "equal committed settlement was ignored",
        DiagnosticSeverity::Info,
        crate::Metadata::empty(),
    )
    .map_err(|_| KernelError::InvariantViolation)?;
    Ok(Decision::duplicate(next_sequence(state)?, diagnostic))
}

pub(super) fn reject_terminal(state: &KernelState) -> Result<(), KernelError> {
    if matches!(
        state.phase,
        Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled)
    ) || state.terminal.is_some()
    {
        return Err(KernelError::TerminalStateImmutable);
    }
    Ok(())
}

pub(super) fn next_sequence(state: &KernelState) -> Result<u64, KernelError> {
    state
        .last_applied_sequence
        .checked_add(1)
        .ok_or(KernelError::InvariantViolation)
}

pub(super) fn required<T: Copy>(
    values: &[T],
    index: usize,
    kind: &'static str,
) -> Result<T, KernelError> {
    values
        .get(index)
        .copied()
        .ok_or(KernelError::AllocatedIdsExhausted { kind })
}
