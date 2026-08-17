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
    assert_error_code(
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
        "duplicate_tool_call",
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
