use crate::content::ContentBlock;
use crate::state::{KernelState, TerminalCandidate};

use super::super::decision::KernelError;

pub(super) fn apply_final_result(
    state: &mut KernelState,
    result: &crate::FinalResultRecorded,
) -> Result<(), KernelError> {
    result
        .validate()
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    let Some(crate::OutputConfiguration {
        output: crate::OutputSpec::JsonSchema { schema },
        end_strategy,
    }) = state.output_configuration.as_ref()
    else {
        return Err(KernelError::InvalidRecordOrder);
    };
    let candidate_matches = matches!(
        state.terminal_candidate.as_ref(),
        Some(TerminalCandidate::Completed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            ..
        }) if result.cycle == *cycle
            && result.turn_id == *turn_id
            && result.model_request_id == *model_request_id
            && result.effect_id == *effect_id
            && result.message_id == *message_id
    );
    let message = state
        .messages
        .last()
        .filter(|message| *message.id() == result.message_id)
        .ok_or(KernelError::InvalidRecordOrder)?;
    let expected_skipped = if *end_strategy == crate::OutputEndStrategy::Early {
        application_tool_ids(message)
    } else {
        Vec::new()
    };
    if !candidate_matches
        || &result.schema != schema
        || result.end_strategy != *end_strategy
        || result.value_digest != result.value.digest()
        || structured_source_value(message, &result.source) != Some(&result.value)
        || result.skipped_tool_call_ids.as_ref() != expected_skipped.as_slice()
        || state.final_result.is_some()
        || state.validation_failure.is_some()
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    state.terminal_candidate = Some(TerminalCandidate::Completed {
        cycle: result.cycle,
        turn_id: result.turn_id,
        model_request_id: result.model_request_id,
        effect_id: result.effect_id,
        message_id: result.message_id,
        result_digest: result.value_digest,
    });
    state.final_result = Some(result.clone());
    state.state_version = 4;
    Ok(())
}

pub(super) fn apply_validation_failure(
    state: &mut KernelState,
    failure: &crate::OutputValidationFailed,
) -> Result<(), KernelError> {
    failure
        .validate()
        .map_err(|_| KernelError::InvalidRecordOrder)?;
    let Some(crate::OutputConfiguration {
        output: crate::OutputSpec::JsonSchema { schema },
        ..
    }) = state.output_configuration.as_ref()
    else {
        return Err(KernelError::InvalidRecordOrder);
    };
    let candidate_matches = matches!(
        state.terminal_candidate.as_ref(),
        Some(TerminalCandidate::Completed {
            cycle,
            turn_id,
            model_request_id,
            effect_id,
            message_id,
            ..
        }) if failure.cycle == *cycle
            && failure.turn_id == *turn_id
            && failure.model_request_id == *model_request_id
            && failure.effect_id == *effect_id
            && failure.message_id == *message_id
    );
    let message = state
        .messages
        .last()
        .filter(|message| *message.id() == failure.message_id)
        .ok_or(KernelError::InvalidRecordOrder)?;
    let expected_error = crate::validation::expected_validation_error(
        state.retry.attempts,
        state
            .accepted
            .as_ref()
            .and_then(|accepted| accepted.limits().max_retries),
    )
    .map_err(|_| KernelError::InvalidInputPayload {
        field: "validation_error",
        reason_code: "expected_error_unavailable",
    })?;
    if !candidate_matches
        || &failure.schema != schema
        || failure.issues.is_empty()
        || failure.error != expected_error
        || structured_source_value(message, &failure.source)
            .is_none_or(|value| value.digest() != failure.candidate_digest)
        || failure.skipped_tool_call_ids.as_ref() != application_tool_ids(message).as_slice()
        || state.final_result.is_some()
        || state.validation_failure.is_some()
    {
        return Err(KernelError::InvalidRecordOrder);
    }
    state.terminal_candidate = Some(TerminalCandidate::Failed {
        cycle: failure.cycle,
        turn_id: Some(failure.turn_id),
        model_request_id: Some(failure.model_request_id),
        effect_id: Some(failure.effect_id),
        error: failure.error.clone(),
    });
    state.validation_failure = Some(failure.clone());
    state.state_version = 4;
    Ok(())
}

pub(super) fn structured_source_value<'a>(
    message: &'a crate::Message,
    source: &crate::StructuredResultSource,
) -> Option<&'a crate::RawJson> {
    match source {
        crate::StructuredResultSource::JsonBlock { content_index } => {
            usize::try_from(*content_index)
                .ok()
                .and_then(|index| message.content().get(index))
                .and_then(|block| match block {
                    ContentBlock::Json(value) => Some(value.value()),
                    _ => None,
                })
        }
        crate::StructuredResultSource::InternalTool { tool_call_id } => {
            message.content().iter().find_map(|block| match block {
                ContentBlock::ToolCall(call)
                    if call.tool_call_id() == tool_call_id
                        && call.tool_name() == crate::SUBMIT_FINAL_OUTPUT_TOOL =>
                {
                    Some(call.arguments())
                }
                _ => None,
            })
        }
    }
}

pub(super) fn application_tool_ids(message: &crate::Message) -> Vec<crate::ToolCallId> {
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
