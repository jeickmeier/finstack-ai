#[test]
fn apply_table_rejects_session_lane_and_run_identity_mismatches() {
    let mut harness = Harness::default();
    accept(&mut harness);
    settle_before_run(&mut harness);
    let decision = harness
        .kernel
        .decide(
            &transition_env(1_200, &[3, 4], &[], &[], &[TURN_ONE], &[], &[]),
            stage_input(
                0,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from(context_messages()),
                },
            ),
        )
        .expect("context decision");
    let cases = [
        IdentityOverride {
            session: Some(id::<finstack_ai_kernel::SessionTag>(900)),
            ..IdentityOverride::default()
        },
        IdentityOverride {
            lane: Some(id::<finstack_ai_kernel::LaneTag>(901)),
            ..IdentityOverride::default()
        },
        IdentityOverride {
            run: Some(id::<finstack_ai_kernel::RunTag>(902)),
            ..IdentityOverride::default()
        },
    ];
    for (index, identity) in cases.into_iter().enumerate() {
        let timestamp_offset = i64::try_from(index).expect("fixture index fits i64");
        let batch = commit_records(
            decision.expected_sequence,
            &decision.records,
            None,
            identity,
            21_000 + timestamp_offset,
        );
        assert_apply_rejected_without_mutation(
            &mut harness.kernel,
            &batch,
            "record_identity_mismatch",
        );
    }
}

#[test]
fn terminal_state_rejects_later_committed_mutation_without_state_change() {
    let mut terminal = drive_to_completed();
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<finstack_ai_kernel::RecordTag>(999),
        id::<finstack_ai_kernel::SessionTag>(SESSION),
        id::<finstack_ai_kernel::LaneTag>(LANE),
        Some(id::<finstack_ai_kernel::RunTag>(RUN)),
        timestamp(9_999),
        vec![],
        RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
            cursor: StageCursor {
                cycle: 1,
                stage: Stage::BeforeRun,
            },
            disposition: StageDisposition::Continued,
            settlement_digest: canonical_digest(
                "stage-settlement",
                &json!({"terminal": "mutation"}),
            ),
        }),
    )
    .expect("terminal mutation draft");
    let next = terminal.kernel.state().last_applied_sequence() + 1;
    let batch = commit_records(next, &[draft], None, IdentityOverride::default(), 29_999);
    assert_apply_rejected_without_mutation(
        &mut terminal.kernel,
        &batch,
        "terminal_state_immutable",
    );
}

