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
    assert!(failed
        .records
        .iter()
        .all(|record| !matches!(record.body(), RecordBody::ToolBatchClosed(_))));
    assert_eq!(
        harness.kernel.state().phase(),
        Some(RunPhase::AwaitingExternal)
    );
    let active = harness
        .kernel
        .state()
        .active_tool_batch()
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
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::AfterToolBatch));
    assert!(matches!(
        harness
            .kernel
            .state()
            .last_tool_batch()
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
        harness.kernel.state().phase(),
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
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::AfterToolBatch));
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
fn second_defer_on_a_different_call_in_the_same_group_applies() {
    let calls = vec![call(CALL_A, "alpha"), call(CALL_B, "beta")];
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
                calls: Arc::from([
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
                ]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    let first = deferred_tool(TOOL_EFFECT_A, "external-first");
    harness.apply_input(
        tool_env(1_700, &[5_010], &[5_010], &[], &[], &[], &[]),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Deferred(first),
        }),
    );
    assert_eq!(
        harness.kernel.state().phase(),
        Some(RunPhase::AwaitingExternal)
    );
    let second = deferred_tool(TOOL_EFFECT_B, "external-second");
    let decision = harness.apply_input(
        tool_env(1_800, &[5_020], &[5_020], &[], &[], &[], &[]),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Deferred(second),
        }),
    );
    assert_eq!(decision.records.len(), 1);
    assert!(matches!(
        decision.records[0].body(),
        RecordBody::EffectDeferred(_)
    ));
    assert_eq!(
        harness.kernel.state().phase(),
        Some(RunPhase::AwaitingExternal)
    );
    let active = harness
        .kernel
        .state()
        .active_tool_batch()
        .expect("batch remains active");
    assert!(matches!(
        active.calls[0].status,
        ActiveToolCallStatus::Requested {
            deferred: Some(_),
            ..
        }
    ));
    assert!(matches!(
        active.calls[1].status,
        ActiveToolCallStatus::Requested {
            deferred: Some(_),
            ..
        }
    ));
}

#[test]
fn decided_tool_settlement_counts_apply_and_replay() {
    // `tool_settlement_shape` uses `ActiveToolBatch::predicted_settlement_counts`
    // as the count owner. Decide output that applies pins those two paths.
    let calls = vec![call(CALL_A, "alpha"), call(CALL_B, "beta")];
    let mut harness = model_with_calls(&calls);
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[7_000, 7_001, 7_002, 7_003],
            &[7_000, 7_001],
            &[TOOL_EFFECT_A, TOOL_EFFECT_B],
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
                        ToolFailurePolicy::ReturnToModel,
                    ),
                    execute(
                        &calls[1],
                        ToolExecutionMode::Parallel,
                        ToolFailurePolicy::ReturnToModel,
                    ),
                ]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    let output = tool_result_output(&calls[0], "first");
    harness.apply_input(
        tool_env(
            1_700,
            &[7_010, 7_011],
            &[7_010, 7_011, 7_012],
            &[],
            &[7_020],
            &[],
            &[],
        ),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Completed(
                EffectCompleted::try_new(
                    id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
                    tool_contract(),
                    output,
                    None,
                    vec![],
                    ProviderIds::empty(),
                    Some("tool-a"),
                    None,
                )
                .expect("completed tool"),
            ),
        }),
    );
    let replayed = replay(&harness.batches);
    assert_eq!(replayed.state(), harness.kernel.state());
    assert_eq!(
        replayed.state().state_hash().expect("replay hash"),
        harness.kernel.state().state_hash().expect("live hash")
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "two-batch lifecycle is required to pin prior-batch digest lookup"
)]
fn prior_batch_external_redelivery_uses_the_original_batch_while_a_newer_batch_is_active() {
    const BATCH_TWO: u64 = 500;
    let first = call(CALL_A, "alpha");
    let second = call(CALL_B, "beta");
    let mut harness = model_with_calls(std::slice::from_ref(&first));
    settle_after_model_for_tools(&mut harness);
    harness.apply_input(
        tool_env(
            1_600,
            &[6_000, 6_001, 6_002],
            &[6_000],
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
                    &first,
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::ContinueModel,
            },
        ),
    );
    harness.apply_input(
        tool_env(1_700, &[6_010], &[6_010], &[], &[], &[], &[]),
        KernelInput::ToolBatchSettled(ToolBatchSettled {
            tool_batch_id: id::<ToolBatchTag>(BATCH),
            outcome: ToolSettlement::Deferred(deferred_tool(
                TOOL_EFFECT_A,
                "external-prior-batch",
            )),
        }),
    );
    let output = tool_result_output(&first, "prior batch result");
    let external = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
            "prior-batch-completion",
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
            &[6_020, 6_021, 6_022],
            &[6_020, 6_021, 6_022],
            &[],
            &[6_100],
            &[],
            &[],
        ),
        external.clone(),
    );
    assert_eq!(harness.kernel.state().phase(), Some(RunPhase::AfterToolBatch));

    harness.apply_input(
        transition_env(2_000, &[6_030], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterToolBatch, ReducerStageOutcome::Continue),
    );
    prepare_context(&mut harness, 1, true);
    request_model(&mut harness, 1, true);
    let message = Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_TWO),
        MessageRole::Assistant,
        vec![
            ContentBlock::Text(TextBlock::try_new("calling more tools").expect("text")),
            ContentBlock::ToolCall(second.clone()),
        ],
        timestamp(2_300),
        None,
        provider_ids(),
        Metadata::empty(),
    )
    .expect("second-cycle tool calls");
    harness.apply_input(
        tool_env(
            2_300,
            &[17, 18],
            &[7, 8],
            &[],
            &[FINAL_MESSAGE_TWO],
            &[],
            &[CALL_B],
        ),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_TWO),
            model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_TWO),
            outcome: ModelSettlement::Completed {
                completion: completed_effect(EFFECT_TWO, "model-tools-two", "calling more tools"),
                assistant_message: message,
            },
        }),
    );
    settle_after_model(&mut harness, 1, true);
    harness.apply_input(
        tool_env(
            2_500,
            &[6_040, 6_041, 6_042],
            &[6_040],
            &[TOOL_EFFECT_B],
            &[],
            &[BATCH_TWO],
            &[],
        ),
        stage_input(
            1,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([execute(
                    &second,
                    ToolExecutionMode::Sequential,
                    ToolFailurePolicy::ReturnToModel,
                )]),
                continuation: ToolBatchContinuation::Finalize,
            },
        ),
    );
    let state = harness.kernel.state();
    assert_eq!(
        state
            .active_tool_batch()
            .expect("newer batch")
            .opened
            .tool_batch_id,
        id::<ToolBatchTag>(BATCH_TWO)
    );
    assert_eq!(
        state
            .tool_calls()
            .get(&id::<finstack_ai_kernel::ToolCallTag>(CALL_A))
            .expect("prior call")
            .tool_batch_id,
        Some(id::<ToolBatchTag>(BATCH))
    );

    let duplicate = harness
        .kernel
        .decide(&empty_env(2_600), external)
        .expect("equal prior-batch redelivery");
    assert!(duplicate.records.is_empty());
    assert!(
        duplicate
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code() == "duplicate_settlement")
    );

    let conflict = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion::try_new(
            id::<finstack_ai_kernel::EffectTag>(TOOL_EFFECT_A),
            "prior-batch-conflict",
            ExternalEffectOutcome::Completed {
                output: tool_result_output(&first, "changed"),
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("conflicting completion"),
        assistant_message: None,
    });
    assert_eq!(
        harness.kernel.decide(&empty_env(2_601), conflict),
        Err(KernelError::ConflictingSettlement)
    );
}

fn deferred_tool(effect_ordinal: u64, handle: &str) -> EffectDeferred {
    EffectDeferred {
        effect_id: id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tool.fixture").expect("tool component"),
            handle,
            RawJson::parse("{}").expect("metadata"),
        )
        .expect("external handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
        output_contract: tool_contract(),
    }
}
