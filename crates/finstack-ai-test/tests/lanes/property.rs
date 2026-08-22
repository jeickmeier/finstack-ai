const PROPERTY_CASES: u32 = 256;
const PROPERTY_SEED: u64 = 0x5eed_0130_0000_0001;

fn lane_property_config() -> ProptestConfig {
    ProptestConfig {
        cases: PROPERTY_CASES,
        rng_seed: RngSeed::Fixed(PROPERTY_SEED),
        failure_persistence: Some(Box::new(
            proptest::test_runner::FileFailurePersistence::SourceParallel("proptest-regressions"),
        )),
        max_shrink_iters: 10_000,
        ..ProptestConfig::default()
    }
}

#[test]
fn lane_created_and_moved_fail_closed_on_unknown_state_bearing_fields() {
    assert!(
        serde_json::from_str::<LaneCreated>(r#"{"name":"research","owner":"evil"}"#).is_err(),
        "LaneCreated must deny unknown fields"
    );
    assert!(
        serde_json::from_str::<LaneMoved>(
            r#"{"leaf_id":"01234567-89ab-7cde-89ab-0123456789ab","forked_from":"x"}"#
        )
        .is_err(),
        "LaneMoved must deny unknown fields"
    );
}

#[test]
fn lane_invariants_hold_for_fixed_seed_sibling_sequences() {
    let mut runner = proptest::test_runner::TestRunner::new(lane_property_config());
    runner
        .run(&(1_usize..=3), |siblings| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let store = memory_store();
                let session = open_session(Arc::clone(&store)).await;
                let mut lane_ids_created = vec![id::<LaneTag>(2)];
                for offset in 0..siblings {
                    let lane = 50 + u64::try_from(offset).expect("lane");
                    session
                        .create_lane(
                            format!("lane-{offset}"),
                            None,
                            lane_ids(lane, 200 + lane * 10, false),
                        )
                        .await
                        .expect("create");
                    lane_ids_created.push(id(lane));
                }
                let mut owners = Vec::new();
                for (index, lane) in lane_ids_created.iter().copied().enumerate() {
                    let run = 3 + u64::try_from(index).expect("run");
                    session.try_acquire_run(lane, id(run)).expect("one owner");
                    assert_eq!(
                        session.try_acquire_run(lane, id(run + 30)),
                        Err(SessionError::LaneBusy),
                        "busy lane rejects a second owner"
                    );
                    let coordinator = session
                        .accept_run(
                            lane,
                            root_acceptance(run),
                            env(
                                1_000 + i64::try_from(index).expect("now"),
                                10 + run,
                                11 + run,
                                12 + run,
                            ),
                        )
                        .await
                        .expect("accept");
                    assert_eq!(coordinator.state().phase(), Some(RunPhase::BeforeRun));
                    owners.push(coordinator);
                }
                for owner in &owners {
                    assert!(owner.state().cancellation().is_none());
                }
                drop(owners);
                drop(session);
                let restored = SessionRuntime::open(store, id(1), "tenant-a")
                    .await
                    .expect("restore");
                for (index, lane) in lane_ids_created.iter().copied().enumerate() {
                    let run = 3 + u64::try_from(index).expect("run");
                    let coordinator = restored
                        .coordinator_for_run(Some(id(run)))
                        .await
                        .expect("restored run");
                    let class = classify_phase(coordinator.state().phase().expect("phase"));
                    assert_eq!(class, LegalRestore::Retryable);
                    let _ = lane;
                }
                Ok(())
            })
        })
        .expect("lane properties");
}
