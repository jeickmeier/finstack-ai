use std::collections::BTreeMap;

use super::apply;
use crate::content::ContentBlock;
use crate::conversation::{Message, MessageRole, ProviderIds};
use crate::effects::{
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested,
    ReconciliationPolicy, RetrySafety,
};
use crate::primitives::ComponentId;
use crate::primitives::Digest;
use crate::primitives::ExternalHandleRef;
use crate::primitives::Timestamp;
use crate::primitives::{Metadata, RawJson};
use crate::records::lifecycle::EntryAppended;
use crate::records::{
    APPEND_BATCH_MAX_RECORDS, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
    RecordEnvelope,
};
use crate::state::{
    KernelState, ModelSettlementFingerprint, ModelSettlementKind, PendingModelEffect, RunPhase,
};
use crate::{CommittedBatch, KernelError, TextBlock};

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end replay fixture keeps preparation, settlement, round-trip, and conflict evidence together"
)]
fn child_preparation_and_budget_replay_are_idempotent_and_conflict_closed() {
    let timestamp = Timestamp::from_unix_ms(1_000).expect("timestamp");
    let session_id = fixed_id::<crate::SessionTag>(10);
    let lane_id = fixed_id::<crate::LaneTag>(11);
    let parent_run_id = fixed_id::<crate::RunTag>(12);
    let parent_effect_id = fixed_id::<crate::EffectTag>(13);
    let child_run_id = fixed_id::<crate::RunTag>(14);
    let reservation_id = fixed_id::<crate::BudgetReservationTag>(15);
    let scope_id = fixed_id::<crate::BudgetScopeTag>(16);
    let accepted = crate::RunAccepted::try_new(
        parent_run_id,
        crate::RunRelation::root(parent_run_id).expect("root relation"),
        crate::RunSecurityContext::try_new(
            "tenant-a",
            crate::PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        None,
        crate::RunLimits::empty(),
        crate::RunPropagationPolicy {
            cancellation: crate::CancellationPropagation::Cascade,
            deadline: crate::DeadlinePropagation::MinimumOfParentAndChild,
            budget: crate::BudgetPropagation::ReservedChildAllocation,
            principal: crate::PrincipalPropagation::Inherit,
        },
        Digest::raw_json(b"agent-lock"),
        None,
    )
    .expect("accepted");
    let state = KernelState {
        session_id: Some(session_id),
        lane_id: Some(lane_id),
        accepted: Some(accepted),
        accepted_at: Some(timestamp),
        phase: Some(RunPhase::BeforeRun),
        ..KernelState::default()
    };
    let amount = crate::BudgetRequest {
        input_tokens: Some(100),
        output_tokens: Some(20),
        cost: None,
        extension_counters: BTreeMap::new(),
    };
    let request_digest = crate::BudgetReserveRequest::compute_digest(
        scope_id,
        reservation_id,
        child_run_id,
        &amount,
    )
    .expect("request digest");
    let request = crate::BudgetReserveRequest {
        scope_id,
        reservation_id,
        run_id: child_run_id,
        amount: amount.clone(),
        request_digest,
    };
    let prepared = crate::ChildRunPrepared {
        parent_run_id,
        parent_effect_id,
        child: crate::ChildRunLocator {
            operation: crate::OperationLocator::try_new(
                "tenant-a",
                session_id,
                fixed_id::<crate::LaneTag>(17),
                child_run_id,
            )
            .expect("child locator"),
            remote: None,
        },
        request_digest: Digest::raw_json(b"child-request"),
        placement: crate::ChildPlacement::CompatibleLaneInParentSession,
        budget_reservation_id: Some(reservation_id),
    };
    let prepare_records = vec![
        composition_envelope(
            1,
            timestamp,
            session_id,
            lane_id,
            parent_run_id,
            RecordBody::ChildRunPrepared(prepared.clone()),
        ),
        composition_envelope(
            2,
            timestamp,
            session_id,
            lane_id,
            parent_run_id,
            RecordBody::BudgetReservationRequested(crate::BudgetReservationRequested {
                request: request.clone(),
            }),
        ),
    ];
    let prepare_batch =
        CommittedBatch::try_new(fixed_id::<crate::AppendBatchTag>(20), 1, 2, prepare_records)
            .expect("prepare batch");
    let applied = apply(&state, &prepare_batch, 0).expect("prepare apply").0;
    assert_eq!(
        applied.child_preparations.get(&parent_effect_id),
        Some(&prepared)
    );
    assert_eq!(
        applied
            .budget_reservations
            .get(&reservation_id)
            .map(|replay| &replay.request),
        Some(&request)
    );

    let receipt = crate::BudgetReservationReceipt {
        scope_id,
        reservation_id,
        reserved: amount.clone(),
        remaining: crate::BudgetRequest::default(),
        request_digest,
        receipt_digest: Digest::raw_json(b"reservation-receipt"),
    };
    let settlement = composition_envelope(
        3,
        timestamp,
        session_id,
        lane_id,
        parent_run_id,
        RecordBody::BudgetReservationSettled(crate::BudgetReservationSettled {
            receipt: receipt.clone(),
        }),
    );
    let settlement_batch = CommittedBatch::try_new(
        fixed_id::<crate::AppendBatchTag>(21),
        3,
        3,
        vec![settlement],
    )
    .expect("settlement batch");
    let settled = apply(&applied, &settlement_batch, 0)
        .expect("settlement apply")
        .0;
    assert_eq!(
        settled
            .budget_reservations
            .get(&reservation_id)
            .and_then(|replay| replay.settlement.as_ref()),
        Some(&receipt)
    );
    let encoded = serde_json::to_vec(&settled).expect("v5 state JSON");
    let decoded: KernelState = serde_json::from_slice(&encoded).expect("v5 replay state");
    assert_eq!(decoded, settled);
    assert_eq!(decoded.state_hash(), settled.state_hash());

    let mut conflicting = prepared;
    conflicting.request_digest = Digest::raw_json(b"conflicting-child-request");
    let conflict_records = vec![
        composition_envelope(
            4,
            timestamp,
            session_id,
            lane_id,
            parent_run_id,
            RecordBody::ChildRunPrepared(conflicting),
        ),
        composition_envelope(
            5,
            timestamp,
            session_id,
            lane_id,
            parent_run_id,
            RecordBody::BudgetReservationRequested(crate::BudgetReservationRequested { request }),
        ),
    ];
    let conflict_batch = CommittedBatch::try_new(
        fixed_id::<crate::AppendBatchTag>(22),
        4,
        5,
        conflict_records,
    )
    .expect("conflict batch");
    assert_eq!(
        apply(&settled, &conflict_batch, 0),
        Err(KernelError::InvalidRecordOrder)
    );
}

#[test]
fn semantic_tampering_precedes_capacity_failure() {
    let (state, batch) = external_batch_at_capacity(true);
    assert_eq!(
        apply(&state, &batch, 0),
        Err(KernelError::ModelSettlementMismatch)
    );
}

#[test]
fn committed_batch_rejects_over_ceiling_with_reason_code() {
    let record = RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        fixed_id::<crate::RecordTag>(1),
        fixed_id::<crate::SessionTag>(1),
        fixed_id::<crate::LaneTag>(2),
        None,
        1,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        Digest::raw_json(b"payload"),
        None,
        Digest::raw_json(b"checksum"),
        vec![],
        RecordBody::LaneCreated(crate::LaneCreated::try_new("research").expect("lane")),
    )
    .expect("record");
    let records = vec![record; APPEND_BATCH_MAX_RECORDS + 1];
    assert_eq!(
        CommittedBatch::try_new(fixed_id::<crate::AppendBatchTag>(99), 1, 1, records),
        Err(KernelError::InvalidInputPayload {
            field: "records",
            reason_code: "too_many_items",
        })
    );
}

#[test]
fn event_sequence_overflow_keeps_reason_code() {
    let (mut state, batch) = external_batch_at_capacity(false);
    state.model_settlements.clear();
    assert_eq!(
        apply(&state, &batch, u64::MAX),
        Err(KernelError::InvalidInputPayload {
            field: "first_transient_sequence",
            reason_code: "overflow",
        })
    );
}

#[test]
fn whole_batch_capacity_failure_leaves_original_state_unchanged() {
    let (state, batch) = external_batch_at_capacity(false);
    let before = state.clone();
    assert_eq!(
        apply(&state, &batch, 0),
        Err(KernelError::StateCapacityExceeded {
            field: "model_settlements"
        })
    );
    assert_eq!(state, before);
}

fn external_batch_at_capacity(tampered: bool) -> (KernelState, CommittedBatch) {
    let effect_id = fixed_id::<crate::EffectTag>(1);
    let turn_id = fixed_id::<crate::TurnTag>(2);
    let model_request_id = fixed_id::<crate::ModelRequestTag>(3);
    let contract = EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"schema"),
    };
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Model,
        None,
        None,
        None,
        contract.clone(),
        EffectInput::Model {
            request: RawJson::parse("{}").expect("request"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("request");
    let deferred = crate::EffectDeferred {
        effect_id,
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.provider.fixture").expect("component"),
            "external-job",
            RawJson::parse("{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
        output_contract: contract.clone(),
    };
    let timestamp = Timestamp::from_unix_ms(1_000).expect("timestamp");
    let completed = crate::EffectCompleted::try_new(
        effect_id,
        contract,
        RawJson::parse("{}").expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("completion"),
        tampered.then(|| fixed_id::<crate::BudgetReservationTag>(4)),
    )
    .expect("shape-valid tampered completion");
    let message = Message::try_new(
        fixed_id::<crate::MessageTag>(5),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        timestamp,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message");
    let entry = EntryAppended {
        cycle: 0,
        turn_id,
        model_request_id,
        effect_id,
        parent_message_id: None,
        message,
    };
    let records = vec![
        envelope(1, 1, timestamp, RecordBody::EffectCompleted(completed)),
        envelope(2, 2, timestamp, RecordBody::EntryAppended(entry)),
    ];
    let batch = CommittedBatch::try_new(fixed_id::<crate::AppendBatchTag>(6), 1, 2, records)
        .expect("batch");
    let model_settlements = (0..crate::SEMANTIC_MAP_MAX_ENTRIES)
        .map(|ordinal| {
            let id = fixed_id::<crate::EffectTag>(u64::try_from(ordinal + 100).expect("ordinal"));
            (
                id,
                ModelSettlementFingerprint {
                    kind: ModelSettlementKind::Completed,
                    digest: Digest::raw_json(format!("settlement-{ordinal}").as_bytes()),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let state = KernelState {
        phase: Some(RunPhase::AwaitingExternal),
        pending_model_effect: Some(PendingModelEffect {
            cycle: 0,
            turn_id,
            model_request_id,
            requested,
            deferred: Some(deferred),
        }),
        model_settlements,
        ..KernelState::default()
    };
    (state, batch)
}

fn envelope(
    record_ordinal: u64,
    event_ordinal: u64,
    timestamp: Timestamp,
    body: RecordBody,
) -> RecordEnvelope {
    RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        fixed_id::<crate::RecordTag>(record_ordinal),
        fixed_id::<crate::SessionTag>(10),
        fixed_id::<crate::LaneTag>(11),
        Some(fixed_id::<crate::RunTag>(12)),
        record_ordinal,
        timestamp,
        None,
        Digest::raw_json(b"payload"),
        None,
        Digest::raw_json(b"checksum"),
        vec![fixed_id::<crate::EventTag>(event_ordinal)],
        body,
    )
    .expect("record")
}

fn composition_envelope(
    sequence: u64,
    timestamp: Timestamp,
    session_id: crate::SessionId,
    lane_id: crate::LaneId,
    run_id: crate::RunId,
    body: RecordBody,
) -> RecordEnvelope {
    RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        fixed_id::<crate::RecordTag>(sequence + 100),
        session_id,
        lane_id,
        Some(run_id),
        sequence,
        timestamp,
        None,
        Digest::raw_json(b"payload"),
        None,
        Digest::raw_json(b"checksum"),
        vec![],
        body,
    )
    .expect("composition record")
}

fn fixed_id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    crate::Id::from_bytes(bytes)
}
