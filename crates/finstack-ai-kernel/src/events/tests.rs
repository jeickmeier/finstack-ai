use super::*;
use crate::effects::{
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested, RetrySafety,
};
use crate::primitives::Digest;
use crate::primitives::RawJson;
use crate::primitives::Timestamp;
use crate::primitives::{
    EffectId, EventId, LaneId, MessageId, ModelRequestId, RecordId, RunId, SessionId, TurnId,
};
use crate::records::{RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordEnvelope};
use crate::refs::Sensitivity;

#[test]
fn durable_and_transient_constructors_are_class_safe() {
    let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("e");
    let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("s");
    let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("l");
    let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("r");
    let turn = TurnId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("turn");
    let model_request =
        ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("request");
    let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("effect");
    let delta = ModelTextDelta::try_new("hi").expect("delta");
    assert!(
        RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session,
            lane,
            run,
            None,
            None,
            None,
            None,
            None,
            1,
            0,
            Timestamp::from_unix_ms(0).expect("ts"),
            Sensitivity::Internal,
            RunEventBody::ModelTextDelta(delta.clone()),
        )
        .is_err()
    );
    RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        event_id,
        session,
        lane,
        run,
        Some(turn),
        Some(model_request),
        None,
        Some(effect),
        None,
        0,
        Timestamp::from_unix_ms(0).expect("ts"),
        Sensitivity::Internal,
        RunEventBody::ModelTextDelta(delta.clone()),
    )
    .expect_err("model deltas require confidential sensitivity");
    let transient = RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        event_id,
        session,
        lane,
        run,
        Some(turn),
        Some(model_request),
        None,
        Some(effect),
        None,
        0,
        Timestamp::from_unix_ms(0).expect("ts"),
        Sensitivity::Confidential,
        RunEventBody::ModelTextDelta(delta),
    )
    .expect("transient");
    assert_eq!(transient.class(), RunEventClass::Transient);
    assert_eq!(transient.durable_sequence(), None);
    assert_eq!(transient.schema_version(), RUN_EVENT_SCHEMA_VERSION);
    assert_eq!(transient.session_id(), session);
    assert!(transient.model_request_id().is_some());
    assert_eq!(transient.sensitivity(), Sensitivity::Confidential);

    let error = RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        event_id,
        session,
        lane,
        run,
        None,
        None,
        None,
        None,
        None,
        1,
        Timestamp::from_unix_ms(0).expect("ts"),
        Sensitivity::Internal,
        RunEventBody::ModelTextDelta(ModelTextDelta::try_new("missing").expect("delta")),
    )
    .expect_err("model delta requires model request correlation");
    assert_eq!(error.code(), "event_correlation_mismatch");
}

#[test]
fn durable_effect_event_round_trips_human_json_with_raw_input() {
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
    let requested = EffectRequested::try_new(
        effect_id,
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
    .expect("request");
    let turn_id = TurnId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("turn");
    let model_request_id =
        ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("request");
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
        RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
        None,
        None,
        None,
        Some(effect_id),
        None,
        1,
        0,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        Sensitivity::Internal,
        RunEventBody::EffectRequested(requested.clone()),
    )
    .expect_err("model effect events require correlations and confidential sensitivity");
    let event = RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
        RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
        Some(turn_id),
        Some(model_request_id),
        None,
        Some(effect_id),
        None,
        1,
        0,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        Sensitivity::Confidential,
        RunEventBody::EffectRequested(requested),
    )
    .expect("event");
    let json = serde_json::to_string(&event).expect("serialize");
    let round: RunEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(round, event);
}

#[test]
fn event_constructors_reject_unknown_versions() {
    let error = RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION + 1,
        RUN_EVENT_KIND_VERSION,
        EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
        RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
        None,
        None,
        None,
        None,
        None,
        0,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        Sensitivity::Internal,
        RunEventBody::ModelTextDelta(ModelTextDelta::try_new("delta").expect("delta")),
    )
    .expect_err("unknown schema version");
    assert_eq!(error.code(), "unsupported_schema_version");
}

#[test]
fn effect_events_require_matching_effect_correlation() {
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
    let requested = EffectRequested::try_new(
        effect_id,
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
    .expect("request");
    let error = RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane"),
        RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run"),
        None,
        None,
        None,
        None,
        None,
        1,
        0,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        Sensitivity::Internal,
        RunEventBody::EffectRequested(requested),
    )
    .expect_err("missing effect correlation");
    assert_eq!(error.code(), "event_correlation_mismatch");
}

#[test]
fn durable_event_from_record_reuses_persisted_ordinal_id() {
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
    let requested = EffectRequested::try_new(
        effect_id,
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
    .expect("request");
    let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event");
    let record = RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("record"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("lane"),
        Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("run")),
        7,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        None,
        Digest::raw_json(br"payload"),
        None,
        Digest::raw_json(br"checksum"),
        vec![event_id],
        RecordBody::EffectRequested(requested),
    )
    .expect("record");
    let missing = RunEvent::try_from_record(&record, 0, 3);
    assert!(matches!(
        missing,
        Err(EventError::CorrelationMismatch { reason })
            if reason.contains("authoritative model correlations")
    ));
}

#[test]
fn non_model_effect_record_derives_without_model_correlations() {
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("effect");
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Context,
        None,
        None,
        None,
        EffectOutputContract {
            kind: EffectOutputKind::ContextContribution,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"schema"),
        },
        EffectInput::Context {
            request: RawJson::parse("{}").expect("request"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("request");
    let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event");
    let record = RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        RecordId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("record"),
        SessionId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("session"),
        LaneId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("lane"),
        Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("run")),
        7,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        None,
        Digest::raw_json(b"payload"),
        None,
        Digest::raw_json(b"checksum"),
        vec![event_id],
        RecordBody::EffectRequested(requested),
    )
    .expect("record");

    let event = RunEvent::try_from_record(&record, 0, 3).expect("derived context event");
    assert_eq!(event.event_id(), event_id);
    assert_eq!(event.effect_id(), Some(effect_id));
    assert_eq!(event.turn_id(), None);
    assert_eq!(event.model_request_id(), None);
    assert_eq!(event.sensitivity(), Sensitivity::Internal);
}

#[test]
fn finalized_message_requires_model_correlations_and_internal_sensitivity() {
    let event_id = EventId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("event");
    let session = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("session");
    let lane = LaneId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("lane");
    let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("run");
    let message = MessageId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("message");
    let turn = TurnId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("turn");
    let request = ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("request");
    let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("effect");
    let body = || RunEventBody::MessageFinalized {
        message_id: message,
    };

    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        event_id,
        session,
        lane,
        run,
        None,
        None,
        None,
        None,
        None,
        1,
        0,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        Sensitivity::Internal,
        body(),
    )
    .expect_err("message finalized requires model correlations");
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        event_id,
        session,
        lane,
        run,
        Some(turn),
        Some(request),
        None,
        Some(effect),
        None,
        1,
        0,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        Sensitivity::Confidential,
        body(),
    )
    .expect_err("message finalized must be internal");
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        event_id,
        session,
        lane,
        run,
        Some(turn),
        Some(request),
        None,
        Some(effect),
        None,
        1,
        0,
        Timestamp::from_unix_ms(0).expect("timestamp"),
        Sensitivity::Internal,
        body(),
    )
    .expect("valid message finalized");
}
