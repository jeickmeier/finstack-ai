//! Public-API compatibility fixtures for PR-009 through PR-011 reducer contracts.

use finstack_ai_kernel::{
    APPEND_BATCH_MAX_RECORDS, CommittedBatch, KernelInput, KernelState, RECORD_KIND_VERSION,
    RecordEnvelope, RunEvent, RunPhase,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::paths::try_compatibility_fixture;
use crate::public_api_fixture::{Expect, PublicApiFixture, PublicApiFixtureError};
use crate::reducer_fixture::execute_reducer_trace;
use crate::trace_fixture::load_golden_trace;

/// Execute a PR-009 public-rust-api fixture subject.
pub(crate) fn run_pr009_subject(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    match fixture.subject.as_str() {
        "run-phase" => run_phase(fixture),
        "kernel-input" => run_kernel_input(fixture),
        "committed-batch" => run_committed_batch(fixture),
        "kernel-state" => run_kernel_state(fixture),
        "pr009-record" | "pr010-record" | "pr011-record" => run_record(fixture),
        other => Err(fail(format!("unsupported PR-009 subject {other}"))),
    }
}

fn run_phase(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input(fixture)?;
    match fixture.operation.as_str() {
        "exact_vocabulary" => {
            let expected = [
                RunPhase::Accepted,
                RunPhase::BeforeRun,
                RunPhase::PreparingContext,
                RunPhase::BeforeModel,
                RunPhase::AwaitingModel,
                RunPhase::AfterModel,
                RunPhase::BeforeToolBatch,
                RunPhase::AwaitingTools,
                RunPhase::AfterToolBatch,
                RunPhase::BeforeFinalize,
                RunPhase::AwaitingInteraction,
                RunPhase::AwaitingExternal,
                RunPhase::Sleeping,
                RunPhase::Cancelling,
                RunPhase::Suspended,
                RunPhase::Completed,
                RunPhase::Failed,
                RunPhase::Cancelled,
            ]
            .map(run_phase_wire_name);
            let actual = input
                .as_array()
                .ok_or_else(|| fail("run-phase exact_vocabulary input must be an array"))?;
            let actual = actual
                .iter()
                .map(|value| value.as_str())
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| fail("run-phase vocabulary values must be strings"))?;
            if actual.as_slice() != expected {
                return Err(fail("run-phase exact ordered vocabulary mismatch"));
            }
            for value in actual {
                let phase: RunPhase = from_json(&Value::String(value.to_owned()))?;
                let encoded =
                    serde_json::to_value(phase).map_err(|error| fail(error.to_string()))?;
                if encoded != value {
                    return Err(fail(format!(
                        "run-phase round trip mismatch: {value} != {encoded}"
                    )));
                }
            }
            if !fixture.expect.ok {
                return Err(fail("expected run-phase vocabulary failure"));
            }
            Ok(())
        }
        "parse" => match from_json::<RunPhase>(&input) {
            Ok(_) if fixture.expect.ok => Ok(()),
            Ok(_) => Err(fail("expected run-phase parse failure")),
            Err(error) => {
                assert_error_code(&fixture.expect, classify_serde_error(&error.to_string()))
            }
        },
        other => Err(fail(format!("unsupported run-phase operation {other}"))),
    }
}

const fn run_phase_wire_name(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Accepted => "accepted",
        RunPhase::BeforeRun => "before_run",
        RunPhase::PreparingContext => "preparing_context",
        RunPhase::BeforeModel => "before_model",
        RunPhase::AwaitingModel => "awaiting_model",
        RunPhase::AfterModel => "after_model",
        RunPhase::BeforeToolBatch => "before_tool_batch",
        RunPhase::AwaitingTools => "awaiting_tools",
        RunPhase::AfterToolBatch => "after_tool_batch",
        RunPhase::BeforeFinalize => "before_finalize",
        RunPhase::AwaitingInteraction => "awaiting_interaction",
        RunPhase::AwaitingExternal => "awaiting_external",
        RunPhase::Sleeping => "sleeping",
        RunPhase::Cancelling => "cancelling",
        RunPhase::Suspended => "suspended",
        RunPhase::Completed => "completed",
        RunPhase::Failed => "failed",
        RunPhase::Cancelled => "cancelled",
    }
}

fn run_kernel_input(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input(fixture)?;
    match fixture.operation.as_str() {
        "parse" => match from_json::<KernelInput>(&input) {
            Ok(value) => {
                if !fixture.expect.ok {
                    return Err(fail("expected KernelInput parse failure"));
                }
                let encoded =
                    serde_json::to_value(value).map_err(|error| fail(error.to_string()))?;
                if encoded != input {
                    return Err(fail("KernelInput strict round-trip mismatch"));
                }
                Ok(())
            }
            Err(error) => {
                assert_error_code(&fixture.expect, classify_serde_error(&error.to_string()))
            }
        },
        other => Err(fail(format!("unsupported kernel-input operation {other}"))),
    }
}

fn run_committed_batch(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input(fixture)?;
    if fixture.operation != "construct_count" {
        return Err(fail(format!(
            "unsupported committed-batch operation {}",
            fixture.operation
        )));
    }
    let count = usize::try_from(
        input
            .get("count")
            .and_then(Value::as_u64)
            .ok_or_else(|| fail("committed-batch count required"))?,
    )
    .map_err(|error| fail(error.to_string()))?;
    let template: RecordEnvelope = from_json_field(&input, "template")?;
    let batch_id = finstack_ai_kernel::AppendBatchId::parse(
        input
            .get("batch_id")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("committed-batch batch_id required"))?,
    )
    .map_err(|error| fail(error.to_string()))?;
    let first_sequence = input
        .get("first_sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| fail("committed-batch first_sequence required"))?;
    let last_sequence = input
        .get("last_sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| fail("committed-batch last_sequence required"))?;
    let materialized_count = count.min(APPEND_BATCH_MAX_RECORDS + 1);
    match CommittedBatch::try_new(
        batch_id,
        first_sequence,
        last_sequence,
        vec![template; materialized_count],
    ) {
        Ok(batch) => {
            if !fixture.expect.ok {
                return Err(fail("expected CommittedBatch construction failure"));
            }
            if batch.records.len() != count || batch.records.len() > APPEND_BATCH_MAX_RECORDS {
                return Err(fail("CommittedBatch count invariant mismatch"));
            }
            Ok(())
        }
        Err(error) => assert_error_code(&fixture.expect, error.code()),
    }
}

fn run_kernel_state(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    if fixture.operation == "roundtrip" {
        let input = require_input(fixture)?;
        let decoded = from_json::<KernelState>(&input);
        return match decoded {
            Ok(state) => {
                if !fixture.expect.ok {
                    return Err(fail("expected KernelState roundtrip failure"));
                }
                let encoded =
                    serde_json::to_value(&state).map_err(|error| fail(error.to_string()))?;
                let reparsed = from_json::<KernelState>(&encoded)?;
                if reparsed != state {
                    return Err(fail("KernelState roundtrip changed semantic state"));
                }
                let expected_version = fixture
                    .expect
                    .extras
                    .get("state_version")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| fail("kernel-state expect.state_version required"))?;
                if u64::from(state.state_version) != expected_version {
                    return Err(fail(format!(
                        "kernel-state version mismatch: expected {expected_version}, got {}",
                        state.state_version
                    )));
                }
                state
                    .state_hash()
                    .map_err(|error| fail(format!("{}: {error}", error.code())))?;
                Ok(())
            }
            Err(error) => {
                assert_error_code(&fixture.expect, classify_serde_error(&error.to_string()))
            }
        };
    }
    let state = match fixture.operation.as_str() {
        "default_hash" => KernelState::default(),
        "reducer_completed_hash" => {
            let input = require_input(fixture)?;
            let relative = input
                .get("trace")
                .and_then(Value::as_str)
                .ok_or_else(|| fail("kernel-state reducer trace required"))?;
            let path =
                try_compatibility_fixture(relative).map_err(|error| fail(error.to_string()))?;
            let trace = load_golden_trace(path).map_err(|error| fail(error.to_string()))?;
            execute_reducer_trace(&trace)
                .map_err(|error| fail(error.to_string()))?
                .kernel_state
        }
        other => return Err(fail(format!("unsupported kernel-state operation {other}"))),
    };
    if !fixture.expect.ok {
        return Err(fail("expected KernelState hash failure"));
    }
    let actual = state
        .state_hash()
        .map_err(|error| fail(format!("{}: {error}", error.code())))?
        .to_hex();
    let expected = fixture
        .expect
        .extras
        .get("state_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("kernel-state expect.state_hash required"))?;
    if actual != expected {
        return Err(fail(format!(
            "kernel-state hash mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn run_record(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    if fixture.operation != "parse_and_derive" {
        return Err(fail(format!(
            "unsupported pr009-record operation {}",
            fixture.operation
        )));
    }
    let input = require_input(fixture)?;
    let record: RecordEnvelope = from_json(&input)?;
    if !fixture.expect.ok {
        return Err(fail("expected PR-009 record parse failure"));
    }
    let expected_kind = fixture
        .expect
        .extras
        .get("body_kind")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("pr009-record expect.body_kind required"))?;
    if record.body().kind_name() != expected_kind {
        return Err(fail(format!(
            "record body kind mismatch: {} != {expected_kind}",
            record.body().kind_name()
        )));
    }
    let expected_count = usize::try_from(
        fixture
            .expect
            .extras
            .get("derived_event_count")
            .and_then(Value::as_u64)
            .ok_or_else(|| fail("pr009-record expect.derived_event_count required"))?,
    )
    .map_err(|error| fail(error.to_string()))?;
    let actual_count = record
        .body()
        .derived_event_count(RECORD_KIND_VERSION)
        .map_err(|error| fail(error.to_string()))?;
    if actual_count != expected_count || record.derived_event_ids().len() != expected_count {
        return Err(fail("PR-009 record derived-event count mismatch"));
    }
    if expected_count == 0 {
        return Ok(());
    }
    if let Some(expected_kinds) = fixture
        .expect
        .extras
        .get("event_kinds")
        .and_then(Value::as_array)
    {
        if expected_kinds.len() != expected_count {
            return Err(fail("record event_kinds count mismatch"));
        }
        for (ordinal, expected_kind) in expected_kinds.iter().enumerate() {
            let event = RunEvent::try_from_record(&record, ordinal, record.sequence())
                .map_err(|error| fail(format!("{}: {error}", error.code())))?;
            let actual_kind =
                serde_json::to_value(event.kind()).map_err(|error| fail(error.to_string()))?;
            if &actual_kind != expected_kind {
                return Err(fail(format!(
                    "derived event kind mismatch at ordinal {ordinal}: {actual_kind} != {expected_kind}"
                )));
            }
        }
    } else {
        let event = RunEvent::try_from_record(&record, 0, record.sequence())
            .map_err(|error| fail(format!("{}: {error}", error.code())))?;
        let actual_kind =
            serde_json::to_value(event.kind()).map_err(|error| fail(error.to_string()))?;
        let expected_kind = fixture
            .expect
            .extras
            .get("event_kind")
            .ok_or_else(|| fail("record expect.event_kind or event_kinds required"))?;
        if &actual_kind != expected_kind {
            return Err(fail(format!(
                "derived event kind mismatch: {actual_kind} != {expected_kind}"
            )));
        }
    }
    Ok(())
}

fn require_input(fixture: &PublicApiFixture) -> Result<Value, PublicApiFixtureError> {
    fixture
        .input
        .clone()
        .ok_or_else(|| fail("fixture input required"))
}

fn from_json_field<T: DeserializeOwned>(
    value: &Value,
    field: &str,
) -> Result<T, PublicApiFixtureError> {
    let child = value
        .get(field)
        .ok_or_else(|| fail(format!("{field} required")))?;
    from_json(child)
}

fn from_json<T: DeserializeOwned>(value: &Value) -> Result<T, PublicApiFixtureError> {
    let text = serde_json::to_string(value).map_err(|error| fail(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| fail(error.to_string()))
}

fn classify_serde_error(message: &str) -> &'static str {
    if message.contains("unknown field") {
        "unknown_field"
    } else if message.contains("unknown variant") {
        "unknown_variant"
    } else {
        "invalid_input_payload"
    }
}

fn assert_error_code(expect: &Expect, actual: &str) -> Result<(), PublicApiFixtureError> {
    if expect.ok {
        return Err(fail(format!("expected success, got error {actual}")));
    }
    let expected = expect
        .error_code
        .as_deref()
        .ok_or_else(|| fail("failed fixture requires expect.error_code"))?;
    if expected != actual {
        return Err(fail(format!(
            "error_code mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn fail(message: impl Into<String>) -> PublicApiFixtureError {
    PublicApiFixtureError::Failed(message.into())
}
