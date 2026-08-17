use super::*;
use crate::effects::{
    EffectInput, EffectKind, EffectOutputContract, InteractionKind, InteractionRequest, RetrySafety,
};
use crate::effects::{EffectOutputKind, EffectRequested};
use crate::policy::RunLimits;
use crate::primitives::AppendBatchId;
use crate::primitives::Digest;
use crate::primitives::Timestamp;
use crate::primitives::{ComponentId, EffectId, EventId, InteractionId};
use crate::primitives::{LaneId, RecordId, RunId, SessionId};
use crate::primitives::{Metadata, RawJson};
use crate::refs::{ComponentRef, PrincipalRef, Version};
use crate::run::RunAccepted;
use crate::run::RunRelationKind;
use crate::run::{
    BudgetPropagation, CancellationPropagation, DeadlinePropagation, PrincipalPropagation,
    RunPropagationPolicy, RunRelation, RunSecurityContext,
};
use std::sync::Arc;

fn sample_run_accepted() -> RunAccepted {
    let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run");
    RunAccepted::try_new(
        run,
        RunRelation::root(run).expect("rel"),
        RunSecurityContext::try_new(
            "tenant",
            PrincipalRef::try_new("iss", "sub", Some("tenant")).expect("p"),
            "oidc",
            "high",
            "policy",
            "decision",
            None,
        )
        .expect("sec"),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br"{}"),
        None,
    )
    .expect("accepted")
}

fn sample_child_run_accepted(parent: &RunAccepted) -> RunAccepted {
    let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("run");
    RunAccepted::try_new(
        run,
        RunRelation::try_new(
            parent.run_id(),
            Some(parent.run_id()),
            Some(EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect")),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("relation"),
        RunSecurityContext::try_new(
            "tenant",
            PrincipalRef::try_new("iss", "sub", Some("tenant")).expect("principal"),
            "oidc",
            "high",
            "policy",
            "decision",
            None,
        )
        .expect("security"),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br"{}"),
        Some(parent),
    )
    .expect("child accepted")
}

fn sample_effect_requested() -> EffectRequested {
    EffectRequested::try_new(
        EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect"),
        EffectKind::Model,
        None,
        None,
        None,
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(br#"{"schema":1}"#),
        },
        EffectInput::Model {
            request: RawJson::parse(r#"{"messages":[]}"#).expect("request"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("effect")
}

#[test]
fn record_creation_rejects_programmatically_invalid_failure_descriptor() {
    let mut error = crate::ErrorDescriptor::new(
        "stage_failed",
        "failed",
        crate::ErrorCategory::Middleware,
        false,
    )
    .expect("descriptor");
    error.message = Arc::from("x".repeat(crate::TEXT_MAX_BYTES + 1));
    let result = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("record"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
        Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run")),
        Timestamp::from_unix_ms(0).expect("timestamp"),
        vec![],
        RecordBody::StageOutcomeRecorded(crate::StageOutcomeRecorded {
            cursor: crate::StageCursor {
                cycle: 0,
                stage: crate::Stage::BeforeRun,
            },
            disposition: crate::StageDisposition::Failed { error },
            settlement_digest: Digest::raw_json(b"settlement"),
        }),
    )
    .expect_err("invalid descriptor");
    assert_eq!(result.code(), "invalid_error_descriptor");
}

#[test]
fn serialized_child_run_record_replays_as_validated_lineage() {
    let parent = sample_run_accepted();
    let child = sample_child_run_accepted(&parent);
    let record = RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("record"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("lane"),
        Some(child.run_id()),
        1,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        None,
        Digest::raw_json(b"payload"),
        None,
        Digest::raw_json(b"checksum"),
        vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("event")],
        RecordBody::RunAccepted(child),
    )
    .expect("record");
    let encoded = serde_json::to_vec(&record).expect("serialize record");
    let decoded: RecordEnvelope = serde_json::from_slice(&encoded).expect("decode record");
    let RecordBody::RunAccepted(decoded_child) = decoded.body() else {
        panic!("run accepted body");
    };
    assert!(decoded_child.lineage_is_validated());

    let batch = crate::CommittedBatch::try_new(
        AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789a5").expect("batch"),
        1,
        1,
        vec![decoded],
    )
    .expect("batch");
    let mut kernel = crate::Kernel::default();
    kernel.apply(&batch, 0).expect("replay child acceptance");
}

#[test]
fn draft_requires_one_derived_event() {
    let record_id = RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("r");
    let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("s");
    let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("l");
    let event = EventId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("e");
    let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run");
    let body = RecordBody::RunAccepted(sample_run_accepted());
    assert!(
        RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            record_id,
            session,
            lane,
            Some(run),
            Timestamp::from_unix_ms(0).expect("ts"),
            vec![],
            body.clone(),
        )
        .is_err()
    );
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        record_id,
        session,
        lane,
        Some(run),
        Timestamp::from_unix_ms(0).expect("ts"),
        vec![event],
        body,
    )
    .expect("draft");
    assert_eq!(draft.derived_event_ids().len(), 1);
}

#[test]
fn effect_record_body_round_trips_human_json_with_raw_input() {
    let body = RecordBody::EffectRequested(sample_effect_requested());
    let json = serde_json::to_string(&body).expect("serialize");
    let round: RecordBody = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(round, body);
}

#[test]
fn append_request_rejects_records_from_another_session() {
    let request_session =
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session");
    let record_session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session");
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("record"),
        record_session,
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("lane"),
        Some(sample_run_accepted().run_id()),
        Timestamp::from_unix_ms(0).expect("timestamp"),
        vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("event")],
        RecordBody::RunAccepted(sample_run_accepted()),
    )
    .expect("draft");
    let error = AppendRequest::try_new(
        AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("batch"),
        request_session,
        0,
        vec![draft],
    )
    .expect_err("mixed sessions");
    assert_eq!(error.code(), "record_session_mismatch");
}

#[test]
fn run_accepted_record_requires_matching_run_id() {
    let accepted = sample_run_accepted();
    let error = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("record"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane"),
        None,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("event")],
        RecordBody::RunAccepted(accepted),
    )
    .expect_err("missing run id");
    assert_eq!(error.code(), "record_run_mismatch");
}

#[test]
fn record_envelope_deserialization_validates_versions_and_event_count() {
    let accepted = sample_run_accepted();
    let envelope = RecordEnvelope {
        format_version: RECORD_FORMAT_VERSION + 1,
        kind_version: RECORD_KIND_VERSION,
        record_id: RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("record"),
        session_id: SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
        lane_id: LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
        run_id: Some(accepted.run_id()),
        sequence: 1,
        timestamp: Timestamp::from_unix_ms(0).expect("timestamp"),
        committed_at: None,
        payload_digest: Digest::raw_json(br"payload"),
        previous_checksum: None,
        checksum: Digest::raw_json(br"checksum"),
        derived_event_ids: Arc::from([]),
        body: RecordBody::RunAccepted(accepted),
    };
    let json = serde_json::to_string(&envelope).expect("serialize");
    assert!(
        serde_json::from_str::<RecordEnvelope>(&json).is_err(),
        "unsupported version and missing derived event id must fail"
    );
}

#[test]
fn append_request_preserves_injected_record_order() {
    let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session");
    let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane");
    let run = sample_run_accepted();
    let records = [
        (
            "01234567-89ab-7cde-89ab-0123456789ad",
            "01234567-89ab-7cde-89ab-0123456789ae",
        ),
        (
            "01234567-89ab-7cde-89ab-0123456789af",
            "01234567-89ab-7cde-89ab-0123456789b0",
        ),
    ]
    .into_iter()
    .map(|(record_id, event_id)| {
        RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse(record_id).expect("record"),
            session,
            lane,
            Some(run.run_id()),
            Timestamp::from_unix_ms(0).expect("timestamp"),
            vec![EventId::parse(event_id).expect("event")],
            RecordBody::RunAccepted(run.clone()),
        )
        .expect("draft")
    })
    .collect::<Vec<_>>();
    let expected = records
        .iter()
        .map(RecordDraft::record_id)
        .collect::<Vec<_>>();
    let request = AppendRequest::try_new(
        AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("batch"),
        session,
        0,
        records,
    )
    .expect("append");
    let actual = request
        .records()
        .iter()
        .map(RecordDraft::record_id)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}

#[test]
fn append_request_rejects_unpaired_interaction_request() {
    let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session");
    let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane");
    let accepted = sample_run_accepted();
    let interaction_id =
        InteractionId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("interaction");
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("effect");
    let interaction = InteractionRequest::try_new(
        1,
        interaction_id,
        effect_id,
        InteractionKind::Approval,
        vec![],
        RawJson::parse(r#"{"type":"boolean"}"#).expect("schema"),
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").expect("component"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
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
    .expect("interaction");
    let interaction_digest = interaction.request_digest().expect("digest");
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("record"),
        session,
        lane,
        Some(accepted.run_id()),
        Timestamp::from_unix_ms(0).expect("timestamp"),
        vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("event")],
        RecordBody::InteractionRequested(interaction),
    )
    .expect("draft");
    let error = AppendRequest::try_new(
        AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("batch"),
        session,
        0,
        vec![draft.clone()],
    )
    .expect_err("interaction request requires effect request sibling");
    assert_eq!(error.code(), "invalid_interaction_pair");

    let effect = EffectRequested::try_new(
        effect_id,
        EffectKind::Interaction,
        None,
        None,
        None,
        EffectOutputContract {
            kind: EffectOutputKind::InteractionResolution,
            schema_version: 1,
            schema_digest: Digest::raw_json(br"interaction-resolution"),
        },
        EffectInput::Interaction {
            interaction_id,
            request_digest: interaction_digest,
        },
        RetrySafety::AtMostOnce,
        None,
    )
    .expect("effect");
    let effect_draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("record"),
        session,
        lane,
        Some(accepted.run_id()),
        Timestamp::from_unix_ms(0).expect("timestamp"),
        vec![EventId::parse("01234567-89ab-7cde-89ab-0123456789b3").expect("event")],
        RecordBody::EffectRequested(effect),
    )
    .expect("effect draft");
    AppendRequest::try_new(
        AppendBatchId::parse("01234567-89ab-7cde-89ab-0123456789b4").expect("batch"),
        session,
        0,
        vec![effect_draft, draft],
    )
    .expect("paired interaction batch");
}
