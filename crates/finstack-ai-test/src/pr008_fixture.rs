//! Public-API fixtures for PR-008 run/effect/record/event subjects.

use finstack_ai_kernel::{
    APPEND_BATCH_MAX_RECORDS, AppendRequest, EffectInput, EffectKind, EffectOutputContract,
    EffectRequested, EventId, LaneId, ModelTextDelta, RUN_EVENT_KIND_VERSION,
    RUN_EVENT_SCHEMA_VERSION, RecordDraft, RetrySafety, RunAccepted, RunEvent, RunEventBody, RunId,
    Sensitivity, SessionId, Timestamp,
};
use serde_json::Value;

use crate::public_api_fixture::{Expect, PublicApiFixture, PublicApiFixtureError};

/// Execute a PR-008 public-rust-api fixture subject.
pub(crate) fn run_pr008_subject(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    match fixture.subject.as_str() {
        "run_accepted" => run_run_accepted(fixture),
        "effect_requested" => run_effect_requested(fixture),
        "record_draft" => run_record_draft(fixture),
        "append_request" => run_append_request(fixture),
        "run_event" => run_run_event(fixture),
        other => Err(fail(format!("unsupported pr008 subject {other}"))),
    }
}

fn run_run_accepted(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_value(fixture)?;
    match fixture.operation.as_str() {
        "parse" => match from_json_value::<RunAccepted>(&input) {
            Ok(value) => {
                if !fixture.expect.ok {
                    return Err(fail("expected run_accepted failure"));
                }
                if let Some(expected) = fixture.expect.extras.get("run_id").and_then(Value::as_str)
                    && value.run_id().to_canonical_string() != expected
                {
                    return Err(fail("run_id mismatch"));
                }
                Ok(())
            }
            Err(error) => {
                assert_error_code(&fixture.expect, classify_run_error(&error.to_string()))
            }
        },
        "construct_child" => {
            let parent_value = input.get("parent").ok_or_else(|| fail("parent required"))?;
            let child_value = input.get("child").ok_or_else(|| fail("child required"))?;
            let parent: RunAccepted = from_json_value(parent_value)?;
            let child_wire: RunAccepted = from_json_value(child_value)?;
            match RunAccepted::try_new(
                child_wire.run_id(),
                child_wire.relation().clone(),
                child_wire.security().clone(),
                child_wire.effective_deadline(),
                child_wire.limits().clone(),
                child_wire.propagation(),
                child_wire.resolved_agent_lock_digest(),
                Some(&parent),
            ) {
                Ok(value) => {
                    if !fixture.expect.ok {
                        return Err(fail("expected run_accepted failure"));
                    }
                    if let Some(expected) =
                        fixture.expect.extras.get("run_id").and_then(Value::as_str)
                        && value.run_id().to_canonical_string() != expected
                    {
                        return Err(fail("run_id mismatch"));
                    }
                    Ok(())
                }
                Err(error) => assert_error_code(&fixture.expect, error.code()),
            }
        }
        other => Err(fail(format!("unsupported run_accepted operation {other}"))),
    }
}

fn run_effect_requested(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_value(fixture)?;
    match fixture.operation.as_str() {
        "construct" => {
            let effect_id = parse_effect_id(&input)?;
            let kind: EffectKind = from_json_field(&input, "kind")?;
            let output_contract: EffectOutputContract = from_json_field(&input, "output_contract")?;
            let effect_input: EffectInput = from_json_field(&input, "input")?;
            match EffectRequested::try_new(
                effect_id,
                kind,
                None,
                None,
                None,
                output_contract,
                effect_input,
                RetrySafety::SafeToRetry,
                None,
            ) {
                Ok(value) => {
                    if !fixture.expect.ok {
                        return Err(fail("expected effect_requested failure"));
                    }
                    if let Some(expected) = fixture
                        .expect
                        .extras
                        .get("input_digest")
                        .and_then(Value::as_str)
                        && value.input_digest().to_hex() != expected
                    {
                        return Err(fail(format!(
                            "input_digest mismatch: {}",
                            value.input_digest().to_hex()
                        )));
                    }
                    Ok(())
                }
                Err(error) => assert_error_code(&fixture.expect, error.code()),
            }
        }
        other => Err(fail(format!(
            "unsupported effect_requested operation {other}"
        ))),
    }
}

fn run_record_draft(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_value(fixture)?;
    match fixture.operation.as_str() {
        "parse" => match from_json_value::<RecordDraft>(&input) {
            Ok(_) => {
                if !fixture.expect.ok {
                    return Err(fail("expected record_draft failure"));
                }
                Ok(())
            }
            Err(error) => {
                assert_error_code(&fixture.expect, classify_record_error(&error.to_string()))
            }
        },
        other => Err(fail(format!("unsupported record_draft operation {other}"))),
    }
}

fn run_append_request(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_value(fixture)?;
    match fixture.operation.as_str() {
        "construct_count" => {
            let count = usize::try_from(
                input
                    .get("count")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| fail("count required"))?,
            )
            .map_err(|error| fail(error.to_string()))?;
            let template: RecordDraft = from_json_field(&input, "template")?;
            let records = vec![template; count];
            let batch_id = input
                .get("batch_id")
                .and_then(Value::as_str)
                .ok_or_else(|| fail("batch_id required"))?;
            let session_id = input
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or_else(|| fail("session_id required"))?;
            match AppendRequest::try_new(
                finstack_ai_kernel::AppendBatchId::parse(batch_id)
                    .map_err(|error| fail(error.to_string()))?,
                SessionId::parse(session_id).map_err(|error| fail(error.to_string()))?,
                0,
                records,
            ) {
                Ok(value) => {
                    if !fixture.expect.ok {
                        return Err(fail("expected append_request failure"));
                    }
                    if value.records().len() > APPEND_BATCH_MAX_RECORDS {
                        return Err(fail("constructed over-limit batch"));
                    }
                    Ok(())
                }
                Err(error) => assert_error_code(&fixture.expect, error.code()),
            }
        }
        other => Err(fail(format!(
            "unsupported append_request operation {other}"
        ))),
    }
}

fn run_run_event(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_value(fixture)?;
    let event_id = parse_uuid_field(&input, "event_id", EventId::parse)?;
    let session_id = parse_uuid_field(&input, "session_id", SessionId::parse)?;
    let lane_id = parse_uuid_field(&input, "lane_id", LaneId::parse)?;
    let run_id = parse_uuid_field(&input, "run_id", RunId::parse)?;
    let text = input.get("text").and_then(Value::as_str).unwrap_or("hello");
    let delta = ModelTextDelta::try_new(text).map_err(|error| fail(error.to_string()))?;
    let ts = Timestamp::from_unix_ms(0).map_err(|error| fail(error.to_string()))?;
    match fixture.operation.as_str() {
        "construct_transient" => match RunEvent::try_transient(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session_id,
            lane_id,
            run_id,
            None,
            None,
            None,
            None,
            None,
            0,
            ts,
            Sensitivity::Internal,
            RunEventBody::ModelTextDelta(delta),
        ) {
            Ok(value) => {
                if !fixture.expect.ok {
                    return Err(fail("expected run_event failure"));
                }
                if value.durable_sequence().is_some() {
                    return Err(fail("transient event has durable_sequence"));
                }
                Ok(())
            }
            Err(error) => assert_error_code(&fixture.expect, error.code()),
        },
        "construct_durable_with_transient_body" => match RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            event_id,
            session_id,
            lane_id,
            run_id,
            None,
            None,
            None,
            None,
            None,
            1,
            0,
            ts,
            Sensitivity::Internal,
            RunEventBody::ModelTextDelta(delta),
        ) {
            Ok(_) => {
                if !fixture.expect.ok {
                    return Err(fail("expected class mismatch failure"));
                }
                Ok(())
            }
            Err(error) => assert_error_code(&fixture.expect, error.code()),
        },
        other => Err(fail(format!("unsupported run_event operation {other}"))),
    }
}

fn parse_effect_id(input: &Value) -> Result<finstack_ai_kernel::EffectId, PublicApiFixtureError> {
    let text = input
        .get("effect_id")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("missing effect_id"))?;
    finstack_ai_kernel::EffectId::parse(text).map_err(|error| fail(error.to_string()))
}

fn parse_uuid_field<T, E: std::fmt::Display>(
    input: &Value,
    field: &str,
    parse: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, PublicApiFixtureError> {
    let text = input
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| fail(format!("missing {field}")))?;
    parse(text).map_err(|error| fail(error.to_string()))
}

fn require_input_value(fixture: &PublicApiFixture) -> Result<Value, PublicApiFixtureError> {
    fixture
        .input
        .clone()
        .ok_or_else(|| fail("fixture input required"))
}

fn from_json_field<T: serde::de::DeserializeOwned>(
    input: &Value,
    field: &str,
) -> Result<T, PublicApiFixtureError> {
    let value = input
        .get(field)
        .ok_or_else(|| fail(format!("missing {field}")))?;
    from_json_value(value)
}

fn from_json_value<T: serde::de::DeserializeOwned>(
    value: &Value,
) -> Result<T, PublicApiFixtureError> {
    // Round-trip through text so `RawJson` human-readable deserialization works.
    let text = serde_json::to_string(value).map_err(|error| fail(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| fail(error.to_string()))
}

fn assert_error_code(expect: &Expect, actual: &str) -> Result<(), PublicApiFixtureError> {
    if expect.ok {
        return Err(fail(format!("expected success, got error {actual}")));
    }
    let Some(expected) = expect.error_code.as_deref() else {
        return Err(fail("expect.error_code required for failures"));
    };
    if expected != actual {
        return Err(fail(format!("error_code mismatch: {actual} != {expected}")));
    }
    Ok(())
}

fn classify_run_error(message: &str) -> &'static str {
    if message.contains("invalid run relation") || message.contains("root relation") {
        "invalid_run_relation"
    } else if message.contains("attenuat") {
        "run_not_attenuated"
    } else {
        "invalid_label"
    }
}

fn classify_record_error(message: &str) -> &'static str {
    if message.contains("derived_event") {
        "derived_event_count"
    } else if message.contains("format_version") {
        "unsupported_format_version"
    } else if message.contains("kind_version") {
        "unsupported_kind_version"
    } else if message.contains("batch") {
        "batch_too_large"
    } else {
        "invalid_label"
    }
}

fn fail(message: impl Into<String>) -> PublicApiFixtureError {
    PublicApiFixtureError::Failed(message.into())
}
