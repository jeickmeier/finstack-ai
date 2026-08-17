//! Public-API compatibility fixtures for PR-009 through PR-014 reducer contracts.

use finstack_ai_kernel::{
    APPEND_BATCH_MAX_RECORDS, CommittedBatch, ExternalCommandRejected,
    ExternalEffectCompletionCommand, InteractionResolutionCommand, Kernel, KernelInput,
    KernelState, OperationLocator, RECORD_KIND_VERSION, RecordEnvelope, RunEvent, RunPhase,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::conformance::reducer::execute_reducer_trace;
use crate::fixtures::public_api::{Expect, PublicApiFixture, PublicApiFixtureError};
use crate::fixtures::trace::load_golden_trace;
use crate::paths::try_compatibility_fixture;

/// Execute a PR-009 public-rust-api fixture subject.
pub(crate) fn run_pr009_subject(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    match fixture.subject.as_str() {
        "run-phase" => run_phase(fixture),
        "kernel-input" => run_kernel_input(fixture),
        "committed-batch" => run_committed_batch(fixture),
        "kernel-state" => run_kernel_state(fixture),
        "operation-locator" => run_strict_parse::<OperationLocator>(fixture),
        "external-effect-completion-command" => {
            run_strict_parse::<ExternalEffectCompletionCommand>(fixture)
        }
        "interaction-resolution-command" => {
            run_strict_parse::<InteractionResolutionCommand>(fixture)
        }
        "external-command-rejected" => run_strict_parse::<ExternalCommandRejected>(fixture),
        "corrupt-replay" => run_corrupt_replay(fixture),
        "pr009-record" | "pr010-record" | "pr011-record" | "pr012-record" | "pr014-record"
        | "pr046-record" => run_record(fixture),
        other => Err(fail(format!("unsupported PR-009 subject {other}"))),
    }
}

fn run_strict_parse<T>(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError>
where
    T: DeserializeOwned + serde::Serialize,
{
    if fixture.operation != "parse" {
        return Err(fail(format!(
            "unsupported {} operation {}",
            fixture.subject, fixture.operation
        )));
    }
    let input = require_input(fixture)?;
    match from_json::<T>(&input) {
        Ok(value) => {
            if !fixture.expect.ok {
                return Err(fail(format!("expected {} parse failure", fixture.subject)));
            }
            let encoded = serde_json::to_value(value).map_err(|error| fail(error.to_string()))?;
            if encoded != input {
                return Err(fail(format!(
                    "{} strict round-trip mismatch",
                    fixture.subject
                )));
            }
            Ok(())
        }
        Err(error) => assert_error_code(&fixture.expect, classify_serde_error(&error.to_string())),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the fixture subject exercises the full decode, replay, atomicity, and hash contract"
)]
fn run_corrupt_replay(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    if fixture.operation != "apply_mutation" {
        return Err(fail(format!(
            "unsupported corrupt-replay operation {}",
            fixture.operation
        )));
    }
    let input = require_input(fixture)?;
    let relative = input
        .get("trace")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("corrupt-replay trace required"))?;
    let mutation = input
        .get("mutation")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("corrupt-replay mutation required"))?;
    let path = try_compatibility_fixture(relative).map_err(|error| fail(error.to_string()))?;
    let mut trace = load_golden_trace(path).map_err(|error| fail(error.to_string()))?;
    if mutation == "trace_format_version" {
        trace.format_version = 2;
        let Err(error) = execute_reducer_trace(&trace) else {
            return Err(fail("unsupported trace version unexpectedly executed"));
        };
        if !error.to_string().contains("format_version") {
            return Err(fail("trace version returned an unstable error"));
        }
        return assert_error_code(&fixture.expect, "unsupported_format_version");
    }
    if mutation == "scripted_format_version" {
        trace.scripted_outcomes.format_version = 2;
        let Err(error) = execute_reducer_trace(&trace) else {
            return Err(fail("unsupported scripted version unexpectedly executed"));
        };
        if !error.to_string().contains("format_version") {
            return Err(fail("scripted version returned an unstable error"));
        }
        return assert_error_code(&fixture.expect, "unsupported_format_version");
    }
    let execution = execute_reducer_trace(&trace).map_err(|error| fail(error.to_string()))?;
    if mutation == "state_version" {
        let mut encoded_state = serde_json::to_value(&execution.kernel_state)
            .map_err(|error| fail(format!("encode kernel state: {error}")))?;
        encoded_state["state_version"] = Value::from(99);
        let Err(error) = from_json::<KernelState>(&encoded_state) else {
            return Err(fail("unsupported state version unexpectedly decoded"));
        };
        if !error.to_string().contains("state_version") {
            return Err(fail("state version returned an unstable error"));
        }
        return assert_error_code(&fixture.expect, "invalid_input_payload");
    }
    let mut batches = execution.committed_batches;
    let target = corrupt_replay_target(mutation, &batches)?;

    let mut encoded = serde_json::to_value(&batches[target])
        .map_err(|error| fail(format!("encode committed batch: {error}")))?;
    mutate_committed_batch_json(mutation, &mut encoded)?;

    let decoded = from_json::<CommittedBatch>(&encoded);
    if matches!(
        mutation,
        "unsupported_format_version" | "unsupported_kind_version" | "derived_event_count"
    ) {
        let Err(error) = decoded else {
            return Err(fail("corrupt record unexpectedly decoded"));
        };
        let code = if error.to_string().contains("format_version") {
            "unsupported_format_version"
        } else if error.to_string().contains("kind_version") {
            "unsupported_kind_version"
        } else if error.to_string().contains("derived_event_ids") {
            "derived_event_count"
        } else {
            "invalid_input_payload"
        };
        return assert_error_code(&fixture.expect, code);
    }
    batches[target] = decoded?;

    let mut kernel = Kernel::default();
    for batch in &batches[..target] {
        kernel
            .apply(batch, 0)
            .map_err(|error| fail(format!("valid replay prefix failed: {}", error.code())))?;
    }
    let state_before = kernel.state().clone();
    let hash_before = state_before
        .state_hash()
        .map_err(|error| fail(format!("state hash before corruption: {}", error.code())))?;
    let Err(error) = kernel.apply(&batches[target], 0) else {
        return Err(fail("corrupt replay batch unexpectedly applied"));
    };
    if kernel.state() != &state_before {
        return Err(fail("rejected corrupt replay changed kernel state"));
    }
    let hash_after = kernel.state().state_hash().map_err(|hash_error| {
        fail(format!(
            "state hash after corruption: {}",
            hash_error.code()
        ))
    })?;
    if hash_after != hash_before {
        return Err(fail("rejected corrupt replay changed state hash"));
    }
    assert_error_code(&fixture.expect, error.code())
}

fn corrupt_replay_target(
    mutation: &str,
    batches: &[CommittedBatch],
) -> Result<usize, PublicApiFixtureError> {
    match mutation {
        "sequence_gap"
        | "range_overlap"
        | "derived_event_count"
        | "unsupported_format_version"
        | "unsupported_kind_version" => Ok(0),
        "identity_substitution"
        | "lane_identity_substitution"
        | "run_identity_substitution"
        | "settlement_tamper" => Ok(1),
        "sibling_reordering" | "record_truncation" => batches
            .iter()
            .position(|batch| batch.records.len() > 1)
            .ok_or_else(|| fail("golden trace has no sibling batch")),
        "derived_event_ordinal" => batches
            .iter()
            .position(|batch| {
                batch
                    .records
                    .iter()
                    .map(|record| record.derived_event_ids().len())
                    .sum::<usize>()
                    > 1
            })
            .ok_or_else(|| fail("golden trace has no multi-event batch")),
        "effect_identity_substitution" => batches
            .iter()
            .position(|batch| {
                batch.records.iter().any(|record| {
                    matches!(
                        record.body(),
                        finstack_ai_kernel::RecordBody::EffectRequested(_)
                    )
                })
            })
            .ok_or_else(|| fail("golden trace has no effect request batch")),
        other => Err(fail(format!("unsupported corrupt-replay mutation {other}"))),
    }
}

fn mutate_committed_batch_json(
    mutation: &str,
    encoded: &mut Value,
) -> Result<(), PublicApiFixtureError> {
    match mutation {
        "sequence_gap" => {
            let sequence = encoded["records"][0]["sequence"]
                .as_u64()
                .ok_or_else(|| fail("record sequence missing"))?;
            encoded["records"][0]["sequence"] = Value::from(sequence + 1);
        }
        "range_overlap" => encoded["first_sequence"] = Value::from(0),
        "identity_substitution" => {
            encoded["records"][0]["session_id"] =
                Value::String("00000000-0000-7000-8000-00000000dead".to_owned());
        }
        "lane_identity_substitution" => {
            encoded["records"][0]["lane_id"] =
                Value::String("00000000-0000-7000-8000-00000000dead".to_owned());
        }
        "run_identity_substitution" => {
            encoded["records"][0]["run_id"] =
                Value::String("00000000-0000-7000-8000-00000000dead".to_owned());
        }
        "effect_identity_substitution" => {
            let records = encoded["records"]
                .as_array_mut()
                .ok_or_else(|| fail("committed records missing"))?;
            let request = records
                .iter_mut()
                .find_map(|record| record["body"].get_mut("effect_requested"))
                .ok_or_else(|| fail("effect request body missing"))?;
            request["effect_id"] = Value::String("00000000-0000-7000-8000-00000000dead".to_owned());
        }
        "sibling_reordering" => {
            let first_sequence = encoded["first_sequence"]
                .as_u64()
                .ok_or_else(|| fail("batch first_sequence missing"))?;
            let records = encoded["records"]
                .as_array_mut()
                .ok_or_else(|| fail("committed records missing"))?;
            records.swap(0, 1);
            for (offset, record) in records.iter_mut().enumerate() {
                record["sequence"] = Value::from(
                    first_sequence
                        + u64::try_from(offset).map_err(|error| fail(error.to_string()))?,
                );
            }
        }
        "record_truncation" => {
            let first_sequence = encoded["first_sequence"]
                .as_u64()
                .ok_or_else(|| fail("batch first_sequence missing"))?;
            encoded["records"]
                .as_array_mut()
                .ok_or_else(|| fail("committed records missing"))?
                .pop();
            encoded["last_sequence"] = Value::from(first_sequence);
        }
        "derived_event_count" => {
            encoded["records"][0]["derived_event_ids"]
                .as_array_mut()
                .ok_or_else(|| fail("derived event ids missing"))?
                .pop();
        }
        "derived_event_ordinal" => mutate_derived_event_ordinal(encoded)?,
        "settlement_tamper" => {
            encoded["records"][0]["body"]["stage_outcome_recorded"]["settlement_digest"] =
                Value::String(
                    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
                );
        }
        "unsupported_format_version" => {
            encoded["records"][0]["format_version"] = Value::from(2);
        }
        "unsupported_kind_version" => {
            encoded["records"][0]["kind_version"] = Value::from(2);
        }
        _ => unreachable!("mutation validated before application"),
    }
    Ok(())
}

fn mutate_derived_event_ordinal(encoded: &mut Value) -> Result<(), PublicApiFixtureError> {
    let records = encoded["records"]
        .as_array_mut()
        .ok_or_else(|| fail("committed records missing"))?;
    let first = records
        .iter()
        .flat_map(|record| record["derived_event_ids"].as_array().into_iter().flatten())
        .next()
        .cloned()
        .ok_or_else(|| fail("first derived event id missing"))?;
    let mut ordinal = 0_usize;
    for record in records {
        let Some(event_ids) = record["derived_event_ids"].as_array_mut() else {
            continue;
        };
        for event_id in event_ids {
            if ordinal == 1 {
                *event_id = first;
                return Ok(());
            }
            ordinal += 1;
        }
    }
    Err(fail("second derived event id missing"))
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
        other => match accepted_hash_version(other) {
            Some(version) => representative_accepted_state(fixture, version)?,
            None => return Err(fail(format!("unsupported kernel-state operation {other}"))),
        },
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

fn accepted_hash_version(operation: &str) -> Option<u16> {
    let digits = operation
        .strip_prefix('v')?
        .strip_suffix("_accepted_hash")?;
    let version = digits.parse().ok()?;
    (2..=6).contains(&version).then_some(version)
}

fn representative_accepted_state(
    fixture: &PublicApiFixture,
    operation_version: u16,
) -> Result<KernelState, PublicApiFixtureError> {
    let input = require_input(fixture)?;
    let state_version = u16::try_from(
        input
            .get("state_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| fail("kernel-state accepted hash requires state_version"))?,
    )
    .map_err(|error| fail(error.to_string()))?;
    if state_version != operation_version {
        return Err(fail(format!(
            "kernel-state accepted hash version mismatch: operation {operation_version}, input {state_version}"
        )));
    }
    let accepted_at = if state_version >= 3 {
        Some(
            finstack_ai_kernel::Timestamp::from_unix_ms(1_000)
                .map_err(|error| fail(error.to_string()))?,
        )
    } else {
        None
    };
    Ok(KernelState {
        state_version,
        session_id: Some(oracle_id::<finstack_ai_kernel::SessionTag>(1)),
        lane_id: Some(oracle_id::<finstack_ai_kernel::LaneTag>(2)),
        accepted: Some(oracle_root_acceptance()?),
        accepted_at,
        ..KernelState::default()
    })
}

fn oracle_id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[..6].copy_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab]);
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    finstack_ai_kernel::Id::from_bytes(bytes)
}

fn oracle_root_acceptance() -> Result<finstack_ai_kernel::RunAccepted, PublicApiFixtureError> {
    let run_id = oracle_id::<finstack_ai_kernel::RunTag>(3);
    finstack_ai_kernel::RunAccepted::try_new(
        run_id,
        finstack_ai_kernel::RunRelation::root(run_id).map_err(|error| fail(error.to_string()))?,
        finstack_ai_kernel::RunSecurityContext::try_new(
            "tenant-a",
            finstack_ai_kernel::PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .map_err(|error| fail(error.to_string()))?,
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .map_err(|error| fail(error.to_string()))?,
        None,
        finstack_ai_kernel::RunLimits::empty(),
        finstack_ai_kernel::RunPropagationPolicy {
            cancellation: finstack_ai_kernel::CancellationPropagation::Cascade,
            deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
            budget: finstack_ai_kernel::BudgetPropagation::SharedScope,
            principal: finstack_ai_kernel::PrincipalPropagation::Inherit,
        },
        finstack_ai_kernel::Digest::raw_json(br#"{"agent":"fixture"}"#),
        None,
    )
    .map_err(|error| fail(error.to_string()))
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
