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
