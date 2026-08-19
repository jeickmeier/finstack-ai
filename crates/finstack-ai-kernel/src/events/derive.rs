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
        RunEventBody::EffectCancelled(cancelled) => {
            cancelled.output_contract().kind == EffectOutputKind::ModelResponse
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
        RunEventBody::EffectCancelled(cancelled) => {
            cancelled.output_contract().kind == EffectOutputKind::ToolResult
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
pub(crate) fn derived_event_kind(
    body: &RecordBody,
    kind_version: u16,
    ordinal: usize,
) -> Result<RunEventKind, EventError> {
    if kind_version != RECORD_KIND_VERSION {
        return Err(EventError::UnsupportedKindVersion { kind_version });
    }
    if ordinal != 0 && !matches!(body, RecordBody::ToolCallSettled(_)) {
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
        RecordBody::ToolCallSettled(_) => match ordinal {
            0 => RunEventKind::MessageFinalized,
            1 => RunEventKind::ToolSettled,
            _ => return Err(EventError::UnsupportedOrdinal { ordinal }),
        },
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

#[cfg(test)]
mod derived_event_kind_tests {
    use super::*;
    use crate::content::{ContentBlock, TextBlock};
    use crate::conversation::{Message, MessageRole, ProviderIds};
    use crate::effects::{
        EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectInput, EffectKind,
        EffectOutputContract, EffectRequested, InteractionCancelled, InteractionExpired,
        InteractionKind, InteractionRequest, InteractionResolution, ReconciliationPolicy,
        RetrySafety,
    };
    use crate::primitives::{
        AuthorizationEvidence, ComponentId, ComponentRef, Digest, ErrorCategory, ErrorCode,
        ErrorDescriptor, ExternalHandleRef, Metadata, PrincipalRef, RawJson, Timestamp, Version,
    };
    use crate::records::lifecycle::{
        ContextPrepared, EntryAppended, RetryClassification, RetryScheduled, RunCancelled,
        RunCompleted, RunFailed, RunSuspended, Stage, StageCursor, StageDisposition,
        StageOutcomeRecorded, TimerFired,
    };
    use crate::records::policy::{LimitDimension, LimitReached, LimitUsage, LimitValue};
    use crate::records::run::{
        BudgetPropagation, CancellationPropagation, CancellationReconciled, CancellationRequest,
        CancellationRequested, DeadlinePropagation, PrincipalPropagation, RunAccepted,
        RunPropagationPolicy, RunRelation, RunSecurityContext,
    };
    use crate::records::tools::{
        ToolBatchClosed, ToolBatchContinuation, ToolBatchOpened, ToolBatchOutcome, ToolCallSettled,
    };

    fn id<T: crate::primitives::IdTag>(ordinal: u64) -> crate::primitives::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::primitives::Id::from_bytes(bytes)
    }

    fn contract(kind: EffectOutputKind) -> EffectOutputContract {
        EffectOutputContract {
            kind,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        }
    }

    fn timestamp() -> Timestamp {
        Timestamp::from_unix_ms(0).expect("ts")
    }

    fn message() -> Message {
        Message::try_new(
            id::<crate::primitives::MessageTag>(1),
            MessageRole::Assistant,
            vec![ContentBlock::Text(TextBlock::try_new("hi").expect("text"))],
            timestamp(),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn run_accepted() -> RunAccepted {
        let run_id = id::<crate::primitives::RunTag>(1);
        RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("root"),
            RunSecurityContext::try_new(
                "tenant",
                PrincipalRef::try_new("iss", "sub", Some("tenant")).expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            crate::RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(b"{}"),
            None,
        )
        .expect("accepted")
    }

    fn requested() -> EffectRequested {
        EffectRequested::try_new(
            id::<crate::primitives::EffectTag>(1),
            EffectKind::Model,
            None,
            None,
            None,
            contract(EffectOutputKind::ModelResponse),
            EffectInput::Model {
                request: RawJson::parse("{}").expect("request"),
            },
            RetrySafety::SafeToRetry,
            None,
        )
        .expect("requested")
    }

    fn section_20_2_1_cases() -> Vec<(RecordBody, Vec<(usize, RunEventKind)>)> {
        let mut cases = section_20_2_1_effect_cases();
        cases.extend(section_20_2_1_interaction_cases());
        cases.extend(section_20_2_1_stage_and_tool_cases());
        cases.extend(section_20_2_1_terminal_cases());
        cases
    }

    fn section_20_2_1_effect_cases() -> Vec<(RecordBody, Vec<(usize, RunEventKind)>)> {
        let effect_id = id::<crate::primitives::EffectTag>(1);
        let error = ErrorDescriptor::new("failed", "failed", ErrorCategory::Internal, false)
            .expect("error");
        vec![
            (
                RecordBody::RunAccepted(run_accepted()),
                vec![(0, RunEventKind::RunAccepted)],
            ),
            (
                RecordBody::EffectRequested(requested()),
                vec![(0, RunEventKind::EffectRequested)],
            ),
            (
                RecordBody::EffectDeferred(EffectDeferred {
                    effect_id,
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.model.fixture").expect("component"),
                        "job",
                        RawJson::parse("{}").expect("meta"),
                    )
                    .expect("handle"),
                    reconciliation: ReconciliationPolicy::CallbackOrPoll,
                    next_poll_at: None,
                    expires_at: None,
                    output_contract: contract(EffectOutputKind::ModelResponse),
                }),
                vec![(0, RunEventKind::EffectDeferred)],
            ),
            (
                RecordBody::EffectCompleted(
                    EffectCompleted::try_new(
                        effect_id,
                        contract(EffectOutputKind::ModelResponse),
                        RawJson::parse("{}").expect("output"),
                        None,
                        vec![],
                        ProviderIds::empty(),
                        None::<&str>,
                        None,
                    )
                    .expect("completed"),
                ),
                vec![(0, RunEventKind::EffectCompleted)],
            ),
            (
                RecordBody::EffectFailed(
                    EffectFailed::try_new(
                        effect_id,
                        contract(EffectOutputKind::ModelResponse),
                        error,
                        None,
                        None::<&str>,
                    )
                    .expect("failed"),
                ),
                vec![(0, RunEventKind::EffectFailed)],
            ),
            (
                RecordBody::EffectCancelled(
                    EffectCancelled::try_new(
                        effect_id,
                        contract(EffectOutputKind::ModelResponse),
                        None::<&str>,
                        None::<&str>,
                    )
                    .expect("cancelled"),
                ),
                vec![(0, RunEventKind::EffectCancelled)],
            ),
        ]
    }

    fn section_20_2_1_interaction_cases() -> Vec<(RecordBody, Vec<(usize, RunEventKind)>)> {
        let effect_id = id::<crate::primitives::EffectTag>(1);
        let interaction_id = id::<crate::primitives::InteractionTag>(1);
        vec![
            (
                RecordBody::InteractionRequested(
                    InteractionRequest::try_new(
                        1,
                        interaction_id,
                        effect_id,
                        InteractionKind::Approval,
                        vec![],
                        RawJson::parse("{}").expect("schema"),
                        ComponentRef::new(
                            ComponentId::parse("policy.approval").expect("component"),
                            None,
                        ),
                        Version {
                            major: 1,
                            minor: 0,
                            patch: 0,
                        },
                        None,
                        None,
                        false,
                        Metadata::empty(),
                    )
                    .expect("request"),
                ),
                vec![(0, RunEventKind::InteractionRequested)],
            ),
            (
                RecordBody::InteractionResolved(
                    InteractionResolution::try_new(
                        interaction_id,
                        "resolution-1",
                        PrincipalRef::try_new("iss", "sub", Some("tenant")).expect("principal"),
                        AuthorizationEvidence::try_new("policy-v1", "decision-v1")
                            .expect("evidence"),
                        RawJson::parse("{}").expect("response"),
                        None::<&str>,
                    )
                    .expect("resolved"),
                ),
                vec![(0, RunEventKind::InteractionResolved)],
            ),
            (
                RecordBody::InteractionExpired(InteractionExpired {
                    interaction_id,
                    expired_at: timestamp(),
                }),
                vec![(0, RunEventKind::InteractionExpired)],
            ),
            (
                RecordBody::InteractionCancelled(
                    InteractionCancelled::try_new(interaction_id, None, None, None::<&str>)
                        .expect("cancelled"),
                ),
                vec![(0, RunEventKind::InteractionCancelled)],
            ),
        ]
    }

    fn section_20_2_1_stage_and_tool_cases() -> Vec<(RecordBody, Vec<(usize, RunEventKind)>)> {
        let effect_id = id::<crate::primitives::EffectTag>(1);
        vec![
            (
                RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
                    cursor: StageCursor {
                        cycle: 0,
                        stage: Stage::BeforeRun,
                    },
                    disposition: StageDisposition::Continued,
                    settlement_digest: Digest::raw_json(b"{}"),
                }),
                vec![],
            ),
            (
                RecordBody::ContextPrepared(
                    ContextPrepared::try_from_messages(
                        0,
                        id::<crate::primitives::TurnTag>(1),
                        vec![],
                    )
                    .expect("context"),
                ),
                vec![],
            ),
            (
                RecordBody::EntryAppended(EntryAppended {
                    cycle: 0,
                    turn_id: id::<crate::primitives::TurnTag>(1),
                    model_request_id: id::<crate::primitives::ModelRequestTag>(1),
                    effect_id,
                    parent_message_id: None,
                    message: message(),
                }),
                vec![(0, RunEventKind::MessageFinalized)],
            ),
            (
                RecordBody::ToolBatchOpened(ToolBatchOpened {
                    cycle: 0,
                    turn_id: id::<crate::primitives::TurnTag>(1),
                    tool_batch_id: id::<crate::primitives::ToolBatchTag>(1),
                    source_message_id: id::<crate::primitives::MessageTag>(1),
                    calls: std::sync::Arc::from([]),
                    continuation: ToolBatchContinuation::Finalize,
                    plan_digest: Digest::raw_json(b"{}"),
                }),
                vec![],
            ),
            (
                RecordBody::ToolCallSettled(ToolCallSettled {
                    cycle: 0,
                    turn_id: id::<crate::primitives::TurnTag>(1),
                    tool_batch_id: id::<crate::primitives::ToolBatchTag>(1),
                    tool_call_id: id::<crate::primitives::ToolCallTag>(1),
                    effect_id,
                    message: message(),
                    settlement_digest: Digest::raw_json(b"{}"),
                    synthetic: false,
                    error: None,
                }),
                vec![
                    (0, RunEventKind::MessageFinalized),
                    (1, RunEventKind::ToolSettled),
                ],
            ),
            (
                RecordBody::ToolBatchClosed(ToolBatchClosed {
                    cycle: 0,
                    turn_id: id::<crate::primitives::TurnTag>(1),
                    tool_batch_id: id::<crate::primitives::ToolBatchTag>(1),
                    source_message_id: id::<crate::primitives::MessageTag>(1),
                    result_message_ids: std::sync::Arc::from([]),
                    outcome: ToolBatchOutcome::Finalize,
                    close_digest: Digest::raw_json(b"{}"),
                }),
                vec![],
            ),
        ]
    }

    fn section_20_2_1_terminal_cases() -> Vec<(RecordBody, Vec<(usize, RunEventKind)>)> {
        let effect_id = id::<crate::primitives::EffectTag>(1);
        let error = ErrorDescriptor::new("failed", "failed", ErrorCategory::Internal, false)
            .expect("error");
        vec![
            (
                RecordBody::RunCompleted(RunCompleted {
                    cycle: 0,
                    turn_id: id::<crate::primitives::TurnTag>(1),
                    model_request_id: id::<crate::primitives::ModelRequestTag>(1),
                    effect_id,
                    result_message_id: id::<crate::primitives::MessageTag>(1),
                    result_digest: Digest::raw_json(b"{}"),
                }),
                vec![(0, RunEventKind::RunCompleted)],
            ),
            (
                RecordBody::RunFailed(RunFailed {
                    cycle: 0,
                    turn_id: Some(id::<crate::primitives::TurnTag>(1)),
                    model_request_id: Some(id::<crate::primitives::ModelRequestTag>(1)),
                    effect_id: Some(effect_id),
                    error: error.clone(),
                }),
                vec![(0, RunEventKind::RunFailed)],
            ),
            (
                RecordBody::CancellationRequested(CancellationRequested {
                    request: CancellationRequest::try_new(
                        id::<crate::primitives::CancellationRequestTag>(1),
                        crate::records::run::CancellationInitiator::RuntimeShutdown,
                        None::<&str>,
                    )
                    .expect("cancel"),
                }),
                vec![],
            ),
            (
                RecordBody::CancellationReconciled(CancellationReconciled {
                    request_id: id::<crate::primitives::CancellationRequestTag>(1),
                    completed_effects: std::sync::Arc::from([]),
                    cancelled_effects: std::sync::Arc::from([]),
                    uncertain_effects: std::sync::Arc::from([]),
                }),
                vec![],
            ),
            (
                RecordBody::LimitReached(LimitReached {
                    dimension: LimitDimension::Turns,
                    observed: LimitValue::Count(2),
                    maximum: LimitValue::Count(1),
                    usage: LimitUsage::default(),
                    usage_digest: LimitUsage::default().digest().expect("digest"),
                }),
                vec![(0, RunEventKind::LimitReached)],
            ),
            (
                RecordBody::RetryScheduled(
                    RetryScheduled::try_new(
                        0,
                        1,
                        RetryClassification::Model,
                        "policy-v1",
                        effect_id,
                        timestamp(),
                        error,
                    )
                    .expect("retry"),
                ),
                vec![],
            ),
            (
                RecordBody::TimerFired(TimerFired {
                    effect_id,
                    due_at: timestamp(),
                    fired_at: timestamp(),
                }),
                vec![],
            ),
            (
                RecordBody::RunSuspended(RunSuspended {
                    reason_code: ErrorCode::new("suspended").expect("code"),
                    cancellation_request_id: None,
                }),
                vec![(0, RunEventKind::RunSuspended)],
            ),
            (
                RecordBody::RunCancelled(RunCancelled {
                    request_id: id::<crate::primitives::CancellationRequestTag>(1),
                    reason_code: ErrorCode::new("cancelled").expect("code"),
                }),
                vec![(0, RunEventKind::RunCancelled)],
            ),
        ]
    }

    #[test]
    fn derived_event_kind_matches_section_20_2_1_ordinals() {
        for (body, expected) in section_20_2_1_cases() {
            assert_eq!(
                derived_event_kind(&body, 2, 0),
                Err(EventError::UnsupportedKindVersion { kind_version: 2 }),
                "{}",
                body.kind_name()
            );
            for (ordinal, kind) in &expected {
                assert_eq!(
                    derived_event_kind(&body, RECORD_KIND_VERSION, *ordinal),
                    Ok(*kind),
                    "{} ordinal {ordinal}",
                    body.kind_name()
                );
            }
            let next = expected.last().map_or(0, |(ordinal, _)| ordinal + 1);
            assert_eq!(
                derived_event_kind(&body, RECORD_KIND_VERSION, next),
                Err(EventError::UnsupportedOrdinal { ordinal: next }),
                "{} ordinal {next}",
                body.kind_name()
            );
        }
    }
}
