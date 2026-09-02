use std::sync::Arc;

use crate::content::ContentBlock;
use crate::records::RecordBody;
use crate::state::{KernelState, RunPhase, TerminalCandidate, TransitionEnv};
use crate::{
    CapabilitiesActivated, FinalResultRecorded, OutputConfiguration, OutputEndStrategy, OutputSpec,
    OutputValidated, OutputValidationFailed, SUBMIT_FINAL_OUTPUT_TOOL, StructuredResultSource,
};

use super::super::allocated_ids::{IdRequirements, validate_allocated_ids};
use super::super::decision::{Decision, KernelError};
use super::super::validation::assistant_tool_calls;
use super::{decision_for, duplicate_decision, reject_terminal};

pub(super) fn decide_configure_output(
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
    decision_for(
        state,
        env,
        vec![RecordBody::OutputConfigured(configuration)],
        Vec::new(),
    )
}

pub(super) fn decide_capabilities_activated(
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
    if !matches!(
        state.phase,
        Some(RunPhase::BeforeRun | RunPhase::AfterToolBatch)
    ) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "capabilities_activated",
        });
    }
    if activation.prior_plan_digest != state.resolved_plan_digest {
        return Err(KernelError::ConflictingSettlement);
    }
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    decision_for(
        state,
        env,
        vec![RecordBody::CapabilitiesActivated(activation)],
        Vec::new(),
    )
}

pub(super) fn decide_output_validated(
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
    let application_calls = assistant_tool_calls(message, true)
        .into_iter()
        .map(|call| *call.tool_call_id())
        .collect::<Vec<_>>();
    let identity = OutputCandidateIdentity {
        cycle: *cycle,
        turn_id: *turn_id,
        model_request_id: *model_request_id,
        effect_id: *effect_id,
        message_id: *message_id,
    };
    let body = output_validation_body(state, input, identity, *end_strategy, application_calls)?;
    validate_allocated_ids(&env.ids, IdRequirements::new(1, 0, 0, 0, 0, 0))?;
    decision_for(state, env, vec![body], Vec::new())
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
            let error = crate::records::policy::expected_validation_error(
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
