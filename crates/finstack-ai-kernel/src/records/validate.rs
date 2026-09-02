//! Record version, pairing, and creation validation.

use crate::effects::{
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested,
    InteractionRequest,
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
        let matches = match record.body() {
            RecordBody::InteractionRequested(request) => records
                .iter()
                .filter(|candidate| {
                    matches!(candidate.body(), RecordBody::EffectRequested(effect)
                        if interaction_request_matches_effect(request, effect))
                })
                .count(),
            RecordBody::EffectRequested(effect) if effect.kind() == EffectKind::Interaction => {
                records
                    .iter()
                    .filter(|candidate| {
                        matches!(candidate.body(), RecordBody::InteractionRequested(request)
                            if interaction_request_matches_effect(request, effect))
                    })
                    .count()
            }
            _ => continue,
        };
        if matches != 1 {
            return Err(RecordError::InvalidInteractionPair);
        }
    }
    let resolves_interaction =
        |contract: &EffectOutputContract| contract.kind == EffectOutputKind::InteractionResolution;
    let (mut resolved, mut completed) = (0_usize, 0_usize);
    let (mut expired, mut failed) = (0_usize, 0_usize);
    let (mut cancelled, mut effect_cancelled) = (0_usize, 0_usize);
    for record in records {
        match record.body() {
            RecordBody::InteractionResolved(_) => resolved += 1,
            RecordBody::EffectCompleted(effect)
                if resolves_interaction(effect.output_contract()) =>
            {
                completed += 1;
            }
            RecordBody::InteractionExpired(_) => expired += 1,
            RecordBody::EffectFailed(effect) if resolves_interaction(effect.output_contract()) => {
                failed += 1;
            }
            RecordBody::InteractionCancelled(_) => cancelled += 1,
            RecordBody::EffectCancelled(effect)
                if resolves_interaction(effect.output_contract()) =>
            {
                effect_cancelled += 1;
            }
            _ => {}
        }
    }
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
    let budget = match body {
        RecordBody::BudgetReservationRequested(value) => value.request.validate(),
        RecordBody::BudgetReservationSettled(value) => value.receipt.validate(),
        RecordBody::BudgetChargeRecorded(value) => value.receipt.validate(),
        RecordBody::BudgetReservationReleased(value) => value.receipt.validate(),
        _ => Ok(()),
    };
    budget.map_err(|_| RecordError::InvalidBudgetRecord)
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
    if body.is_structural() && run_id.is_some() {
        return Err(RecordError::Session(
            SessionRecordError::StructuralRunIdPresent,
        ));
    }
    Ok(())
}
