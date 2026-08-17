use super::*;
use finstack_ai_kernel::{
    ActiveToolCallStatus, EffectFailed, ToolBatchClosed, ToolBatchContinuation, ToolBatchOpened,
    ToolBatchOutcome, ToolBatchSettled, ToolBatchTag, ToolCallBlock, ToolCallPlan, ToolCallSettled,
    ToolExecutionMode, ToolFailurePolicy, ToolId, ToolResultBlock, ToolSettlement,
    ValidatedToolCall,
};

pub(super) const CALL_A: u64 = 301;
const CALL_B: u64 = 302;
const CALL_C: u64 = 303;
const CALL_D: u64 = 304;
pub(super) const BATCH: u64 = 400;
pub(super) const TOOL_EFFECT_A: u64 = 401;
const TOOL_EFFECT_B: u64 = 402;
const TOOL_EFFECT_C: u64 = 403;
const TOOL_EFFECT_D: u64 = 404;

pub(super) fn model_with_calls(calls: &[ToolCallBlock]) -> Harness {
    model_with_calls_and_limits(calls, RunLimits::empty())
}

pub(super) fn model_with_calls_and_limits(calls: &[ToolCallBlock], limits: RunLimits) -> Harness {
    let mut harness = Harness::default();
    harness.apply_input(
        transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
        KernelInput::AcceptRun(AcceptRun {
            session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
            lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
            accepted: root_acceptance_with(limits, None),
        }),
    );
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    request_model(&mut harness, 0, false);
    let mut content = vec![ContentBlock::Text(
        TextBlock::try_new("calling tools").expect("text"),
    )];
    content.extend(calls.iter().cloned().map(ContentBlock::ToolCall));
    let message = Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
        MessageRole::Assistant,
        content,
        timestamp(1_400),
        None,
        provider_ids(),
        Metadata::empty(),
    )
    .expect("assistant tool calls");
    harness.apply_input(
        tool_env(
            1_400,
            &[7, 8],
            &[3, 4],
            &[],
            &[FINAL_MESSAGE_ONE],
            &[],
            &calls
                .iter()
                .map(|call| ordinal(call.tool_call_id()))
                .collect::<Vec<_>>(),
        ),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
            model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
            outcome: ModelSettlement::Completed {
                completion: completed_effect(EFFECT_ONE, "model-tools", "calling tools"),
                assistant_message: message,
            },
        }),
    );
    assert_eq!(harness.kernel.state().state_version, 2);
    harness
}

pub(super) fn settle_after_model_for_tools(harness: &mut Harness) {
    harness.apply_input(
        transition_env(1_500, &[900], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterModel, ReducerStageOutcome::Continue),
    );
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::BeforeToolBatch)
    );
}

fn settle_tool(
    harness: &mut Harness,
    now_ms: i64,
    records: &[u64],
    events: &[u64],
    messages: &[u64],
    effect: u64,
    call: &ToolCallBlock,
) -> Decision {
    harness.apply_input(
        tool_env(now_ms, records, events, &[], messages, &[], &[]),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Completed(tool_completed(effect, call)),
        }),
    )
}

pub(super) fn call(ordinal: u64, name: &str) -> ToolCallBlock {
    ToolCallBlock::try_new(
        id::<finstack_ai_kernel::ToolCallTag>(ordinal),
        name,
        RawJson::parse(json!({"value": ordinal}).to_string()).expect("arguments"),
    )
    .expect("tool call")
}

pub(super) fn execute(
    call: &ToolCallBlock,
    execution: ToolExecutionMode,
    failure_policy: ToolFailurePolicy,
) -> ToolCallPlan {
    ToolCallPlan::Execute(ValidatedToolCall {
        call: call.clone(),
        tool_id: ToolId::parse("finstack.tools.fixture").expect("tool id"),
        component: None,
        output_contract: tool_contract(),
        retry_safety: RetrySafety::IdempotentWithKey,
        deadline: None,
        execution,
        failure_policy,
    })
}

pub(super) fn tool_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ToolResult,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"tool_result"}"#),
    }
}

pub(super) fn tool_completed(effect: u64, call: &ToolCallBlock) -> EffectCompleted {
    EffectCompleted::try_new(
        id::<finstack_ai_kernel::EffectTag>(effect),
        tool_contract(),
        tool_result_output(call, &format!("result-{effect}")),
        None,
        vec![],
        ProviderIds::empty(),
        Some(format!("tool-completion-{effect}")),
        None,
    )
    .expect("tool completion")
}

fn tool_result_output(call: &ToolCallBlock, text: &str) -> RawJson {
    let result = ToolResultBlock::try_new(
        *call.tool_call_id(),
        vec![ContentBlock::Text(
            TextBlock::try_new(text).expect("result text"),
        )],
        false,
    )
    .expect("result block");
    RawJson::parse(serde_json::to_string(&result).expect("result JSON")).expect("raw result")
}

pub(super) fn tool_env(
    now_ms: i64,
    record_ids: &[u64],
    event_ids: &[u64],
    effect_ids: &[u64],
    message_ids: &[u64],
    tool_batch_ids: &[u64],
    tool_call_ids: &[u64],
) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now_ms),
        ids: AllocatedIds::try_new(
            record_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::RecordTag>)
                .collect(),
            event_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::EventTag>)
                .collect(),
            effect_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::EffectTag>)
                .collect(),
            vec![],
            message_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::MessageTag>)
                .collect(),
            vec![],
            vec![],
            tool_batch_ids
                .iter()
                .copied()
                .map(id::<ToolBatchTag>)
                .collect(),
            tool_call_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::ToolCallTag>)
                .collect(),
            vec![],
            vec![],
        )
        .expect("tool allocated IDs"),
    }
}

fn ordinal<T: IdTag>(value: &Id<T>) -> u64 {
    u64::from_be_bytes(value.as_bytes()[8..].try_into().expect("ordinal bytes"))
        & 0x3fff_ffff_ffff_ffff
}

fn tool_message_call_ids(harness: &Harness) -> Vec<u64> {
    harness
        .kernel
        .state()
        .messages
        .iter()
        .filter(|message| message.role() == MessageRole::Tool)
        .flat_map(finstack_ai_kernel::Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(ordinal(result.tool_call_id())),
            _ => None,
        })
        .collect()
}

fn tool_event_call_ids(harness: &Harness) -> Vec<u64> {
    harness
        .events
        .iter()
        .filter_map(|event| match event.body() {
            finstack_ai_kernel::RunEventBody::ToolSettled { tool_call_id } => {
                Some(ordinal(tool_call_id))
            }
            _ => None,
        })
        .collect()
}

fn last_tool_result(harness: &Harness) -> &ToolResultBlock {
    let message = harness
        .kernel
        .state()
        .messages
        .iter()
        .rev()
        .find(|message| message.role() == MessageRole::Tool)
        .expect("tool message");
    let ContentBlock::ToolResult(result) = &message.content()[0] else {
        panic!("tool result block");
    };
    result
}

fn fixture_error_with_message(code: &str, message: &str) -> ErrorDescriptor {
    ErrorDescriptor::new(code, message, ErrorCategory::Tool, false).expect("tool error")
}

fn assert_replay_prefixes(harness: &Harness) {
    let mut incremental = Kernel::default();
    let mut transient = 0_u64;
    for (prefix, batch) in harness.batches.iter().enumerate() {
        let events = incremental
            .apply(batch, transient)
            .expect("incremental replay prefix");
        transient += u64::try_from(events.len()).expect("event count");
        let expected_hash = incremental.state().state_hash().expect("prefix hash");

        let mut restarted = Kernel::default();
        let mut restarted_transient = 0_u64;
        for replay_batch in harness.batches.iter().take(prefix + 1) {
            let events = restarted
                .apply(replay_batch, restarted_transient)
                .expect("restart replay prefix");
            restarted_transient += u64::try_from(events.len()).expect("event count");
        }
        assert_eq!(
            restarted.state().state_hash().expect("restarted hash"),
            expected_hash,
            "prefix {prefix} hash"
        );
    }
    assert_eq!(incremental.state(), harness.kernel.state());
}

fn assert_tool_golden(file: &str, harness: &Harness) {
    let source = match file {
        "valid--pr010-continue-model.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-continue-model.json"
        ),
        "valid--pr010-deferred-external.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-deferred-external.json"
        ),
        "valid--pr010-duplicate-conflict.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-duplicate-conflict.json"
        ),
        "valid--pr010-fail-run.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-fail-run.json"
        ),
        "valid--pr010-finalize-after-batch.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-finalize-after-batch.json"
        ),
        "valid--pr010-mixed-groups.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-mixed-groups.json"
        ),
        "valid--pr010-reverse-parallel.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-reverse-parallel.json"
        ),
        "valid--pr010-unknown-tool.json" => include_str!(
            "../../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-unknown-tool.json"
        ),
        _ => panic!("unknown tool golden: {file}"),
    };
    let expected: Value = serde_json::from_str(source)
        .unwrap_or_else(|error| panic!("parse tool golden {file}: {error}"));

    let records = harness
        .batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .skip_while(|record| !matches!(record.body(), RecordBody::ToolBatchOpened(_)))
        .map(|record| record.body().kind_name())
        .collect::<Vec<_>>();
    let state = harness.kernel.state();
    let last_batch_outcome = state
        .last_tool_batch
        .as_ref()
        .map(|closed| match &closed.outcome {
            ToolBatchOutcome::ContinueModel => "continue_model",
            ToolBatchOutcome::Finalize => "finalize",
            ToolBatchOutcome::Failed { .. } => "failed",
        });
    let terminal_candidate = state
        .terminal_candidate
        .as_ref()
        .map(|candidate| match candidate {
            TerminalCandidate::Completed { .. } => "completed",
            TerminalCandidate::Failed { .. } => "failed",
        });
    let terminal = state.terminal.as_ref().map(|terminal| match terminal {
        TerminalState::Completed(_) => "completed",
        TerminalState::Failed(_) => "failed",
        TerminalState::Cancelled(_) => "cancelled",
    });
    let mut replayed = Kernel::default();
    let mut transient = 0_u64;
    for batch in &harness.batches {
        let events = replayed.apply(batch, transient).expect("golden replay");
        transient = transient
            .checked_add(u64::try_from(events.len()).expect("event count"))
            .expect("event sequence");
    }
    let replay_hash_equal = replayed.state().state_hash().expect("replay hash")
        == state.state_hash().expect("live hash");
    let actual = json!({
        "format_version": 1,
        "phase": state.phase,
        "cycle": state.cycle,
        "state_version": state.state_version,
        "records_after_open": records,
        "tool_message_call_ids": tool_message_call_ids(harness),
        "tool_event_call_ids": tool_event_call_ids(harness),
        "active_batch": state.active_tool_batch.is_some(),
        "last_batch_outcome": last_batch_outcome,
        "terminal_candidate": terminal_candidate,
        "terminal": terminal,
        "replay_hash_equal": replay_hash_equal,
    });
    assert_eq!(actual, expected, "{file}");
}

include!("execution.rs");
include!("deferral.rs");
include!("validation.rs");
include!("wire.rs");
include!("permutations.rs");
