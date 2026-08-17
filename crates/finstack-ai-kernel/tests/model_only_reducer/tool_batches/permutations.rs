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
