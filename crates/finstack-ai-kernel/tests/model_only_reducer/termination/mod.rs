use super::*;
use std::collections::BTreeMap;

fn accept_with(harness: &mut Harness, limits: RunLimits, deadline: Option<Timestamp>) {
    harness.apply_input(
        transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
        KernelInput::AcceptRun(AcceptRun {
            session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
            lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
            accepted: root_acceptance_with(limits, deadline),
        }),
    );
}

fn completed_input_with_usage(
    turn_ordinal: u64,
    request_ordinal: u64,
    effect_ordinal: u64,
    message_ordinal: u64,
    now_ms: i64,
    completion_id: &str,
    usage: finstack_ai_kernel::Usage,
) -> KernelInput {
    KernelInput::ModelSettled(ModelSettled {
        turn_id: id::<finstack_ai_kernel::TurnTag>(turn_ordinal),
        model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(request_ordinal),
        outcome: ModelSettlement::Completed {
            completion: EffectCompleted::try_new(
                id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
                output_contract(),
                RawJson::parse(r#"{"text":"hello"}"#).expect("model output"),
                Some(usage),
                vec![],
                provider_ids(),
                Some(completion_id),
                None,
            )
            .expect("completed model effect"),
            assistant_message: assistant_message(message_ordinal, now_ms, "hello"),
        },
    })
}

fn drive_to_awaiting_model_with_limits(limits: RunLimits) -> Harness {
    let mut harness = Harness::default();
    accept_with(&mut harness, limits, None);
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    request_model(&mut harness, 0, false);
    harness
}

fn assert_limit_decision(decision: &Decision, expected: &finstack_ai_kernel::LimitDimension) {
    let RecordBody::LimitReached(reached) = decision.records[0].body() else {
        panic!("expected limit record");
    };
    assert_eq!(&reached.dimension, expected);
    let RecordBody::RunFailed(failed) = decision.records[1].body() else {
        panic!("expected terminal limit failure");
    };
    assert_eq!(failed.error.code.as_str(), "limit_reached");
    assert_eq!(failed.error.category, ErrorCategory::Limit);
    assert!(!failed.error.retryable);
    assert!(decision.actions.is_empty());
}

fn assert_termination_golden(file: &str, harness: &Harness) {
    let source = match file {
        "valid--pr011-limit.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-limit.json"
        ),
        "valid--pr011-retry.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-retry.json"
        ),
        "valid--pr011-model-cancel.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-model-cancel.json"
        ),
        "valid--pr011-tool-cancel.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-tool-cancel.json"
        ),
        "valid--pr011-uncertain.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/termination/valid--pr011-uncertain.json"
        ),
        _ => panic!("unknown termination golden: {file}"),
    };
    let expected: Value = serde_json::from_str(source).expect("parse termination golden");
    let state = harness.kernel.state();
    let records = harness
        .batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(|record| record.body().kind_name())
        .collect::<Vec<_>>();
    let tool_call_ids = state
        .messages
        .iter()
        .filter(|message| message.role() == MessageRole::Tool)
        .map(|message| match &message.content()[0] {
            ContentBlock::ToolResult(result) => {
                u64::from_be_bytes(
                    result.tool_call_id().as_bytes()[8..]
                        .try_into()
                        .expect("ordinal bytes"),
                ) & 0x3fff_ffff_ffff_ffff
            }
            _ => panic!("tool message must contain result"),
        })
        .collect::<Vec<_>>();
    let terminal = state.terminal.as_ref().map(|terminal| match terminal {
        TerminalState::Completed(_) => "completed",
        TerminalState::Failed(_) => "failed",
        TerminalState::Cancelled(_) => "cancelled",
    });
    let mut replayed = Kernel::default();
    let mut transient = 0_u64;
    for batch in &harness.batches {
        let events = replayed.apply(batch, transient).expect("golden replay");
        transient += u64::try_from(events.len()).expect("event count");
    }
    let actual = json!({
        "format_version": 1,
        "phase": state.phase,
        "cycle": state.cycle,
        "state_version": state.state_version,
        "records": records,
        "retry_attempts": state.retry.attempts,
        "has_pending_retry": state.retry.pending.is_some(),
        "last_limit": state.last_limit.as_ref().map(|limit| &limit.dimension),
        "cancellation_outstanding": state.cancellation.as_ref().map_or(0, |value| value.outstanding_effects.len()),
        "cancellation_uncertain": state.cancellation.as_ref().map_or(0, |value| value.uncertain_effects.len()),
        "tool_call_ids": tool_call_ids,
        "active_tool_batch": state.active_tool_batch.is_some(),
        "terminal": terminal,
        "replay_hash_equal": replayed.state().state_hash().expect("replay hash") == state.state_hash().expect("live hash"),
    });
    assert_eq!(actual, expected, "{file}");
}

include!("cancellation.rs");
include!("limits.rs");
include!("usage_and_cost.rs");
include!("propagation.rs");
include!("races.rs");
