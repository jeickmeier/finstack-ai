use crate::digest::Digest;
use crate::entries::RunSuspended;
use crate::error::{ErrorCategory, ErrorCode, ErrorDescriptor};
use crate::limits::{LimitDimension, LimitReached, LimitUsage, LimitValue};
use crate::records::RecordBody;
use crate::state::{KernelState, RunPhase, TransitionEnv};

use super::super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::super::decision::{Decision, KernelError};
use super::super::failure_from_state;
use super::super::input::{
    ExternalEffectCompletedInput, ExternalEffectOutcome, KernelInput, ModelSettled,
    ModelSettlement, ReducerStageOutcome, StageSettled,
};
use super::{draft_for_state, next_sequence};

#[expect(
    clippy::too_many_lines,
    reason = "limit precedence and all frozen observation points stay visible in one pure decision"
)]
pub(super) fn decide_limit(
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
        | KernelInput::ToolBatchSettled(super::super::input::ToolBatchSettled {
            outcome: super::super::input::ToolSettlement::Completed(completion),
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
        | KernelInput::ToolBatchSettled(super::super::input::ToolBatchSettled {
            outcome: super::super::input::ToolSettlement::Completed(completion),
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
