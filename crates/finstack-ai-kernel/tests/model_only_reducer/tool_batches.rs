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

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end flow proves grouping, reverse completion, events, finalization, and replay"
)]
fn mixed_groups_reverse_completion_preserves_source_order_and_finalize_candidate() {
    let calls = vec![
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
        call(CALL_D, "delta"),
    ];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);

    let plans = vec![
        execute(
            &calls[0],
            ToolExecutionMode::Parallel,
            ToolFailurePolicy::ReturnToModel,
        ),
        execute(
            &calls[1],
            ToolExecutionMode::Parallel,
            ToolFailurePolicy::ReturnToModel,
        ),
        execute(
            &calls[2],
            ToolExecutionMode::Sequential,
            ToolFailurePolicy::ReturnToModel,
        ),
        execute(
            &calls[3],
            ToolExecutionMode::Barrier,
            ToolFailurePolicy::ReturnToModel,
        ),
    ];
    let opened = harness.apply_input(
        tool_env(
            1_600,
            &[1_000, 1_001, 1_002, 1_003],
            &[1_000, 1_001],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C, TOOL_EFFECT_D],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: plans.into(),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    assert_eq!(opened.actions.len(), 2);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AwaitingTools));
    assert_tool_golden("valid--pr010-mixed-groups.json", &harness);

    let reverse = settle_tool(
        &mut harness,
        1_700,
        &[1_010],
        &[1_010],
        &[],
        TOOL_EFFECT_B,
        &calls[1],
    );
    assert_eq!(reverse.records.len(), 1, "later result remains buffered");
    assert_eq!(tool_message_call_ids(&harness), Vec::<u64>::new());
    assert_tool_golden("valid--pr010-reverse-parallel.json", &harness);

    let first = settle_tool(
        &mut harness,
        1_800,
        &[1_020, 1_021, 1_022, 1_023],
        &[1_020, 1_021, 1_022, 1_023, 1_024, 1_025],
        &[1_100, 1_101],
        TOOL_EFFECT_A,
        &calls[0],
    );
    assert_eq!(
        first.actions,
        vec![PostCommitAction::ExecuteEffect {
            effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_C),
        }]
    );
    assert_eq!(tool_message_call_ids(&harness), vec![CALL_A, CALL_B]);

    settle_tool(
        &mut harness,
        1_900,
        &[1_030, 1_031, 1_032],
        &[1_030, 1_031, 1_032, 1_033],
        &[1_102],
        TOOL_EFFECT_C,
        &calls[2],
    );
    settle_tool(
        &mut harness,
        2_000,
        &[1_040, 1_041, 1_042],
        &[1_040, 1_041, 1_042],
        &[1_103],
        TOOL_EFFECT_D,
        &calls[3],
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AfterToolBatch));
    assert_eq!(
        tool_message_call_ids(&harness),
        vec![CALL_A, CALL_B, CALL_C, CALL_D]
    );
    assert_eq!(
        tool_event_call_ids(&harness),
        vec![CALL_A, CALL_B, CALL_C, CALL_D]
    );

    harness.apply_input(
        transition_env(2_100, &[1_050], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterToolBatch, ReducerStageOutcome::Continue),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    harness.apply_input(
        transition_env(2_200, &[1_060, 1_061], &[1_060], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    );
    let TerminalState::Completed(completed) =
        harness.kernel.state().terminal.as_ref().expect("terminal")
    else {
        panic!("expected completed run");
    };
    assert_eq!(
        completed.result_message_id,
        id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE)
    );
    assert_tool_golden("valid--pr010-finalize-after-batch.json", &harness);
    assert_replay_prefixes(&harness);
}

#[test]
fn unknown_tool_is_synthetic_and_continue_model_uses_state_v2() {
    let calls = vec![call(CALL_A, "missing")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    let unknown = fixture_error_with_message("unknown_tool", "unknown tool: missing");
    let decision = harness.apply_input(
        tool_env(
            1_600,
            &[2_000, 2_001, 2_002, 2_003],
            &[2_000, 2_001],
            &[TOOL_EFFECT_A],
            &[2_100],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([ToolCallPlan::SyntheticClosure(
                    finstack_ai_kernel::SyntheticToolClosure {
                        call: calls[0].clone(),
                        execution: ToolExecutionMode::Sequential,
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        error: unknown.clone(),
                    },
                )]),
                continuation: ToolBatchContinuation::ContinueModel,
            },
        ),
    );
    assert!(decision.actions.is_empty());
    assert_eq!(harness.kernel.state().state_version, 2);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AfterToolBatch));
    let result = last_tool_result(&harness);
    assert!(result.is_error());
    let ContentBlock::Json(value) = &result.content()[0] else {
        panic!("synthetic result must contain JSON");
    };
    assert!(value.value().as_str().contains("unknown_tool"));
    assert_tool_golden("valid--pr010-unknown-tool.json", &harness);

    harness.apply_input(
        transition_env(1_700, &[2_010], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterToolBatch, ReducerStageOutcome::Continue),
    );
    assert_eq!(harness.kernel.state().cycle, 1);
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::PreparingContext)
    );
    let encoded = serde_json::to_value(harness.kernel.state()).expect("state v2 JSON");
    assert_eq!(encoded.get("state_version"), Some(&json!(2)));
    assert!(encoded.get("tool_calls").is_some());
    assert!(harness.kernel.state().state_hash().is_ok());
    assert_tool_golden("valid--pr010-continue-model.json", &harness);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end failure flow proves draining and undispatched closure together"
)]
fn fail_run_drains_parallel_group_and_closes_only_undispatched_calls() {
    let calls = vec![
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
    ];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    let plans = vec![
        execute(
            &calls[0],
            ToolExecutionMode::Parallel,
            ToolFailurePolicy::FailRun,
        ),
        execute(
            &calls[1],
            ToolExecutionMode::Parallel,
            ToolFailurePolicy::ReturnToModel,
        ),
        execute(
            &calls[2],
            ToolExecutionMode::Sequential,
            ToolFailurePolicy::ReturnToModel,
        ),
    ];
    harness.apply_input(
        tool_env(
            1_600,
            &[3_000, 3_001, 3_002, 3_003],
            &[3_000, 3_001],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: plans.into(),
                continuation: ToolBatchContinuation::ContinueModel,
            },
        ),
    );
    let failure = EffectFailed::try_new(
        id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
        tool_contract(),
        fixture_error("tool_framework_failed"),
        None,
        Some("tool-failure-a"),
    )
    .expect("tool failure");
    harness.apply_input(
        tool_env(
            1_700,
            &[3_010, 3_011],
            &[3_010, 3_011, 3_012],
            &[],
            &[3_100],
            &[],
            &[],
        ),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Failed(failure),
        }),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AwaitingTools));
    let active = harness
        .kernel
        .state()
        .active_tool_batch
        .as_ref()
        .expect("active batch");
    assert!(matches!(
        active.calls[2].status,
        ActiveToolCallStatus::Undispatched
    ));

    let decision = settle_tool(
        &mut harness,
        1_800,
        &[3_020, 3_021, 3_022, 3_023],
        &[3_020, 3_021, 3_022, 3_023, 3_024],
        &[3_101, 3_102],
        TOOL_EFFECT_B,
        &calls[1],
    );
    assert!(decision.actions.is_empty());
    assert!(decision.records.iter().all(|record| !matches!(
        record.body(),
        RecordBody::EffectRequested(requested)
            if requested.effect_id() == id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_C)
    )));
    let closed = harness
        .kernel
        .state()
        .last_tool_batch
        .as_ref()
        .expect("failed close");
    assert!(matches!(closed.outcome, ToolBatchOutcome::Failed { .. }));
    assert_eq!(
        tool_message_call_ids(&harness),
        vec![CALL_A, CALL_B, CALL_C]
    );
    let result = last_tool_result(&harness);
    let ContentBlock::Json(value) = &result.content()[0] else {
        panic!("aborted result JSON");
    };
    assert!(value.value().as_str().contains("tool_batch_aborted"));
    assert_tool_golden("valid--pr010-fail-run.json", &harness);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the failure/deferral interleaving proves that unresolved effects are never fabricated closed"
)]
fn fail_run_waits_for_a_deferred_parallel_call_before_aborting_later_groups() {
    let calls = vec![
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
    ];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[3_200, 3_201, 3_202, 3_203],
            &[3_200, 3_201],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([
                    execute(
                        &calls[0],
                        ToolExecutionMode::Parallel,
                        ToolFailurePolicy::FailRun,
                    ),
                    execute(
                        &calls[1],
                        ToolExecutionMode::Parallel,
                        ToolFailurePolicy::ReturnToModel,
                    ),
                    execute(
                        &calls[2],
                        ToolExecutionMode::Sequential,
                        ToolFailurePolicy::ReturnToModel,
                    ),
                ]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );

    let deferred = EffectDeferred {
        effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_B),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tool.fixture").expect("tool component"),
            "external-parallel-job",
            RawJson::parse("{}").expect("metadata"),
        )
        .expect("external handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
        output_contract: tool_contract(),
    };
    harness.apply_input(
        tool_env(1_700, &[3_210], &[3_210], &[], &[], &[], &[]),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Deferred(deferred),
        }),
    );

    let failure = EffectFailed::try_new(
        id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
        tool_contract(),
        fixture_error("tool_framework_failed"),
        None,
        Some("tool-failure-with-deferred-peer"),
    )
    .expect("tool failure");
    let failed = harness.apply_input(
        tool_env(
            1_800,
            &[3_220, 3_221],
            &[3_220, 3_221, 3_222],
            &[],
            &[3_230],
            &[],
            &[],
        ),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Failed(failure),
        }),
    );
    assert!(
        failed
            .records
            .iter()
            .all(|record| !matches!(record.body(), RecordBody::ToolBatchClosed(_)))
    );
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::AwaitingExternal)
    );
    let active = harness
        .kernel
        .state()
        .active_tool_batch
        .as_ref()
        .expect("unresolved batch remains active");
    assert!(matches!(
        active.calls[1].status,
        ActiveToolCallStatus::Requested {
            deferred: Some(_),
            ..
        }
    ));
    assert!(matches!(
        active.calls[2].status,
        ActiveToolCallStatus::Undispatched
    ));
    assert_eq!(tool_message_call_ids(&harness), vec![CALL_A]);

    let external = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_B),
            "external-parallel-completion",
            ExternalEffectOutcome::Completed {
                output: tool_result_output(&calls[1], "external peer result"),
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("external completion"),
        assistant_message: None,
    });
    let closed = harness.apply_input(
        tool_env(
            1_900,
            &[3_240, 3_241, 3_242, 3_243],
            &[3_240, 3_241, 3_242, 3_243, 3_244],
            &[],
            &[3_250, 3_251],
            &[],
            &[],
        ),
        external,
    );
    assert!(closed.actions.is_empty());
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AfterToolBatch));
    assert!(matches!(
        harness
            .kernel
            .state()
            .last_tool_batch
            .as_ref()
            .expect("failed close")
            .outcome,
        ToolBatchOutcome::Failed { .. }
    ));
    assert_eq!(
        tool_message_call_ids(&harness),
        vec![CALL_A, CALL_B, CALL_C]
    );
    assert_replay_prefixes(&harness);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one external lifecycle proves deferral, resumption, duplicate, and conflict classification"
)]
fn deferred_tool_resumes_externally_and_duplicate_or_conflict_is_stable() {
    let calls = vec![call(CALL_A, "alpha")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[4_000, 4_001, 4_002],
            &[4_000],
            &[TOOL_EFFECT_A],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &calls[0],
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    let deferred = EffectDeferred {
        effect_id: id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tool.fixture").expect("tool component"),
            "external-tool-job",
            RawJson::parse("{}").expect("metadata"),
        )
        .expect("external handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
        output_contract: tool_contract(),
    };
    let deferred_input = KernelInput::ToolBatchSettled(ToolBatchSettled {
        tool_batch_id: id::<ToolBatchTag>(BATCH),
        outcome: ToolSettlement::Deferred(deferred.clone()),
    });
    harness.apply_input(
        tool_env(1_700, &[4_010], &[4_010], &[], &[], &[], &[]),
        deferred_input.clone(),
    );
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::AwaitingExternal)
    );
    let duplicate = harness
        .kernel
        .decide(&empty_env(1_701), deferred_input)
        .expect("equal deferred duplicate");
    assert!(duplicate.records.is_empty());

    let output = tool_result_output(&calls[0], "external result");
    let external = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
            "external-tool-completion",
            ExternalEffectOutcome::Completed {
                output,
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("external completion"),
        assistant_message: None,
    });
    harness.apply_input(
        tool_env(
            1_800,
            &[4_020, 4_021, 4_022],
            &[4_020, 4_021, 4_022],
            &[],
            &[4_100],
            &[],
            &[],
        ),
        external.clone(),
    );
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::AfterToolBatch));
    let duplicate = harness
        .kernel
        .decide(&empty_env(1_801), external)
        .expect("equal external duplicate");
    assert!(duplicate.records.is_empty());

    let conflict = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
            "external-tool-completion",
            ExternalEffectOutcome::Completed {
                output: tool_result_output(&calls[0], "changed"),
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("conflicting completion"),
        assistant_message: None,
    });
    assert_eq!(
        harness.kernel.decide(&empty_env(1_802), conflict),
        Err(KernelError::ConflictingCompletionId)
    );
    assert_tool_golden("valid--pr010-deferred-external.json", &harness);
    assert_tool_golden("valid--pr010-duplicate-conflict.json", &harness);
}

#[test]
fn malformed_plans_and_effect_contracts_fail_closed() {
    let calls = vec![call(CALL_A, "alpha")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);

    let mutated = call(CALL_A, "mutated");
    let bad_plan = stage_input(
        0,
        Stage::BeforeToolBatch,
        ReducerStageOutcome::ToolBatchPrepared {
            calls: Arc::from([execute(
                &mutated,
                ToolExecutionMode::Sequential,
                ToolFailurePolicy::ReturnToModel,
            )]),
            continuation: ToolBatchContinuation::Finalize,
        },
    );
    assert_eq!(
        harness.kernel.decide(
            &tool_env(
                1_600,
                &[5_000, 5_001, 5_002],
                &[5_000],
                &[TOOL_EFFECT_A],
                &[],
                &[BATCH],
                &[],
            ),
            bad_plan,
        ),
        Err(KernelError::ToolBatchPlanMismatch)
    );

    let mut wrong_contract = match execute(
        &calls[0],
        ToolExecutionMode::Sequential,
        ToolFailurePolicy::ReturnToModel,
    ) {
        ToolCallPlan::Execute(value) => value,
        ToolCallPlan::SyntheticClosure(_) => unreachable!(),
    };
    wrong_contract.output_contract = output_contract();
    assert_eq!(
        harness.kernel.decide(
            &tool_env(
                1_600,
                &[5_010, 5_011, 5_012],
                &[5_010],
                &[TOOL_EFFECT_A],
                &[],
                &[BATCH],
                &[],
            ),
            stage_input(
                0,
                Stage::BeforeToolBatch,
                ReducerStageOutcome::ToolBatchPrepared {
                    calls: Arc::from([ToolCallPlan::Execute(wrong_contract)]),
                    continuation: ToolBatchContinuation::Finalize,
                },
            ),
        ),
        Err(KernelError::ToolEffectContractMismatch)
    );
}

#[test]
fn mismatched_tool_results_fail_closed() {
    let calls = vec![call(CALL_A, "alpha")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[5_020, 5_021, 5_022],
            &[5_020],
            &[TOOL_EFFECT_A],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &calls[0],
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    let wrong_call = call(CALL_B, "beta");
    let wrong_result = EffectCompleted::try_new(
        id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
        tool_contract(),
        tool_result_output(&wrong_call, "wrong association"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("wrong-result"),
        None,
    )
    .expect("wrong associated output");
    assert_eq!(
        harness.kernel.decide(
            &tool_env(1_700, &[5_030], &[5_030], &[], &[], &[], &[]),
            KernelInput::ToolBatchSettled(ToolBatchSettled {
                tool_batch_id: id::<ToolBatchTag>(BATCH),
                outcome: ToolSettlement::Completed(wrong_result),
            }),
        ),
        Err(KernelError::ToolResultMismatch)
    );
}

#[test]
fn duplicate_assistant_call_ids_fail_before_batch_planning() {
    let duplicate = call(CALL_C, "duplicate");
    let mut duplicate_harness = Harness::default();
    accept(&mut duplicate_harness);
    settle_before_run(&mut duplicate_harness);
    prepare_context(&mut duplicate_harness, 0, false);
    request_model(&mut duplicate_harness, 0, false);
    let message = Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
        MessageRole::Assistant,
        vec![
            ContentBlock::ToolCall(duplicate.clone()),
            ContentBlock::ToolCall(duplicate),
        ],
        timestamp(1_400),
        None,
        provider_ids(),
        Metadata::empty(),
    )
    .expect("structurally valid assistant calls");
    let input = KernelInput::ModelSettled(ModelSettled {
        turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
        model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
        outcome: ModelSettlement::Completed {
            completion: completed_effect(EFFECT_ONE, "duplicate-calls", "calls"),
            assistant_message: message,
        },
    });
    assert_eq!(
        duplicate_harness.kernel.decide(
            &tool_env(
                1_400,
                &[5_040, 5_041],
                &[5_040, 5_041],
                &[],
                &[FINAL_MESSAGE_ONE],
                &[],
                &[CALL_C, CALL_C],
            ),
            input,
        ),
        Err(KernelError::DuplicateToolCall)
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the canonical allocated-ID queue precedence is asserted in one transactional table"
)]
fn tool_allocated_id_shortage_and_extra_queues_fail_transactionally() {
    let calls = [call(CALL_A, "alpha")];
    let input = || {
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &calls[0],
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        )
    };
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    let before = harness.kernel.state().clone();
    assert!(matches!(
        harness.kernel.decide(
            &tool_env(1_600, &[6_000, 6_001, 6_002], &[6_000], &[], &[], &[], &[]),
            input(),
        ),
        Err(KernelError::AllocatedIdsExhausted { kind: "effect_ids" })
    ));
    assert!(matches!(
        harness.kernel.decide(
            &tool_env(
                1_600,
                &[6_000, 6_001, 6_002],
                &[6_000],
                &[TOOL_EFFECT_A],
                &[],
                &[],
                &[],
            ),
            input(),
        ),
        Err(KernelError::AllocatedIdsExhausted {
            kind: "tool_batch_ids"
        })
    ));
    assert!(matches!(
        harness.kernel.decide(
            &tool_env(
                1_600,
                &[6_000, 6_001, 6_002],
                &[6_000],
                &[TOOL_EFFECT_A, TOOL_EFFECT_B],
                &[],
                &[BATCH],
                &[],
            ),
            input(),
        ),
        Err(KernelError::UnusedAllocatedIds { kind: "effect_ids" })
    ));
    assert_eq!(harness.kernel.state(), &before);

    harness.apply_input(
        tool_env(
            1_600,
            &[6_010, 6_011, 6_012],
            &[6_010],
            &[TOOL_EFFECT_A],
            &[],
            &[BATCH],
            &[],
        ),
        input(),
    );
    let settlement = KernelInput::ToolBatchSettled(ToolBatchSettled {
        tool_batch_id: id::<ToolBatchTag>(BATCH),
        outcome: ToolSettlement::Completed(tool_completed(TOOL_EFFECT_A, &calls[0])),
    });
    let before = harness.kernel.state().clone();
    assert!(matches!(
        harness.kernel.decide(
            &tool_env(
                1_700,
                &[6_020, 6_021, 6_022],
                &[6_020, 6_021, 6_022],
                &[],
                &[],
                &[],
                &[],
            ),
            settlement.clone(),
        ),
        Err(KernelError::AllocatedIdsExhausted {
            kind: "message_ids"
        })
    ));
    assert!(matches!(
        harness.kernel.decide(
            &tool_env(
                1_700,
                &[6_020, 6_021, 6_022],
                &[6_020, 6_021, 6_022],
                &[],
                &[6_030, 6_031],
                &[],
                &[],
            ),
            settlement,
        ),
        Err(KernelError::UnusedAllocatedIds {
            kind: "message_ids"
        })
    ));
    assert_eq!(harness.kernel.state(), &before);
}

#[test]
fn apply_rejects_a_truncated_contiguous_tool_result_prefix() {
    let calls = [call(CALL_A, "alpha"), call(CALL_B, "beta")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[5_000, 5_001, 5_002, 5_003],
            &[5_000, 5_001],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: calls
                    .iter()
                    .map(|call| {
                        execute(
                            call,
                            ToolExecutionMode::Parallel,
                            ToolFailurePolicy::ReturnToModel,
                        )
                    })
                    .collect::<Vec<_>>()
                    .into(),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    settle_tool(
        &mut harness,
        1_700,
        &[5_010],
        &[5_010],
        &[],
        TOOL_EFFECT_B,
        &calls[1],
    );

    let env = tool_env(
        1_800,
        &[5_020, 5_021, 5_022, 5_023],
        &[5_020, 5_021, 5_022, 5_023, 5_024],
        &[],
        &[5_030, 5_031],
        &[],
        &[],
    );
    let decision = harness
        .kernel
        .decide(
            &env,
            KernelInput::ToolBatchSettled(ToolBatchSettled {
                tool_batch_id: id::<ToolBatchTag>(BATCH),
                outcome: ToolSettlement::Completed(tool_completed(TOOL_EFFECT_A, &calls[0])),
            }),
        )
        .expect("valid complete-prefix decision");
    assert_eq!(
        decision_body_names(&decision),
        vec![
            "effect_completed",
            "tool_call_settled",
            "tool_call_settled",
            "tool_batch_closed",
        ]
    );
    let truncated = commit_records(
        decision.expected_sequence,
        &decision.records[..2],
        None,
        IdentityOverride::default(),
        21_800,
    );
    assert_apply_rejected_without_mutation(&mut harness.kernel, &truncated, "invalid_record_order");
}

#[test]
fn apply_rejects_partial_dispatch_of_the_next_execution_group() {
    let calls = [
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
    ];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[5_100, 5_101, 5_102],
            &[5_100],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([
                    execute(
                        &calls[0],
                        ToolExecutionMode::Sequential,
                        ToolFailurePolicy::ReturnToModel,
                    ),
                    execute(
                        &calls[1],
                        ToolExecutionMode::Parallel,
                        ToolFailurePolicy::ReturnToModel,
                    ),
                    execute(
                        &calls[2],
                        ToolExecutionMode::Parallel,
                        ToolFailurePolicy::ReturnToModel,
                    ),
                ]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );

    let env = tool_env(
        1_700,
        &[5_110, 5_111, 5_112, 5_113],
        &[5_110, 5_111, 5_112, 5_113, 5_114],
        &[],
        &[5_120],
        &[],
        &[],
    );
    let decision = harness
        .kernel
        .decide(
            &env,
            KernelInput::ToolBatchSettled(ToolBatchSettled {
                tool_batch_id: id::<ToolBatchTag>(BATCH),
                outcome: ToolSettlement::Completed(tool_completed(TOOL_EFFECT_A, &calls[0])),
            }),
        )
        .expect("valid next-group decision");
    assert_eq!(
        decision_body_names(&decision),
        vec![
            "effect_completed",
            "tool_call_settled",
            "effect_requested",
            "effect_requested",
        ]
    );
    let truncated = commit_records(
        decision.expected_sequence,
        &decision.records[..3],
        None,
        IdentityOverride::default(),
        21_700,
    );
    assert_apply_rejected_without_mutation(&mut harness.kernel, &truncated, "invalid_record_order");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one compatibility test keeps all PR-010 strict wire and state-v2 invariants together"
)]
fn tool_wire_contracts_state_v2_and_event_ordinals_are_strict() {
    let calls = [call(CALL_A, "alpha")];
    let plan = execute(
        &calls[0],
        ToolExecutionMode::Sequential,
        ToolFailurePolicy::ReturnToModel,
    );
    assert_json_round_trip_and_unknown_fields(&plan);
    assert_json_round_trip_and_unknown_fields(&ReducerStageOutcome::ToolBatchPrepared {
        calls: Arc::from([plan.clone()]),
        continuation: ToolBatchContinuation::Finalize,
    });
    assert_json_round_trip_and_unknown_fields(&KernelInput::ToolBatchSettled(ToolBatchSettled {
        tool_batch_id: id::<ToolBatchTag>(BATCH),
        outcome: ToolSettlement::Completed(tool_completed(TOOL_EFFECT_A, &calls[0])),
    }));
    assert!(serde_json::from_value::<KernelInput>(json!({"future_input": {}})).is_err());
    assert!(serde_json::from_value::<ToolSettlement>(json!({"future": {}})).is_err());

    let mut bounded = serde_json::to_value(ReducerStageOutcome::ToolBatchPrepared {
        calls: vec![plan; finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS].into(),
        continuation: ToolBatchContinuation::Finalize,
    })
    .expect("bounded plan JSON");
    assert!(serde_json::from_value::<ReducerStageOutcome>(bounded.clone()).is_ok());
    let extra = bounded
        .get("tool_batch_prepared")
        .and_then(|value| value.get("calls"))
        .and_then(Value::as_array)
        .and_then(|calls| calls.first())
        .cloned()
        .expect("plan entry");
    bounded
        .get_mut("tool_batch_prepared")
        .and_then(|value| value.get_mut("calls"))
        .and_then(Value::as_array_mut)
        .expect("plan array")
        .push(extra);
    assert!(serde_json::from_value::<ReducerStageOutcome>(bounded).is_err());

    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[5_100, 5_101, 5_102],
            &[5_100],
            &[TOOL_EFFECT_A],
            &[],
            &[BATCH],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &calls[0],
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    settle_tool(
        &mut harness,
        1_700,
        &[5_110, 5_111, 5_112],
        &[5_110, 5_111, 5_112],
        &[5_120],
        TOOL_EFFECT_A,
        &calls[0],
    );

    let opened: ToolBatchOpened = find_body(&harness.batches, |body| match body {
        RecordBody::ToolBatchOpened(value) => Some(value.clone()),
        _ => None,
    });
    let settled: ToolCallSettled = find_body(&harness.batches, |body| match body {
        RecordBody::ToolCallSettled(value) => Some(value.clone()),
        _ => None,
    });
    let closed: ToolBatchClosed = find_body(&harness.batches, |body| match body {
        RecordBody::ToolBatchClosed(value) => Some(value.clone()),
        _ => None,
    });
    assert_json_round_trip_and_unknown_fields(&opened);
    assert_json_round_trip_and_unknown_fields(&settled);
    assert_json_round_trip_and_unknown_fields(&closed);

    let settled_envelope = harness
        .batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .find(|record| matches!(record.body(), RecordBody::ToolCallSettled(_)))
        .expect("settled envelope");
    assert_eq!(settled_envelope.derived_event_ids().len(), 2);
    assert_eq!(
        RunEvent::try_from_record(settled_envelope, 0, 0)
            .expect("message event")
            .kind(),
        RunEventKind::MessageFinalized
    );
    assert_eq!(
        RunEvent::try_from_record(settled_envelope, 1, 1)
            .expect("tool event")
            .kind(),
        RunEventKind::ToolSettled
    );
    assert!(RunEvent::try_from_record(settled_envelope, 2, 2).is_err());

    let state = harness.kernel.state();
    assert_eq!(state.state_version, 2);
    assert_json_round_trip_and_unknown_fields(state);
    let state_json = serde_json::to_value(state).expect("state v2 JSON");
    for required in [
        "active_tool_batch",
        "tool_calls",
        "tool_settlements",
        "last_tool_batch",
    ] {
        let mut missing = state_json.clone();
        missing
            .as_object_mut()
            .expect("state object")
            .remove(required);
        assert!(
            serde_json::from_value::<finstack_ai_kernel::KernelState>(missing).is_err(),
            "v2 accepted missing {required}"
        );
    }
    for repeated in ["tool_calls", "tool_settlements"] {
        let mut duplicate = state_json.clone();
        let entries = duplicate
            .get_mut(repeated)
            .and_then(Value::as_array_mut)
            .expect("state map projection");
        entries.push(entries[0].clone());
        assert!(
            serde_json::from_value::<finstack_ai_kernel::KernelState>(duplicate).is_err(),
            "v2 accepted duplicate {repeated} key"
        );
    }
    let mut inconsistent = state_json;
    inconsistent
        .as_object_mut()
        .expect("state object")
        .insert("phase".to_owned(), json!("awaiting_tools"));
    assert!(serde_json::from_value::<finstack_ai_kernel::KernelState>(inconsistent).is_err());

    let mut v1 =
        serde_json::to_value(finstack_ai_kernel::KernelState::default()).expect("state v1 JSON");
    v1.as_object_mut()
        .expect("state object")
        .insert("active_tool_batch".to_owned(), Value::Null);
    assert!(serde_json::from_value::<finstack_ai_kernel::KernelState>(v1).is_err());
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the deterministic permutation generator includes exact transactional ID accounting"
)]
fn generated_parallel_completion_permutations_are_source_ordered_and_replayable() {
    let permutations = [
        [0_usize, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let calls = [
        call(CALL_A, "alpha"),
        call(CALL_B, "beta"),
        call(CALL_C, "gamma"),
    ];
    let effects = [TOOL_EFFECT_A, TOOL_EFFECT_B, TOOL_EFFECT_C];
    let mut final_hash = None;

    for permutation in permutations {
        let mut harness = model_with_calls(&calls);
        settle_after_model_for_tools(&mut harness);
        let base = 10_000;
        harness.apply_input(
            tool_env(
                2_000,
                &[base, base + 1, base + 2, base + 3, base + 4],
                &[base, base + 1, base + 2],
                &effects,
                &[],
                &[BATCH],
                &[],
            ),
            stage_input(
                0,
                Stage::BeforeToolBatch,
                ReducerStageOutcome::ToolBatchPrepared {
                    calls: calls
                        .iter()
                        .map(|call| {
                            execute(
                                call,
                                ToolExecutionMode::Parallel,
                                ToolFailurePolicy::ReturnToModel,
                            )
                        })
                        .collect::<Vec<_>>()
                        .into(),
                    continuation: ToolBatchContinuation::Finalize,
                },
            ),
        );

        let mut completed = [false; 3];
        let mut finalized = 0_usize;
        for (arrival, source_index) in permutation.into_iter().enumerate() {
            completed[source_index] = true;
            let new_finalized = completed
                .iter()
                .position(|done| !done)
                .unwrap_or(completed.len());
            let emitted = new_finalized - finalized;
            let closing = new_finalized == calls.len();
            let record_count = 1 + emitted + usize::from(closing);
            let event_count = 1 + 2 * emitted;
            let arrival = u64::try_from(arrival).expect("arrival");
            let allocation = base + 100 + arrival * 20;
            let record_ids = (0..record_count)
                .map(|offset| allocation + u64::try_from(offset).expect("record offset"))
                .collect::<Vec<_>>();
            let event_ids = (0..event_count)
                .map(|offset| allocation + u64::try_from(offset).expect("event offset"))
                .collect::<Vec<_>>();
            let message_ids = (0..emitted)
                .map(|offset| {
                    base + 500 + u64::try_from(finalized + offset).expect("source message offset")
                })
                .collect::<Vec<_>>();
            settle_tool(
                &mut harness,
                2_100,
                &record_ids,
                &event_ids,
                &message_ids,
                effects[source_index],
                &calls[source_index],
            );
            finalized = new_finalized;
        }

        assert_eq!(
            tool_message_call_ids(&harness),
            vec![CALL_A, CALL_B, CALL_C],
            "completion permutation {permutation:?}"
        );
        assert_eq!(
            tool_event_call_ids(&harness),
            vec![CALL_A, CALL_B, CALL_C],
            "completion permutation {permutation:?}"
        );
        assert_replay_prefixes(&harness);
        let hash = harness.kernel.state().state_hash().expect("v2 hash");
        if let Some(expected) = final_hash {
            assert_eq!(hash, expected, "completion permutation {permutation:?}");
        } else {
            final_hash = Some(hash);
        }
    }
}

pub(super) fn model_with_calls(calls: &[ToolCallBlock]) -> Harness {
    let mut harness = Harness::default();
    accept(&mut harness);
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

fn tool_contract() -> EffectOutputContract {
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
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-continue-model.json"
        ),
        "valid--pr010-deferred-external.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-deferred-external.json"
        ),
        "valid--pr010-duplicate-conflict.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-duplicate-conflict.json"
        ),
        "valid--pr010-fail-run.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-fail-run.json"
        ),
        "valid--pr010-finalize-after-batch.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-finalize-after-batch.json"
        ),
        "valid--pr010-mixed-groups.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-mixed-groups.json"
        ),
        "valid--pr010-reverse-parallel.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-reverse-parallel.json"
        ),
        "valid--pr010-unknown-tool.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/tool-batch/valid--pr010-unknown-tool.json"
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
