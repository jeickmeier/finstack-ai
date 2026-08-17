//! Record version, pairing, and creation validation.

use crate::effects::{
    EffectInput, EffectKind, EffectOutputKind, EffectRequested, InteractionRequest,
};
use crate::primitives::RunId;

use super::body::RecordBody;
use super::draft::RecordDraft;
use super::error::RecordError;
use super::{RECORD_FORMAT_VERSION, RECORD_KIND_VERSION};
use crate::records::SessionRecordError;
use crate::records::tools::ToolBatchOutcome;

pub(super) fn validate_versions_and_events(
    format_version: u16,
    kind_version: u16,
    actual_event_count: usize,
    body: &RecordBody,
) -> Result<(), RecordError> {
    if format_version != RECORD_FORMAT_VERSION {
        return Err(RecordError::UnsupportedFormatVersion { format_version });
    }
    if kind_version != RECORD_KIND_VERSION {
        return Err(RecordError::UnsupportedKindVersion { kind_version });
    }
    let expected = body.derived_event_count(kind_version)?;
    if actual_event_count != expected {
        return Err(RecordError::DerivedEventCount {
            expected,
            actual: actual_event_count,
        });
    }
    Ok(())
}

pub(super) fn validate_interaction_request_pairs(
    records: &[RecordDraft],
) -> Result<(), RecordError> {
    for record in records {
        match record.body() {
            RecordBody::InteractionRequested(request) => {
                let matches = records
                    .iter()
                    .filter(|candidate| {
                        let RecordBody::EffectRequested(effect) = candidate.body() else {
                            return false;
                        };
                        interaction_request_matches_effect(request, effect)
                    })
                    .count();
                if matches != 1 {
                    return Err(RecordError::InvalidInteractionPair);
                }
            }
            RecordBody::EffectRequested(effect) if effect.kind() == EffectKind::Interaction => {
                let matches = records
                    .iter()
                    .filter(|candidate| {
                        let RecordBody::InteractionRequested(request) = candidate.body() else {
                            return false;
                        };
                        interaction_request_matches_effect(request, effect)
                    })
                    .count();
                if matches != 1 {
                    return Err(RecordError::InvalidInteractionPair);
                }
            }
            _ => {}
        }
    }
    let resolved = records
        .iter()
        .filter(|record| matches!(record.body(), RecordBody::InteractionResolved(_)))
        .count();
    let completed = records
        .iter()
        .filter(|record| {
            matches!(
                record.body(),
                RecordBody::EffectCompleted(effect)
                    if effect.output_contract().kind == EffectOutputKind::InteractionResolution
            )
        })
        .count();
    let expired = records
        .iter()
        .filter(|record| matches!(record.body(), RecordBody::InteractionExpired(_)))
        .count();
    let failed = records
        .iter()
        .filter(|record| {
            matches!(
                record.body(),
                RecordBody::EffectFailed(effect)
                    if effect.output_contract().kind == EffectOutputKind::InteractionResolution
            )
        })
        .count();
    let cancelled = records
        .iter()
        .filter(|record| matches!(record.body(), RecordBody::InteractionCancelled(_)))
        .count();
    let effect_cancelled = records
        .iter()
        .filter(|record| {
            matches!(
                record.body(),
                RecordBody::EffectCancelled(effect)
                    if effect.output_contract().kind == EffectOutputKind::InteractionResolution
            )
        })
        .count();
    if resolved != completed || expired != failed || cancelled != effect_cancelled {
        return Err(RecordError::InvalidInteractionPair);
    }
    Ok(())
}

fn interaction_request_matches_effect(
    request: &InteractionRequest,
    effect: &EffectRequested,
) -> bool {
    if effect.effect_id() != request.effect_id() || effect.kind() != EffectKind::Interaction {
        return false;
    }
    let EffectInput::Interaction {
        interaction_id,
        request_digest,
    } = effect.input()
    else {
        return false;
    };
    *interaction_id == request.interaction_id()
        && request
            .request_digest()
            .is_ok_and(|digest| digest == *request_digest)
}

pub(super) fn validate_body_for_creation(body: &RecordBody) -> Result<(), RecordError> {
    if let RecordBody::RunAccepted(accepted) = body
        && !accepted.lineage_is_validated()
    {
        return Err(RecordError::UnvalidatedRunLineage);
    }
    let error = match body {
        RecordBody::StageOutcomeRecorded(outcome) => match &outcome.disposition {
            crate::StageDisposition::Failed { error } => Some(error),
            _ => None,
        },
        RecordBody::EffectFailed(failed) => Some(failed.error()),
        RecordBody::ToolCallSettled(settled) => settled.error.as_ref(),
        RecordBody::ToolBatchClosed(closed) => match &closed.outcome {
            ToolBatchOutcome::Failed { error } => Some(error),
            ToolBatchOutcome::ContinueModel | ToolBatchOutcome::Finalize => None,
        },
        RecordBody::RunFailed(failed) => Some(&failed.error),
        RecordBody::OutputValidationFailed(failed) => Some(&failed.error),
        RecordBody::RetryScheduled(retry) => Some(&retry.prior_error),
        _ => None,
    };
    if let Some(error) = error {
        error
            .validate()
            .map_err(RecordError::InvalidErrorDescriptor)?;
    }
    match body {
        RecordBody::BudgetReservationRequested(value) => value
            .request
            .validate()
            .map_err(|_| RecordError::InvalidBudgetRecord)?,
        RecordBody::BudgetReservationSettled(value) => value
            .receipt
            .validate()
            .map_err(|_| RecordError::InvalidBudgetRecord)?,
        RecordBody::BudgetChargeRecorded(value) => value
            .receipt
            .validate()
            .map_err(|_| RecordError::InvalidBudgetRecord)?,
        RecordBody::BudgetReservationReleased(value) => value
            .receipt
            .validate()
            .map_err(|_| RecordError::InvalidBudgetRecord)?,
        _ => {}
    }
    Ok(())
}

pub(super) fn validate_record_run_id(
    run_id: Option<RunId>,
    body: &RecordBody,
) -> Result<(), RecordError> {
    if let RecordBody::RunAccepted(accepted) = body
        && run_id != Some(accepted.run_id())
    {
        return Err(RecordError::RecordRunMismatch);
    }
    if matches!(
        body,
        RecordBody::SessionCreated(_)
            | RecordBody::LaneCreated(_)
            | RecordBody::LaneMoved(_)
            | RecordBody::SnapshotWritten(_)
            | RecordBody::ConversationEntry(_)
    ) && run_id.is_some()
    {
        return Err(RecordError::Session(
            SessionRecordError::StructuralRunIdPresent,
        ));
    }
    Ok(())
}
