//! Byte-exact `kernel-state` hash oracles for schemas 2–6.
//!
//! Schema 1 stays pinned in `successful.rs`. Each later version uses one
//! accepted root run with no further progress. The oracle hashes an independent
//! JCS JSON projection with `Digest::domain_separated("kernel-state", version, …)`
//! and compares that digest to [`KernelState::state_hash`]. Expected hex lives
//! under `fixtures/compatibility/public-rust-api/v1/kernel-state/`.

use std::sync::Arc;

use super::*;
use finstack_ai_kernel::{
    ContentBlock, KernelState, Message, MessageRole, Metadata, ProviderIds, RawJson, ToolCallBlock,
    ToolCallIdentity, ToolCallTag,
};
use serde_json::Value;

const ACCEPTED_AT_MS: i64 = 1_000;

fn kernel_state_digest(schema_version: u16, projection: &Value) -> Digest {
    let canonical = serde_json_canonicalizer::to_string(projection).expect("canonical JSON");
    Digest::domain_separated(
        "kernel-state",
        u32::from(schema_version),
        canonical.as_bytes(),
    )
    .expect("kernel-state domain")
}

fn representative_state(state_version: u16) -> KernelState {
    KernelState {
        state_version,
        session_id: Some(id::<finstack_ai_kernel::SessionTag>(SESSION)),
        lane_id: Some(id::<finstack_ai_kernel::LaneTag>(LANE)),
        accepted: Some(root_acceptance()),
        accepted_at: (state_version >= 3).then(|| timestamp(ACCEPTED_AT_MS)),
        ..KernelState::default()
    }
}

fn accepted_projection() -> Value {
    let mut accepted = serde_json::to_value(root_acceptance()).expect("accepted JSON");
    make_hash_acceptance_explicit(&mut accepted);
    accepted
}

fn make_hash_acceptance_explicit(accepted: &mut Value) {
    let object = accepted.as_object_mut().expect("accepted object");
    object.insert("effective_deadline".to_owned(), Value::Null);
    let relation = object
        .get_mut("relation")
        .and_then(Value::as_object_mut)
        .expect("relation");
    for field in [
        "parent_run_id",
        "parent_effect_id",
        "budget_scope_id",
        "external_work_ref",
    ] {
        relation.insert(field.to_owned(), Value::Null);
    }
    object
        .get_mut("security")
        .and_then(Value::as_object_mut)
        .expect("security")
        .insert("delegated_from".to_owned(), Value::Null);
    let limits = object
        .get_mut("limits")
        .and_then(Value::as_object_mut)
        .expect("limits");
    for field in [
        "max_model_requests",
        "max_turns",
        "max_tool_calls",
        "max_parallel_tools",
        "max_input_tokens",
        "max_output_tokens",
        "max_context_bytes",
        "max_output_bytes",
        "max_retries",
        "max_wall_time",
        "max_cost",
    ] {
        limits.insert(field.to_owned(), Value::Null);
    }
    limits.insert("extension_counters".to_owned(), json!({}));
}

fn empty_limit_usage() -> Value {
    json!({
        "model_requests": 0,
        "turns": 0,
        "tool_calls": 0,
        "max_parallel_tools": 0,
        "input_tokens": 0,
        "output_tokens": 0,
        "context_bytes": 0,
        "output_bytes": 0,
        "retries": 0,
        "wall_time": 0,
        "cost": null,
        "extension_counters": [],
    })
}

fn empty_retry() -> Value {
    json!({
        "attempts": 0,
        "pending": null,
        "timer_firings": [],
    })
}

fn independent_projection(state_version: u16) -> Value {
    let mut projection = json!({
        "state_version": state_version,
        "last_applied_sequence": 0,
        "session_id": id::<finstack_ai_kernel::SessionTag>(SESSION),
        "lane_id": id::<finstack_ai_kernel::LaneTag>(LANE),
        "accepted": accepted_projection(),
        "phase": null,
        "cycle": 0,
        "current_turn": null,
        "messages": [],
        "pending_model_effect": null,
        "terminal_candidate": null,
        "stage_settlements": [],
        "model_settlements": [],
        "completion_identities": [],
        "active_tool_batch": null,
        "tool_calls": [],
        "tool_settlements": [],
        "last_tool_batch": null,
        "terminal": null,
    });
    if state_version >= 3 {
        let object = projection.as_object_mut().expect("projection object");
        object.insert(
            "accepted_at".to_owned(),
            serde_json::to_value(timestamp(ACCEPTED_AT_MS)).expect("accepted_at JSON"),
        );
        object.insert("limit_usage".to_owned(), empty_limit_usage());
        object.insert("cancellation".to_owned(), Value::Null);
        object.insert("retry".to_owned(), empty_retry());
        object.insert("last_limit".to_owned(), Value::Null);
        object.insert("suspension".to_owned(), Value::Null);
    }
    if state_version >= 4 {
        let object = projection.as_object_mut().expect("projection object");
        object.insert("output_configuration".to_owned(), Value::Null);
        object.insert("active_capabilities".to_owned(), json!([]));
        object.insert("resolved_plan_digest".to_owned(), Value::Null);
        object.insert("final_result".to_owned(), Value::Null);
        object.insert("validation_failure".to_owned(), Value::Null);
    }
    if state_version >= 5 {
        let object = projection.as_object_mut().expect("projection object");
        object.insert("child_preparations".to_owned(), json!([]));
        object.insert("budget_reservations".to_owned(), json!([]));
        object.insert("budget_charges".to_owned(), json!([]));
    }
    if state_version >= 6 {
        let object = projection.as_object_mut().expect("projection object");
        object.insert("pending_interaction".to_owned(), Value::Null);
        object.insert("resolution_identities".to_owned(), json!([]));
        object.insert("last_interaction_terminal".to_owned(), Value::Null);
    }
    projection
}

fn expected_hex(state_version: u16) -> String {
    let source = match state_version {
        2 => include_str!(
            "../../../../fixtures/compatibility/public-rust-api/v1/kernel-state/valid--v2-accepted-hash.json"
        ),
        3 => include_str!(
            "../../../../fixtures/compatibility/public-rust-api/v1/kernel-state/valid--v3-accepted-hash.json"
        ),
        4 => include_str!(
            "../../../../fixtures/compatibility/public-rust-api/v1/kernel-state/valid--v4-accepted-hash.json"
        ),
        5 => include_str!(
            "../../../../fixtures/compatibility/public-rust-api/v1/kernel-state/valid--v5-accepted-hash.json"
        ),
        6 => include_str!(
            "../../../../fixtures/compatibility/public-rust-api/v1/kernel-state/valid--v6-accepted-hash.json"
        ),
        other => panic!("no accepted-hash fixture for state version {other}"),
    };
    let fixture: Value = serde_json::from_str(source).expect("parse accepted-hash fixture");
    fixture["expect"]["state_hash"]
        .as_str()
        .expect("fixture state_hash")
        .to_owned()
}

#[test]
fn accepted_states_match_independent_jcs_oracles_for_versions_2_through_6() {
    let mut live_hex = Vec::new();
    let mut expected = Vec::new();
    for state_version in 2_u16..=6 {
        let state = representative_state(state_version);
        let projection = independent_projection(state_version);
        let independent = kernel_state_digest(state_version, &projection);
        let live = state
            .state_hash()
            .unwrap_or_else(|error| panic!("state hash at v{state_version}: {error}"));
        assert_eq!(
            live, independent,
            "live hash must match independent JCS oracle at v{state_version}"
        );
        live_hex.push((state_version, live.to_hex()));
        expected.push((state_version, expected_hex(state_version)));
    }
    assert_eq!(
        live_hex, expected,
        "pinned digest hex for accepted v2–v6 states"
    );
}

fn tool_call_bearing_state() -> KernelState {
    let source_message_id = id::<finstack_ai_kernel::MessageTag>(902);
    let turn_id = id::<finstack_ai_kernel::TurnTag>(903);
    let call = ToolCallBlock::try_new_with_provider_call_id(
        id::<ToolCallTag>(901),
        "lookup_price",
        RawJson::parse(r#"{"q":1}"#).expect("args"),
        Some("call-provider-1"),
    )
    .expect("call");
    let identity = ToolCallIdentity {
        cycle: 0,
        turn_id,
        source_message_id,
        tool_batch_id: None,
        effect_id: None,
        call: call.clone(),
    };
    let message = Message::try_new(
        source_message_id,
        MessageRole::Assistant,
        vec![ContentBlock::ToolCall(call.clone())],
        timestamp(4_000),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message");
    KernelState {
        state_version: 2,
        messages: Arc::new(vec![message]),
        tool_calls: [(*call.tool_call_id(), identity)].into_iter().collect(),
        ..KernelState::default()
    }
}

fn tool_call_independent_projection() -> Value {
    json!({
        "state_version": 2,
        "last_applied_sequence": 0,
        "session_id": null,
        "lane_id": null,
        "accepted": null,
        "phase": null,
        "cycle": 0,
        "current_turn": null,
        "messages": [{
            "id": id::<finstack_ai_kernel::MessageTag>(902),
            "role": "assistant",
            "content": [{
                "kind": "tool_call",
                "tool_call_id": id::<ToolCallTag>(901),
                "tool_name": "lookup_price",
                "arguments": {"q": 1},
                "provider_call_id": "call-provider-1",
            }],
            "created_at": timestamp(4_000),
            "model": null,
            "provider_ids": {
                "request_id": null,
                "response_id": null,
                "continuation_id": null,
            },
            "metadata": {},
        }],
        "pending_model_effect": null,
        "terminal_candidate": null,
        "stage_settlements": [],
        "model_settlements": [],
        "completion_identities": [],
        "active_tool_batch": null,
        "tool_calls": [{
            "tool_call_id": id::<ToolCallTag>(901),
            "cycle": 0,
            "turn_id": id::<finstack_ai_kernel::TurnTag>(903),
            "source_message_id": id::<finstack_ai_kernel::MessageTag>(902),
            "tool_batch_id": null,
            "effect_id": null,
            "call": {
                "tool_call_id": id::<ToolCallTag>(901),
                "tool_name": "lookup_price",
                "arguments": {"q": 1},
                "provider_call_id": "call-provider-1",
            },
        }],
        "tool_settlements": [],
        "last_tool_batch": null,
        "terminal": null,
    })
}

#[test]
fn tool_call_bearing_state_hash_includes_provider_call_id() {
    let state = tool_call_bearing_state();
    let independent = kernel_state_digest(2, &tool_call_independent_projection());
    let live = state.state_hash().expect("tool-call state hash");
    assert_eq!(
        live, independent,
        "live hash must include ContentProjection::ToolCall.provider_call_id"
    );
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/compatibility/public-rust-api/v1/kernel-state/valid--tool-call-hash.json"
    ))
    .expect("parse tool-call hash fixture");
    assert_eq!(
        live.to_hex(),
        fixture["expect"]["state_hash"]
            .as_str()
            .expect("fixture state_hash"),
        "pinned tool-call digest hex"
    );
}
