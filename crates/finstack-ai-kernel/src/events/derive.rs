//! Durable-derived event policy, correlations, and record mapping.

use std::sync::Arc;

use crate::effects::EffectOutputKind;
use crate::primitives::Sensitivity;
use crate::primitives::{EffectId, ModelRequestId, RunId, ToolBatchId, ToolCallId, TurnId};
use crate::records::{RECORD_KIND_VERSION, RecordBody};

use super::body::RunEventBody;
use super::envelope::{EventCorrelations, ResolvedEventCorrelations};
use super::{EventError, RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RunEventKind};

pub(super) fn validate_event_policy(
    turn_id: Option<TurnId>,
    model_request_id: Option<ModelRequestId>,
    tool_batch_id: Option<ToolBatchId>,
    effect_id: Option<EffectId>,
    tool_call_id: Option<ToolCallId>,
    sensitivity: Sensitivity,
    body: &RunEventBody,
) -> Result<(), EventError> {
    match body {
        RunEventBody::MessageFinalized { .. }
            if turn_id.is_none()
                || effect_id.is_none()
                || (model_request_id.is_none()
                    && (tool_batch_id.is_none() || tool_call_id.is_none()))
                || sensitivity != Sensitivity::Internal =>
        {
            return Err(EventError::CorrelationMismatch {
                reason: "message event requires model or tool correlations and internal sensitivity",
            });
        }
        RunEventBody::ToolSettled { .. }
            if turn_id.is_none()
                || tool_batch_id.is_none()
                || effect_id.is_none()
                || tool_call_id.is_none()
                || sensitivity != Sensitivity::Internal =>
        {
            return Err(EventError::CorrelationMismatch {
                reason: "tool-settled event requires turn, batch, effect, call, and internal sensitivity",
            });
        }
        RunEventBody::RunCompleted { .. }
            if turn_id.is_none()
                || model_request_id.is_none()
                || effect_id.is_none()
                || sensitivity != Sensitivity::Internal =>
        {
            return Err(EventError::CorrelationMismatch {
                reason: "run-completed event requires model correlations and internal sensitivity",
            });
        }
        RunEventBody::RunFailed { .. } if sensitivity != Sensitivity::Internal => {
            return Err(EventError::CorrelationMismatch {
                reason: "run failed event requires internal sensitivity",
            });
        }
        _ => {}
    }
    let model_event = match body {
        RunEventBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Model,
        RunEventBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ModelResponse
        }
        RunEventBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RunEventBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RunEventBody::ModelTextDelta(_) | RunEventBody::ReasoningDelta(_) => true,
        _ => false,
    };
    if model_event
        && (turn_id.is_none()
            || model_request_id.is_none()
            || effect_id.is_none()
            || sensitivity != Sensitivity::Confidential)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "model event requires turn_id, model_request_id, effect_id, and confidential sensitivity",
        });
    }
    let tool_event = match body {
        RunEventBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Tool,
        RunEventBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ToolResult
        }
        RunEventBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RunEventBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RunEventBody::ToolProgress(_) => true,
        _ => false,
    };
    if tool_event
        && (turn_id.is_none()
            || tool_batch_id.is_none()
            || effect_id.is_none()
            || tool_call_id.is_none()
            || sensitivity != Sensitivity::Confidential)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "tool event requires turn_id, tool_batch_id, effect_id, tool_call_id, and confidential sensitivity",
        });
    }
    Ok(())
}

pub(super) fn validate_event_versions(
    schema_version: u16,
    kind_version: u16,
) -> Result<(), EventError> {
    if schema_version != RUN_EVENT_SCHEMA_VERSION {
        return Err(EventError::UnsupportedSchemaVersion { schema_version });
    }
    if kind_version != RUN_EVENT_KIND_VERSION {
        return Err(EventError::UnsupportedKindVersion { kind_version });
    }
    Ok(())
}

pub(super) fn validate_event_correlations(
    run_id: RunId,
    model_request_id: Option<ModelRequestId>,
    effect_id: Option<EffectId>,
    tool_call_id: Option<ToolCallId>,
    body: &RunEventBody,
) -> Result<(), EventError> {
    if let RunEventBody::RunAccepted(accepted) = body {
        if !accepted.lineage_is_validated() {
            return Err(EventError::CorrelationMismatch {
                reason: "run-accepted lineage has not been validated",
            });
        }
        if accepted.run_id() != run_id {
            return Err(EventError::CorrelationMismatch {
                reason: "run-accepted event run_id does not match body",
            });
        }
    }
    if let Some(expected) = effect_id_for_body(body)
        && effect_id != Some(expected)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "effect correlation does not match body",
        });
    }
    if let RunEventBody::ToolSettled {
        tool_call_id: expected,
    } = body
        && tool_call_id != Some(*expected)
    {
        return Err(EventError::CorrelationMismatch {
            reason: "tool-call correlation does not match body",
        });
    }
    if matches!(
        body,
        RunEventBody::ModelTextDelta(_) | RunEventBody::ReasoningDelta(_)
    ) && model_request_id.is_none()
    {
        return Err(EventError::CorrelationMismatch {
            reason: "model delta requires model_request_id",
        });
    }
    if matches!(body, RunEventBody::ToolProgress(_)) && tool_call_id.is_none() {
        return Err(EventError::CorrelationMismatch {
            reason: "tool progress requires tool_call_id",
        });
    }
    Ok(())
}

pub(super) fn effect_id_for_body(body: &RunEventBody) -> Option<EffectId> {
    match body {
        RunEventBody::EffectRequested(value) => Some(value.effect_id()),
        RunEventBody::EffectDeferred(value) => Some(value.effect_id),
        RunEventBody::EffectCompleted(value) => Some(value.effect_id()),
        RunEventBody::EffectFailed(value) => Some(value.effect_id()),
        RunEventBody::EffectCancelled(value) => Some(value.effect_id()),
        RunEventBody::InteractionRequested(value) => Some(value.effect_id()),
        _ => None,
    }
}

pub(super) fn run_event_body_from_record(
    body: &RecordBody,
    ordinal: usize,
) -> Result<RunEventBody, EventError> {
    let event = match body {
        RecordBody::RunAccepted(value) => RunEventBody::RunAccepted(value.clone()),
        RecordBody::EffectRequested(value) => RunEventBody::EffectRequested(value.clone()),
        RecordBody::EffectDeferred(value) => RunEventBody::EffectDeferred(value.clone()),
        RecordBody::EffectCompleted(value) => RunEventBody::EffectCompleted(value.clone()),
        RecordBody::EffectFailed(value) => RunEventBody::EffectFailed(value.clone()),
        RecordBody::EffectCancelled(value) => RunEventBody::EffectCancelled(value.clone()),
        RecordBody::InteractionRequested(value) => {
            RunEventBody::InteractionRequested(value.clone())
        }
        RecordBody::InteractionResolved(value) => RunEventBody::InteractionResolved(value.clone()),
        RecordBody::InteractionExpired(value) => RunEventBody::InteractionExpired(value.clone()),
        RecordBody::InteractionCancelled(value) => {
            RunEventBody::InteractionCancelled(value.clone())
        }
        RecordBody::EntryAppended(value) => RunEventBody::MessageFinalized {
            message_id: *value.message.id(),
        },
        RecordBody::ToolCallSettled(value) if ordinal == 0 => RunEventBody::MessageFinalized {
            message_id: *value.message.id(),
        },
        RecordBody::ToolCallSettled(value) if ordinal == 1 => RunEventBody::ToolSettled {
            tool_call_id: value.tool_call_id,
        },
        RecordBody::RunCompleted(value) => RunEventBody::RunCompleted {
            result_digest: value.result_digest,
        },
        RecordBody::RunFailed(value) => RunEventBody::RunFailed {
            error: value.error.clone(),
        },
        RecordBody::LimitReached(value) => RunEventBody::LimitReached {
            dimension: value.dimension.clone(),
        },
        RecordBody::RunSuspended(value) => RunEventBody::RunSuspended {
            reason_code: Some(Arc::from(value.reason_code.as_str())),
        },
        RecordBody::RunCancelled(value) => RunEventBody::RunCancelled {
            request_id: Some(value.request_id),
        },
        RecordBody::StageOutcomeRecorded(_)
        | RecordBody::ContextPrepared(_)
        | RecordBody::ToolBatchOpened(_)
        | RecordBody::ToolBatchClosed(_)
        | RecordBody::CancellationRequested(_)
        | RecordBody::CancellationReconciled(_)
        | RecordBody::RetryScheduled(_)
        | RecordBody::TimerFired(_)
        | RecordBody::OutputConfigured(_)
        | RecordBody::CapabilitiesActivated(_)
        | RecordBody::FinalResultRecorded(_)
        | RecordBody::OutputValidationFailed(_)
        | RecordBody::ExternalCommandRejected(_)
        | RecordBody::ChildRunPrepared(_)
        | RecordBody::BudgetReservationRequested(_)
        | RecordBody::BudgetReservationSettled(_)
        | RecordBody::BudgetChargeRecorded(_)
        | RecordBody::BudgetReservationReleased(_)
        | RecordBody::SessionCreated(_)
        | RecordBody::LaneCreated(_)
        | RecordBody::LaneMoved(_)
        | RecordBody::SnapshotWritten(_)
        | RecordBody::ConversationEntry(_)
        | RecordBody::ToolCallSettled(_) => {
            return Err(EventError::UnsupportedOrdinal { ordinal });
        }
    };
    Ok(event)
}

pub(super) fn record_correlations(
    body: &RecordBody,
    correlations: EventCorrelations,
    body_effect_id: Option<EffectId>,
) -> ResolvedEventCorrelations {
    match body {
        RecordBody::EntryAppended(value) => ResolvedEventCorrelations {
            turn: Some(value.turn_id),
            model_request: Some(value.model_request_id),
            tool_batch: None,
            effect: Some(value.effect_id),
            tool_call: None,
        },
        RecordBody::ToolCallSettled(value) => ResolvedEventCorrelations {
            turn: Some(value.turn_id),
            model_request: None,
            tool_batch: Some(value.tool_batch_id),
            effect: Some(value.effect_id),
            tool_call: Some(value.tool_call_id),
        },
        RecordBody::RunCompleted(value) => ResolvedEventCorrelations {
            turn: Some(value.turn_id),
            model_request: Some(value.model_request_id),
            tool_batch: None,
            effect: Some(value.effect_id),
            tool_call: None,
        },
        RecordBody::RunFailed(value) => ResolvedEventCorrelations {
            turn: value.turn_id,
            model_request: value.model_request_id,
            tool_batch: None,
            effect: value.effect_id,
            tool_call: None,
        },
        RecordBody::RunCancelled(_) | RecordBody::RunSuspended(_) | RecordBody::LimitReached(_) => {
            ResolvedEventCorrelations {
                turn: correlations.model_turn,
                model_request: correlations.model_request,
                tool_batch: correlations.tool_batch,
                effect: body_effect_id,
                tool_call: correlations.tool_call,
            }
        }
        RecordBody::EffectRequested(_)
        | RecordBody::EffectDeferred(_)
        | RecordBody::EffectCompleted(_)
        | RecordBody::EffectFailed(_)
        | RecordBody::EffectCancelled(_)
            if is_tool_effect_record(body) =>
        {
            ResolvedEventCorrelations {
                turn: correlations.tool_turn,
                model_request: None,
                tool_batch: correlations.tool_batch,
                effect: body_effect_id,
                tool_call: correlations.tool_call,
            }
        }
        RecordBody::EffectRequested(_)
        | RecordBody::EffectDeferred(_)
        | RecordBody::EffectCompleted(_)
        | RecordBody::EffectFailed(_)
        | RecordBody::EffectCancelled(_) => ResolvedEventCorrelations {
            turn: correlations.model_turn,
            model_request: correlations.model_request,
            tool_batch: None,
            effect: body_effect_id,
            tool_call: None,
        },
        _ => ResolvedEventCorrelations {
            turn: None,
            model_request: None,
            tool_batch: None,
            effect: body_effect_id,
            tool_call: None,
        },
    }
}

pub(super) fn derived_event_sensitivity(body: &RecordBody) -> Sensitivity {
    if is_model_effect_record(body) || is_tool_effect_record(body) {
        Sensitivity::Confidential
    } else {
        Sensitivity::Internal
    }
}

pub(super) fn is_tool_effect_record(body: &RecordBody) -> bool {
    match body {
        RecordBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Tool,
        RecordBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ToolResult
        }
        RecordBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RecordBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ToolResult
        }
        RecordBody::EffectCancelled(cancelled) => {
            cancelled.output_contract().kind == EffectOutputKind::ToolResult
        }
        _ => false,
    }
}

pub(super) fn is_model_effect_record(body: &RecordBody) -> bool {
    match body {
        RecordBody::EffectRequested(requested) => requested.kind() == crate::EffectKind::Model,
        RecordBody::EffectDeferred(deferred) => {
            deferred.output_contract.kind == EffectOutputKind::ModelResponse
        }
        RecordBody::EffectCompleted(completed) => {
            completed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RecordBody::EffectFailed(failed) => {
            failed.output_contract().kind == EffectOutputKind::ModelResponse
        }
        RecordBody::EffectCancelled(cancelled) => {
            cancelled.output_contract().kind == EffectOutputKind::ModelResponse
        }
        _ => false,
    }
}
/// Map a record body to derived event kinds by ordinal (`kind_version` = 1).
///
/// # Arguments
///
/// * `body` - Committed record body whose derived-event table is consulted.
/// * `kind_version` - Body kind version. Only version 1 is supported.
/// * `ordinal` - Zero-based derived-event ordinal for that body.
///
/// # Errors
///
/// Returns [`EventError::UnsupportedOrdinal`] when the ordinal is out of range.
pub fn derived_event_kind(
    body: &RecordBody,
    kind_version: u16,
    ordinal: usize,
) -> Result<RunEventKind, EventError> {
    if kind_version != RECORD_KIND_VERSION {
        return Err(EventError::UnsupportedKindVersion { kind_version });
    }
    if let RecordBody::ToolCallSettled(_) = body {
        return match ordinal {
            0 => Ok(RunEventKind::MessageFinalized),
            1 => Ok(RunEventKind::ToolSettled),
            _ => Err(EventError::UnsupportedOrdinal { ordinal }),
        };
    }
    if ordinal != 0 {
        return Err(EventError::UnsupportedOrdinal { ordinal });
    }
    let kind = match body {
        RecordBody::RunAccepted(_) => RunEventKind::RunAccepted,
        RecordBody::EffectRequested(_) => RunEventKind::EffectRequested,
        RecordBody::EffectDeferred(_) => RunEventKind::EffectDeferred,
        RecordBody::EffectCompleted(_) => RunEventKind::EffectCompleted,
        RecordBody::EffectFailed(_) => RunEventKind::EffectFailed,
        RecordBody::EffectCancelled(_) => RunEventKind::EffectCancelled,
        RecordBody::InteractionRequested(_) => RunEventKind::InteractionRequested,
        RecordBody::InteractionResolved(_) => RunEventKind::InteractionResolved,
        RecordBody::InteractionExpired(_) => RunEventKind::InteractionExpired,
        RecordBody::InteractionCancelled(_) => RunEventKind::InteractionCancelled,
        RecordBody::EntryAppended(_) => RunEventKind::MessageFinalized,
        RecordBody::ToolCallSettled(_) => unreachable!("handled above"),
        RecordBody::RunCompleted(_) => RunEventKind::RunCompleted,
        RecordBody::RunFailed(_) => RunEventKind::RunFailed,
        RecordBody::LimitReached(_) => RunEventKind::LimitReached,
        RecordBody::RunSuspended(_) => RunEventKind::RunSuspended,
        RecordBody::RunCancelled(_) => RunEventKind::RunCancelled,
        RecordBody::StageOutcomeRecorded(_)
        | RecordBody::ContextPrepared(_)
        | RecordBody::ToolBatchOpened(_)
        | RecordBody::ToolBatchClosed(_)
        | RecordBody::CancellationRequested(_)
        | RecordBody::CancellationReconciled(_)
        | RecordBody::RetryScheduled(_)
        | RecordBody::TimerFired(_)
        | RecordBody::OutputConfigured(_)
        | RecordBody::CapabilitiesActivated(_)
        | RecordBody::FinalResultRecorded(_)
        | RecordBody::OutputValidationFailed(_)
        | RecordBody::ExternalCommandRejected(_)
        | RecordBody::ChildRunPrepared(_)
        | RecordBody::BudgetReservationRequested(_)
        | RecordBody::BudgetReservationSettled(_)
        | RecordBody::BudgetChargeRecorded(_)
        | RecordBody::BudgetReservationReleased(_)
        | RecordBody::SessionCreated(_)
        | RecordBody::LaneCreated(_)
        | RecordBody::LaneMoved(_)
        | RecordBody::SnapshotWritten(_)
        | RecordBody::ConversationEntry(_) => {
            return Err(EventError::UnsupportedOrdinal { ordinal });
        }
    };
    Ok(kind)
}
