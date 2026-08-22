fn fixed_id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn fixed_timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

/// Mirrors `coordinator.rs`'s own `tests::acceptance()` fixture (same field
/// values), parameterized on `effective_deadline` so the deadline path can
/// exercise a run whose deadline has already passed.
fn acceptance(effective_deadline: Option<Timestamp>) -> RunAccepted {
    let run_id = fixed_id::<RunTag>(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        effective_deadline,
        finstack_ai_kernel::RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"agent":"fixture"}"#),
        None,
    )
    .expect("acceptance")
}

/// Base fixture reused by every test below: a minimally valid accepted,
/// running `KernelState`. Individual tests override `phase`/`cycle`/
/// `accepted` via the test-fixture mutators where the scenario needs it.
fn accepted_state() -> KernelState {
    let mut state = KernelState::default();
    state.set_session_id(Some(fixed_id::<SessionTag>(1)));
    state.set_lane_id(Some(fixed_id::<LaneTag>(2)));
    state.set_accepted(Some(acceptance(None)));
    state.set_accepted_at(Some(fixed_timestamp(1_000)));
    state.set_phase(Some(RunPhase::BeforeRun));
    state.set_cycle(0);
    state
}

fn state_at_phase(base: &KernelState, phase: RunPhase) -> KernelState {
    let mut state = base.clone();
    state.set_phase(Some(phase));
    state
}

fn with_candidate(mut state: KernelState, candidate: TerminalCandidate) -> KernelState {
    state.set_terminal_candidate(Some(candidate));
    state
}

/// Deterministic, collision-free random source: each `fill_bytes` call
/// tiles the buffer with the bytes of a monotonic counter, so repeated
/// allocations inside one test never collide the way two calls against a
/// truly fixed byte pattern would.
#[derive(Default)]
struct CountingRandom(AtomicU64);

impl RandomSource for CountingRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        let counter = self.0.fetch_add(1, Ordering::Relaxed);
        let bytes = counter.to_be_bytes();
        for (index, slot) in buf.iter_mut().enumerate() {
            *slot = bytes[index % bytes.len()];
        }
        Ok(())
    }
}

fn test_sources() -> SettlementSources<ExternalClock, CountingRandom> {
    sources_at(1_000)
}

fn sources_at(now_ms: i64) -> SettlementSources<ExternalClock, CountingRandom> {
    SettlementSources::try_new(
        ExternalClock::new(fixed_timestamp(now_ms)),
        CountingRandom::default(),
    )
    .expect("sources")
}

/// The test named in the task-3 brief: allocation for the same outcome
/// must differ by stage. This is exactly the property `StageIds::for_outcome`
/// (reverted by this task) got wrong by keying allocation on the outcome
/// alone.
#[test]
fn fail_allocation_differs_between_before_finalize_and_other_stages() {
    let state = accepted_state();
    let sources = test_sources();
    let fail = ReducerStageOutcome::Fail(
        ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
            .expect("descriptor"),
    );

    let at_finalize = stage_allocation(
        &state,
        StageCursor {
            cycle: 0,
            stage: Stage::BeforeFinalize,
        },
        &fail,
        &sources,
    )
    .expect("finalize allocation");

    let at_before_run = stage_allocation(
        &state,
        StageCursor {
            cycle: 0,
            stage: Stage::BeforeRun,
        },
        &fail,
        &sources,
    )
    .expect("before_run allocation");

    assert_eq!(
        at_finalize.record_ids().len(),
        2,
        "BeforeFinalize Fail needs 2 records"
    );
    assert_eq!(
        at_finalize.event_ids().len(),
        1,
        "BeforeFinalize Fail needs 1 event"
    );
    assert_eq!(
        at_before_run.record_ids().len(),
        1,
        "BeforeRun Fail needs 1 record"
    );
    assert_eq!(
        at_before_run.event_ids().len(),
        0,
        "BeforeRun Fail needs 0 events"
    );
}

#[test]
fn run_deadline_fail_closed_uses_a_stage_legal_outcome() {
    // fail_closed_on_run_deadline settles Stage::BeforeToolBatch. The kernel
    // rejects Continue there (decide.rs:1107-1111), so the fail-closed path
    // must not emit Continue.
    let outcome = run_deadline_outcome().expect("run deadline outcome");
    assert!(
        !matches!(outcome, ReducerStageOutcome::Continue),
        "BeforeToolBatch cannot accept Continue; got {outcome:?}"
    );
}

/// `stage_allocation` in isolation (no run deadline configured, so the
/// kernel's own `decide_limit` deadline-crossing precedence — see the two
/// tests below — cannot confound the result): it must mirror
/// `stage_id_requirements` and refuse `Continue` at `BeforeToolBatch`,
/// exactly the outcome the pre-existing bug submitted.
#[test]
fn stage_allocation_rejects_continue_at_before_tool_batch() {
    let state = state_at_phase(&accepted_state(), RunPhase::BeforeToolBatch);
    let sources = test_sources();
    let cursor = StageCursor {
        cycle: state.cycle(),
        stage: Stage::BeforeToolBatch,
    };
    assert!(
        stage_allocation(&state, cursor, &ReducerStageOutcome::Continue, &sources).is_err(),
        "stage_allocation must reject Continue at BeforeToolBatch, matching stage_id_requirements"
    );
}

/// End-to-end, with the run's deadline actually in the past — the exact
/// precondition `fail_closed_on_run_deadline`'s only caller
/// (`prepare_tool_batch_if_ready`) guarantees before invoking it.
///
/// This is the test that caught a second, deeper issue than the one in
/// the brief: once `now >= effective_deadline`, `decide_limit`
/// (`decide.rs:469-497`) intercepts *before* `decide_stage` — and
/// therefore `stage_id_requirements` — ever runs, and always demands
/// `IdRequirements::new(2, 2, 0, 0, 0, 0)` (`decide.rs:684`), not whatever
/// `stage_allocation` computes for the submitted outcome. Allocating via
/// `stage_allocation` here (as an earlier version of this fix did)
/// compiles and passes every other unit test in this module, but fails an
/// actual deadline-expiry run end to end
/// (`finstack-ai-test/tests/interaction.rs::expire_if_due_on_restore_never_dispatches`,
/// confirmed by reproducing the failure locally before writing this
/// assertion). This test pins both halves of that discovery so a future
/// change cannot silently reintroduce it.
#[test]
fn run_deadline_fail_closed_is_admitted_by_the_kernel_at_before_tool_batch() {
    let deadline = fixed_timestamp(1_500);
    let now = fixed_timestamp(2_000);
    let mut state = state_at_phase(&accepted_state(), RunPhase::BeforeToolBatch);
    state.set_accepted(Some(acceptance(Some(deadline))));
    let sources = test_sources();
    let cursor = StageCursor {
        cycle: state.cycle(),
        stage: Stage::BeforeToolBatch,
    };
    let outcome = run_deadline_outcome().expect("run deadline outcome");

    // The wrong shape: stage_allocation's Fail@BeforeToolBatch tuple
    // (1, 0, 0, 0, 0, 0) is the stage_id_requirements answer, but
    // decide_limit never lets stage_id_requirements run here.
    let wrong_ids = stage_allocation(&state, cursor, &outcome, &sources).expect("stage allocation");
    let rejected = Kernel::try_restore(state.clone())
        .expect("restore state")
        .decide(
            &TransitionEnv {
                now,
                ids: wrong_ids,
            },
            KernelInput::StageSettled(StageSettled {
                cursor,
                outcome: outcome.clone(),
            }),
        );
    assert!(
        rejected.is_err(),
        "stage_allocation's ids must NOT satisfy decide_limit's deadline-crossing \
             requirement once the deadline has passed: {rejected:?}"
    );

    // The real fix's shape: matches decide_limit's own requirement.
    let ids = stage_ids(2, 2, 0, 0, 0, 0, &sources).expect("deadline crossing ids");
    let decision = Kernel::try_restore(state).expect("restore state").decide(
        &TransitionEnv { now, ids },
        KernelInput::StageSettled(StageSettled { cursor, outcome }),
    );
    assert!(
        decision.is_ok(),
        "kernel rejected the fail-closed submission at BeforeToolBatch: {decision:?}"
    );
}

/// The deadline itself: `fail_closed_on_run_deadline` is reached only when
/// `now >= effective_deadline`. This pins that the fixture used above
/// actually represents an expired run, not merely a state the kernel
/// happens to accept.
#[test]
fn accepted_run_with_past_deadline_reports_expired() {
    let deadline = fixed_timestamp(1_500);
    let accepted = acceptance(Some(deadline));
    let now = fixed_timestamp(2_000);
    assert!(
        accepted
            .effective_deadline()
            .is_some_and(|value| now >= value)
    );
}
