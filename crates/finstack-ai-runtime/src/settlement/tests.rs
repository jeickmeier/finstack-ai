use std::sync::atomic::{AtomicU64, Ordering};

use finstack_ai_kernel::{
    AllocatedIds, BudgetPropagation, CancellationPropagation, ComponentId, ContentBlock,
    DeadlinePropagation, Digest, EffectCompleted, EffectOutputKind, ErrorCategory, ErrorDescriptor,
    Id, IdTag, Kernel, KernelInput, KernelState, LaneTag, Message, MessageRole, Metadata,
    ModelSettled, ModelSettlement, OutputConfiguration, OutputSpec, PrincipalPropagation,
    PrincipalRef, ProviderIds, RawJson, ReducerStageOutcome, RetrySafety, RunAccepted, RunPhase,
    RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag, SessionTag, Stage, StageCursor,
    StageSettled, TerminalCandidate, TextBlock, Timestamp, ToolBatchContinuation, ToolCallBlock,
    ToolCallId, ToolCallPlan, ToolFailurePolicy, TransitionEnv, Version,
};

use super::stage::{
    TOOL_PLAN_COVERAGE_MISMATCH, assert_plan_coverage, run_deadline_outcome, stage_ids,
};
use super::*;
use crate::coordinator::CommitCoordinator;
use crate::{CancellationSignal, ExternalClock, ResolvedToolCatalog};

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
/// `accepted` via struct-update syntax where the scenario needs it.
fn accepted_state() -> KernelState {
    KernelState {
        session_id: Some(fixed_id::<SessionTag>(1)),
        lane_id: Some(fixed_id::<LaneTag>(2)),
        accepted: Some(acceptance(None)),
        accepted_at: Some(fixed_timestamp(1_000)),
        phase: Some(RunPhase::BeforeRun),
        cycle: 0,
        ..KernelState::default()
    }
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
    let state = KernelState {
        phase: Some(RunPhase::BeforeToolBatch),
        ..accepted_state()
    };
    let sources = test_sources();
    let cursor = StageCursor {
        cycle: state.cycle,
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
    let state = KernelState {
        phase: Some(RunPhase::BeforeToolBatch),
        accepted: Some(acceptance(Some(deadline))),
        ..accepted_state()
    };
    let sources = test_sources();
    let cursor = StageCursor {
        cycle: state.cycle,
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

fn completed_candidate(cycle: u64) -> TerminalCandidate {
    TerminalCandidate::Completed {
        cycle,
        turn_id: fixed_id(20),
        model_request_id: fixed_id(21),
        effect_id: fixed_id(22),
        message_id: fixed_id(23),
        result_digest: Digest::raw_json(b"{}"),
    }
}

fn failed_candidate(cycle: u64) -> TerminalCandidate {
    TerminalCandidate::Failed {
        cycle,
        turn_id: None,
        model_request_id: None,
        effect_id: None,
        error: ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
            .expect("descriptor"),
    }
}

fn model_request_prepared(kind: EffectOutputKind) -> ReducerStageOutcome {
    ReducerStageOutcome::ModelRequestPrepared {
        request: RawJson::parse(b"{}").expect("request"),
        component: None,
        output_contract: finstack_ai_kernel::EffectOutputContract {
            kind,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: RetrySafety::SafeToRetry,
        deadline: None,
    }
}

/// Ids expected out of a successful `stage_allocation` call, in the same
/// `(records, events, effects, turns, model_requests, messages)` order as
/// `IdRequirements::new`.
struct Ids {
    records: usize,
    events: usize,
    effects: usize,
    turns: usize,
    model_requests: usize,
    messages: usize,
}

enum Expect {
    Ok(Ids),
    Err(&'static str),
}

/// One `(state, cursor, outcome)` case and the tuple or error code
/// `decide.rs` demands for it, checked against `stage_allocation`'s own
/// answer.
struct Case {
    label: &'static str,
    state: KernelState,
    cursor: StageCursor,
    outcome: ReducerStageOutcome,
    expect: Expect,
}

/// Table-driven: every `stage_allocation` match arm, both the `Ok` tuple
/// and, where the arm has one, the guard's `Err` code — checked against
/// one shared assertion so the table is the single place a future edit to
/// `stage_allocation` (or a drift against `decide.rs`) has to be updated,
/// rather than seven separate ad hoc tests.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one flat case table keeps every stage_allocation arm's tuple/error next to its decide.rs citation"
)]
fn stage_allocation_matches_decide_rs_arm_for_arm() {
    let base = accepted_state();
    let sources = test_sources();

    let cases = vec![
        // decide.rs:1107-1130 — Continue is admitted at BeforeRun.
        Case {
            label: "Continue @ BeforeRun",
            state: KernelState {
                phase: Some(RunPhase::BeforeRun),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Continue,
            expect: Expect::Ok(Ids {
                records: 1,
                events: 0,
                effects: 0,
                turns: 0,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide.rs:1113-1128 — the AfterModel + JsonSchema + no-final-
        // result-yet guard rejects Continue even though AfterModel is
        // otherwise an admitted stage for it.
        Case {
            label: "Continue @ AfterModel (pending JsonSchema output)",
            state: KernelState {
                phase: Some(RunPhase::AfterModel),
                output_configuration: Some(OutputConfiguration {
                    output: OutputSpec::JsonSchema {
                        schema: finstack_ai_kernel::SchemaRef {
                            draft: finstack_ai_kernel::JsonSchemaDraft::Draft202012,
                            schema_version: 1,
                            schema_digest: Digest::raw_json(b"{}"),
                        },
                    },
                    ..OutputConfiguration::default()
                }),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::AfterModel,
            },
            outcome: ReducerStageOutcome::Continue,
            expect: Expect::Err("stage_allocation_output_contract_pending"),
        },
        // decide.rs:1131-1133.
        Case {
            label: "ContextPrepared @ PrepareContext",
            state: KernelState {
                phase: Some(RunPhase::PreparingContext),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([]),
            },
            expect: Expect::Ok(Ids {
                records: 2,
                events: 0,
                effects: 0,
                turns: 1,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide.rs:1134-1141.
        Case {
            label: "ModelRequestPrepared @ BeforeModel (ModelResponse contract)",
            state: KernelState {
                phase: Some(RunPhase::BeforeModel),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeModel,
            },
            outcome: model_request_prepared(EffectOutputKind::ModelResponse),
            expect: Expect::Ok(Ids {
                records: 2,
                events: 1,
                effects: 1,
                turns: 0,
                model_requests: 1,
                messages: 0,
            }),
        },
        // decide.rs:1137-1139 — the contract-kind guard.
        Case {
            label: "ModelRequestPrepared @ BeforeModel (ToolResult contract)",
            state: KernelState {
                phase: Some(RunPhase::BeforeModel),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeModel,
            },
            outcome: model_request_prepared(EffectOutputKind::ToolResult),
            expect: Expect::Err("stage_allocation_model_request_contract_mismatch"),
        },
        // decide.rs:1142-1145.
        Case {
            label: "FinalizeAccepted @ BeforeFinalize (candidate present)",
            state: KernelState {
                phase: Some(RunPhase::BeforeFinalize),
                terminal_candidate: Some(completed_candidate(0)),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::FinalizeAccepted,
            expect: Expect::Ok(Ids {
                records: 2,
                events: 1,
                effects: 0,
                turns: 0,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide.rs:1142-1145 — terminal_body_from_candidate's own
        // precondition (private to the kernel crate; mirrored here via the
        // public `terminal_candidate` field) that a candidate must exist.
        Case {
            label: "FinalizeAccepted @ BeforeFinalize (no candidate)",
            state: KernelState {
                phase: Some(RunPhase::BeforeFinalize),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::FinalizeAccepted,
            expect: Expect::Err("stage_allocation_terminal_candidate_missing"),
        },
        // decide.rs:1146-1157.
        Case {
            label: "ContinueModel @ BeforeFinalize (Completed candidate)",
            state: KernelState {
                phase: Some(RunPhase::BeforeFinalize),
                terminal_candidate: Some(completed_candidate(0)),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::ContinueModel { reason: None },
            expect: Expect::Ok(Ids {
                records: 1,
                events: 0,
                effects: 0,
                turns: 0,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide.rs:1146-1151 — the guard requires a *Completed* candidate;
        // a Failed one is not admitted for ContinueModel and falls through
        // to the catch-all.
        Case {
            label: "ContinueModel @ BeforeFinalize (Failed candidate)",
            state: KernelState {
                phase: Some(RunPhase::BeforeFinalize),
                terminal_candidate: Some(failed_candidate(0)),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::ContinueModel { reason: None },
            expect: Expect::Err("stage_allocation_outcome_not_admitted"),
        },
        // decide.rs:1153-1156 — the checked cycle-overflow guard.
        Case {
            label: "ContinueModel @ BeforeFinalize (cycle overflow)",
            state: KernelState {
                phase: Some(RunPhase::BeforeFinalize),
                cycle: u64::MAX,
                terminal_candidate: Some(completed_candidate(u64::MAX)),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: u64::MAX,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::ContinueModel { reason: None },
            expect: Expect::Err("stage_allocation_cycle_overflow"),
        },
        // decide.rs:1159-1161.
        Case {
            label: "Fail @ BeforeFinalize",
            state: KernelState {
                phase: Some(RunPhase::BeforeFinalize),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::Fail(
                ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
                    .expect("descriptor"),
            ),
            expect: Expect::Ok(Ids {
                records: 2,
                events: 1,
                effects: 0,
                turns: 0,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide.rs:1162-1164.
        Case {
            label: "Retry @ BeforeFinalize",
            state: KernelState {
                phase: Some(RunPhase::BeforeFinalize),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeFinalize,
            },
            outcome: ReducerStageOutcome::Retry(
                finstack_ai_kernel::RetryDirective::try_new(
                    finstack_ai_kernel::RetryClassification::Validation,
                    finstack_ai_kernel::Duration::ZERO,
                    "probe-policy",
                )
                .expect("directive"),
            ),
            expect: Expect::Ok(Ids {
                records: 3,
                events: 1,
                effects: 1,
                turns: 0,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide.rs:1165-1177.
        Case {
            label: "Fail @ BeforeRun",
            state: KernelState {
                phase: Some(RunPhase::BeforeRun),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Fail(
                ErrorDescriptor::new("probe_failed", "probe", ErrorCategory::Validation, false)
                    .expect("descriptor"),
            ),
            expect: Expect::Ok(Ids {
                records: 1,
                events: 0,
                effects: 0,
                turns: 0,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide_stage:1078-1080 — ToolBatchPrepared bypasses the table
        // entirely and is delegated whole to `allocate_tool_opening`.
        // `tool_opening_counts` on an empty call list yields
        // records = 2 + 0 (requests) + 0 (messages) + 1 (no executable
        // group) = 3, events = 0, effects = 0, messages = 0.
        Case {
            label: "ToolBatchPrepared @ BeforeToolBatch (delegates to allocate_tool_opening)",
            state: KernelState {
                phase: Some(RunPhase::BeforeToolBatch),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeToolBatch,
            },
            outcome: ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([]),
                continuation: ToolBatchContinuation::ContinueModel,
            },
            expect: Expect::Ok(Ids {
                records: 3,
                events: 0,
                effects: 0,
                turns: 0,
                model_requests: 0,
                messages: 0,
            }),
        },
        // decide.rs:1178-1181 — the catch-all: an outcome that is never
        // admitted at the given stage under any arm.
        Case {
            label: "ContextPrepared @ BeforeRun (wrong stage, catch-all)",
            state: KernelState {
                phase: Some(RunPhase::BeforeRun),
                ..base.clone()
            },
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([]),
            },
            expect: Expect::Err("stage_allocation_outcome_not_admitted"),
        },
    ];

    for case in cases {
        let result = stage_allocation(&case.state, case.cursor, &case.outcome, &sources);
        match case.expect {
            Expect::Ok(ids) => {
                let allocated = result
                    .unwrap_or_else(|error| panic!("{}: expected Ok, got {error:?}", case.label));
                assert_eq!(
                    allocated.record_ids().len(),
                    ids.records,
                    "{}: record_ids",
                    case.label
                );
                assert_eq!(
                    allocated.event_ids().len(),
                    ids.events,
                    "{}: event_ids",
                    case.label
                );
                assert_eq!(
                    allocated.effect_ids().len(),
                    ids.effects,
                    "{}: effect_ids",
                    case.label
                );
                assert_eq!(
                    allocated.turn_ids().len(),
                    ids.turns,
                    "{}: turn_ids",
                    case.label
                );
                assert_eq!(
                    allocated.model_request_ids().len(),
                    ids.model_requests,
                    "{}: model_request_ids",
                    case.label
                );
                assert_eq!(
                    allocated.message_ids().len(),
                    ids.messages,
                    "{}: message_ids",
                    case.label
                );
                assert_eq!(
                    allocated.append_batch_ids().len(),
                    1,
                    "{}: append_batch_ids",
                    case.label
                );
            }
            Expect::Err(code) => match result {
                Err(RunHandleError::ToolSettlement { code: actual }) => {
                    assert_eq!(actual, code, "{}: error code", case.label);
                }
                other => panic!(
                    "{}: expected ToolSettlement {{ code: {code:?} }}, got {other:?}",
                    case.label
                ),
            },
        }
    }
}

// ---- BeforeToolBatch middleware --------------------------------------
//
// The one facade stage that never settles through `submit_command`:
// `prepare_tool_batch_if_ready` settles it, so its chain runs here and
// lands through `decide_plan`'s `middleware` parameter rather than as an
// aggregate `ReducerStageOutcome`.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::task::{Context as TaskContext, Poll, Waker};

use finstack_ai_kernel::{
    AcceptRun, AppendRequest, AssignedToolCall, CommittedBatch, ComponentInvocation,
    EffectOutputContract, InvocationRecovery, RecordBody, RecordDraft, RecordEnvelope,
    ToolBatchOpened, ToolExecutionMode, ToolId,
};

use crate::middleware::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRegistration, MiddlewareRole, OrderTier, ResolvedMiddlewareChain, StageInput,
    StageMask, StageOutcome,
};
use crate::middleware_driver::StageDriver;
use crate::{
    ApprovalMetadata, ApprovalRequirement, JournalStore, JsonSchemaToolValidatorCompiler,
    LoadRequest, LoadedSession, PortFuture, SideEffectClass, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth, ToolCallContext, ToolEventStream, ToolExecutionPolicy,
    ToolPolicyDecision, ToolSpec, ToolsetRegistration,
};

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = TaskContext::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "mirrors AllocatedIds' own bags")]
fn env(
    now: i64,
    records: &[u64],
    events: &[u64],
    effects: &[u64],
    turns: &[u64],
    model_requests: &[u64],
    messages: &[u64],
    append_batch: u64,
) -> TransitionEnv {
    TransitionEnv {
        now: fixed_timestamp(now),
        ids: AllocatedIds::try_new(
            records.iter().copied().map(fixed_id).collect(),
            events.iter().copied().map(fixed_id).collect(),
            effects.iter().copied().map(fixed_id).collect(),
            Vec::new(),
            messages.iter().copied().map(fixed_id).collect(),
            turns.iter().copied().map(fixed_id).collect(),
            model_requests.iter().copied().map(fixed_id).collect(),
            Vec::new(),
            Vec::new(),
            vec![fixed_id(append_batch)],
            Vec::new(),
        )
        .expect("allocated ids"),
    }
}

fn model_settled_env(now: i64, message: u64, tool_calls: &[u64]) -> TransitionEnv {
    TransitionEnv {
        now: fixed_timestamp(now),
        ids: AllocatedIds::try_new(
            vec![fixed_id(607), fixed_id(608)],
            vec![fixed_id(603), fixed_id(604)],
            Vec::new(),
            Vec::new(),
            vec![fixed_id(message)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tool_calls.iter().copied().map(fixed_id).collect(),
            vec![fixed_id(605)],
            Vec::new(),
        )
        .expect("model settled ids"),
    }
}

// -- in-memory journal --------------------------------------------------

struct MemoryStore {
    inner: Mutex<MemoryInner>,
}

#[derive(Default)]
struct MemoryInner {
    batches: Vec<CommittedBatch>,
    drafts: Vec<RecordDraft>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MemoryInner::default()),
        }
    }

    /// The single durable `ToolBatchOpened`, i.e. the kernel's own record
    /// of the complete source-ordered assigned plan. Read from the journal
    /// rather than from `state.active_tool_batch` because a batch of
    /// nothing but synthetic closures opens and closes in one transition,
    /// leaving no active batch behind to inspect.
    fn opened_tool_batch(&self) -> Option<ToolBatchOpened> {
        self.inner
            .lock()
            .expect("lock")
            .drafts
            .iter()
            .find_map(|draft| match draft.body() {
                RecordBody::ToolBatchOpened(opened) => Some(opened.clone()),
                _ => None,
            })
    }
}

fn commit_request(request: &AppendRequest) -> CommittedBatch {
    let records = request
        .records()
        .iter()
        .enumerate()
        .map(|(offset, draft)| {
            let sequence = request.expected_sequence() + u64::try_from(offset).expect("offset");
            RecordEnvelope::try_new(
                draft.format_version(),
                draft.kind_version(),
                draft.record_id(),
                draft.session_id(),
                draft.lane_id(),
                draft.run_id(),
                sequence,
                draft.timestamp(),
                None,
                Digest::raw_json(format!("payload-{sequence}").as_bytes()),
                None,
                Digest::raw_json(format!("checksum-{sequence}").as_bytes()),
                draft.derived_event_ids().to_vec(),
                draft.body().clone(),
            )
            .expect("envelope")
        })
        .collect::<Vec<_>>();
    CommittedBatch::try_new(
        request.batch_id(),
        request.expected_sequence(),
        request.expected_sequence() + u64::try_from(records.len()).expect("count") - 1,
        records,
    )
    .expect("committed batch")
}

impl JournalStore for MemoryStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let committed = commit_request(&request);
        let mut inner = self.inner.lock().expect("lock");
        inner.drafts.extend(request.records().iter().cloned());
        inner.batches.push(committed.clone());
        Box::pin(async move { Ok(committed) })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let inner = self.inner.lock().expect("lock");
        let batches = inner.batches.clone();
        let head_sequence = batches.last().map_or(0, |batch| batch.last_sequence);
        Box::pin(async move {
            Ok(LoadedSession {
                session_id: request.session_id,
                head_sequence,
                head_checksum: batches
                    .last()
                    .and_then(|batch| batch.records.last().map(RecordEnvelope::checksum)),
                metadata: Metadata::empty(),
                committed_batches: batches.into(),
                snapshot: None,
                accelerated: None,
            })
        })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "not_used",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("test"),
            })
        })
    }
}

/// These tests stop at batch opening, so nothing is ever executed — but a
/// coordinator with no dispatcher at all faults the moment the kernel
/// commits an effect, which would mask the behaviour under test.
struct NoopDispatcher;

impl crate::coordinator::PostCommitDispatcher for NoopDispatcher {
    fn dispatch(
        &self,
        _dispatch: crate::coordinator::RuntimeDispatch,
    ) -> PortFuture<Result<(), crate::coordinator::DispatchError>> {
        Box::pin(async { Ok(()) })
    }
}

// -- tool catalog -------------------------------------------------------

const TOOL_NAMES: [&str; 3] = ["alpha", "beta", "gamma"];

fn tool_id(name: &str) -> ToolId {
    ToolId::parse(format!("finstack.tools.{name}")).expect("tool id")
}

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        id: tool_id(name),
        model_name: Arc::from(name),
        title: Arc::from(name),
        description: Arc::from("fixture tool"),
        input_schema: RawJson::parse(br#"{"type":"object"}"#).expect("input schema"),
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
    }
}

/// A registered but never-dispatched toolset: these tests stop at batch
/// opening, which is where the `BeforeToolBatch` decision lands.
struct FixtureToolset {
    specs: Arc<[ToolSpec]>,
}

impl crate::Toolset for FixtureToolset {
    fn descriptor(&self) -> crate::ToolsetDescriptor {
        crate::ToolsetDescriptor {
            name: Arc::from("fixture.toolset"),
            metadata: Metadata::empty(),
        }
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.specs)
    }

    fn call(
        &self,
        _ctx: ToolCallContext,
        _call: finstack_ai_kernel::ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        Box::pin(async {
            Err(ToolError::stable(
                "fixture_tool_never_dispatched",
                "the fixture stops at batch opening",
            ))
        })
    }
}

fn catalog() -> ResolvedToolCatalog {
    let specs: Arc<[ToolSpec]> = TOOL_NAMES.iter().copied().map(tool_spec).collect();
    let policies = specs
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                ToolExecutionPolicy {
                    failure_policy: ToolFailurePolicy::ReturnToModel,
                    approval: ToolPolicyDecision::Allow,
                    max_concurrency: 1,
                },
            )
        })
        .collect();
    ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset: Arc::new(FixtureToolset { specs }),
            policies,
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &JsonSchemaToolValidatorCompiler,
    )
    .expect("catalog")
}

// -- middleware ---------------------------------------------------------

fn descriptor(component: &str, stage: Stage) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: ComponentInvocation {
            component: ComponentId::parse(component).expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery: InvocationRecovery::RecomputeSafe,
        },
        stages: StageMask::from_stages([stage]),
        order: MiddlewareOrder {
            tier: OrderTier::Standard,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        role: MiddlewareRole::Standard,
        metadata: Metadata::empty(),
    }
}

/// A component that returns one fixed outcome and counts its invocations,
/// so a test can tell "ran and contributed nothing" from "never ran".
struct Fixed {
    descriptor: MiddlewareDescriptor,
    outcome: StageOutcome,
    calls: Arc<AtomicUsize>,
}

impl Middleware for Fixed {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let outcome = self.outcome.clone();
        Box::pin(async move { Ok(outcome) })
    }
}

fn driver_for(stage: Stage, outcome: StageOutcome, calls: &Arc<AtomicUsize>) -> StageDriver {
    let middleware: Arc<dyn Middleware> = Arc::new(Fixed {
        descriptor: descriptor("fixture.tool-policy", stage),
        outcome,
        calls: Arc::clone(calls),
    });
    StageDriver::new(
        Arc::new(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                .expect("chain"),
        ),
        CancellationSignal::new(),
    )
}

fn retain(names: &[&str]) -> StageOutcome {
    StageOutcome::FilterTools(names.iter().copied().map(tool_id).collect())
}

// -- driving a coordinator to BeforeToolBatch ---------------------------

fn tool_call(ordinal: u64, name: &str) -> ToolCallBlock {
    ToolCallBlock::try_new(
        fixed_id(ordinal),
        name,
        RawJson::parse(b"{}").expect("arguments"),
    )
    .expect("tool call")
}

fn model_output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

/// Drive a fresh coordinator all the way to `RunPhase::BeforeToolBatch`
/// with an assistant message carrying one tool call per name in `names`.
fn coordinator_at_before_tool_batch(
    store: &Arc<MemoryStore>,
    names: &[&str],
    deadline: Option<Timestamp>,
) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(Arc::clone(store) as Arc<dyn JournalStore>);
    coordinator.install_dispatcher(Arc::new(NoopDispatcher));
    block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        KernelInput::AcceptRun(AcceptRun {
            session_id: fixed_id::<SessionTag>(1),
            lane_id: fixed_id::<LaneTag>(2),
            accepted: acceptance(deadline),
        }),
    ))
    .expect("accept");
    block_on(coordinator.submit(
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    ))
    .expect("before run");
    let user = Message::try_new(
        fixed_id(4),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("call the tools").expect("text"),
        )],
        fixed_timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("user message");
    block_on(coordinator.submit(
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([user]),
            },
        }),
    ))
    .expect("context");
    block_on(coordinator.submit(
        env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::BeforeModel,
            },
            outcome: ReducerStageOutcome::ModelRequestPrepared {
                request: RawJson::parse(br#"{"messages":[]}"#).expect("request"),
                component: None,
                output_contract: model_output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
        }),
    ))
    .expect("model request");
    settle_model_with_tool_calls(&mut coordinator, names);
    block_on(coordinator.submit(
        env(1_500, &[609], &[], &[], &[], &[], &[], 606),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::AfterModel,
            },
            outcome: ReducerStageOutcome::Continue,
        }),
    ))
    .expect("after model");
    assert_eq!(
        coordinator.state().phase,
        Some(RunPhase::BeforeToolBatch),
        "the fixture must park the run exactly at the BeforeToolBatch cursor"
    );
    coordinator
}

/// Settle the outstanding model effect with an assistant message carrying
/// one tool call per name, ordinals 301, 302, ... in source order.
fn settle_model_with_tool_calls(coordinator: &mut CommitCoordinator, names: &[&str]) {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending model effect")
        .clone();
    let call_ordinals = (0..names.len())
        .map(|index| 301 + u64::try_from(index).expect("index"))
        .collect::<Vec<_>>();
    let mut content = vec![ContentBlock::Text(
        TextBlock::try_new("calling").expect("text"),
    )];
    content.extend(
        names
            .iter()
            .zip(&call_ordinals)
            .map(|(name, ordinal)| ContentBlock::ToolCall(tool_call(*ordinal, name))),
    );
    let assistant = Message::try_new(
        fixed_id(617),
        MessageRole::Assistant,
        content,
        fixed_timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant message");
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        model_output_contract(),
        RawJson::parse(br#"{"text":"calling"}"#).expect("output"),
        None,
        Vec::new(),
        ProviderIds::empty(),
        Some("cmpl-tool"),
        None,
    )
    .expect("completion");
    block_on(coordinator.submit(
        model_settled_env(1_400, 617, &call_ordinals),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            outcome: ModelSettlement::Completed {
                completion,
                assistant_message: assistant,
            },
        }),
    ))
    .expect("model settled");
}

/// Drive to `BeforeToolBatch` and run `prepare_tool_batch_if_ready` with
/// `driver`, returning the durable source-ordered plan the kernel opened.
fn prepare_with(
    names: &[&str],
    driver: Option<&StageDriver>,
) -> (Vec<AssignedToolCall>, CommitCoordinator, Arc<MemoryStore>) {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, names, None);
    let sources = sources_at(1_600);
    block_on(prepare_tool_batch_if_ready(
        &mut coordinator,
        &catalog(),
        &sources,
        driver,
    ))
    .expect("tool batch preparation");
    let plans = store
        .opened_tool_batch()
        .expect("a tool batch must have been opened")
        .calls
        .to_vec();
    (plans, coordinator, store)
}

fn planned_call_ids(plans: &[AssignedToolCall]) -> Vec<ToolCallId> {
    plans
        .iter()
        .map(|assigned| *assigned.plan.call().tool_call_id())
        .collect()
}

fn source_call_ids(count: usize) -> Vec<ToolCallId> {
    (0..count)
        .map(|index| fixed_id(301 + u64::try_from(index).expect("index")))
        .collect()
}

// -- passthrough --------------------------------------------------------

#[test]
fn no_driver_plans_every_source_call_for_execution() {
    let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, None);
    assert_eq!(plans.len(), 3);
    assert!(
        plans
            .iter()
            .all(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_))),
        "with no chain every registered, schema-valid call must execute: {plans:?}"
    );
}

#[test]
fn a_chain_with_no_before_tool_batch_component_is_passthrough() {
    // The facade installs its chain unconditionally, so the driver is
    // almost always `Some`. Passthrough must key off `is_active`, and the
    // resulting plan must be byte-identical to the no-driver one.
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::AfterModel, retain(&[]), &calls);
    let (with_driver, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));
    let (without_driver, _c, _s) = prepare_with(&TOOL_NAMES, None);

    assert_eq!(
        with_driver, without_driver,
        "an inactive stage must not perturb the opened plan"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "no BeforeToolBatch component is registered, so none may run"
    );
}

#[test]
fn an_identity_fold_leaves_the_plan_unchanged() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, StageOutcome::Continue, &calls);
    let (with_driver, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));
    let (without_driver, _c, _s) = prepare_with(&TOOL_NAMES, None);

    assert_eq!(calls.load(Ordering::Relaxed), 1, "the component must run");
    assert_eq!(
        with_driver, without_driver,
        "an all-Continue chain must not perturb the opened plan"
    );
}

// -- filtering ----------------------------------------------------------

#[test]
fn filtered_tool_call_becomes_a_synthetic_closure() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
    let (plans, _coordinator, _store) = prepare_with(&["alpha"], Some(&driver));

    assert_eq!(plans.len(), 1, "every source call must appear exactly once");
    assert!(
        matches!(plans[0].plan, ToolCallPlan::SyntheticClosure(_)),
        "a denied call must be a synthetic closure, got {:?}",
        plans[0].plan
    );
    assert_eq!(planned_call_ids(&plans), source_call_ids(1));
}

#[test]
fn fully_filtered_batch_still_commits_every_source_call() {
    // `prepare_tool_batch_if_ready` guards on the source `calls` being
    // empty, never on the plans, so denying every call still commits a
    // batch of all-SyntheticClosure plans rather than short-circuiting.
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
    let (plans, coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

    assert_eq!(plans.len(), 3);
    assert!(
        plans
            .iter()
            .all(|assigned| matches!(assigned.plan, ToolCallPlan::SyntheticClosure(_))),
        "every denied call must still be planned: {plans:?}"
    );
    assert_eq!(
        planned_call_ids(&plans),
        source_call_ids(3),
        "coverage is positional: same calls, same order, no duplicates"
    );
    assert!(
        coordinator.state().terminal.is_none(),
        "a fully filtered batch is not a run failure: {:?}",
        coordinator.state().terminal
    );
}

#[test]
fn partial_filter_denies_only_the_calls_outside_the_retained_set() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&["beta"]), &calls);
    let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

    let shapes = plans
        .iter()
        .map(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        shapes,
        vec![false, true, false],
        "only the retained tool may execute, and source order is preserved: {plans:?}"
    );
    assert_eq!(planned_call_ids(&plans), source_call_ids(3));
}

#[test]
fn source_order_survives_a_filter_that_denies_the_first_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&["gamma"]), &calls);
    let (plans, _coordinator, _store) = prepare_with(&TOOL_NAMES, Some(&driver));

    let shapes = plans
        .iter()
        .map(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        shapes,
        vec![false, false, true],
        "denying the leading calls must not shift the surviving one forward: {plans:?}"
    );
    assert_eq!(
        plans
            .iter()
            .map(|assigned| assigned.source_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(planned_call_ids(&plans), source_call_ids(3));
}

// -- the coverage invariant itself --------------------------------------

#[test]
fn plan_coverage_rejects_a_short_reordered_or_duplicated_plan_array() {
    let alpha = tool_call(301, "alpha");
    let beta = tool_call(302, "beta");
    let plan = |call: &ToolCallBlock| {
        ToolCallPlan::SyntheticClosure(finstack_ai_kernel::SyntheticToolClosure {
            call: call.clone(),
            execution: ToolExecutionMode::Sequential,
            failure_policy: ToolFailurePolicy::ReturnToModel,
            error: ErrorDescriptor::new(
                "tool_policy_denied",
                "denied",
                ErrorCategory::Validation,
                false,
            )
            .expect("descriptor"),
        })
    };
    let source = vec![*alpha.tool_call_id(), *beta.tool_call_id()];

    assert_plan_coverage(&[plan(&alpha), plan(&beta)], &source).expect("exact cover");
    for (label, plans) in [
        ("short", vec![plan(&alpha)]),
        ("reordered", vec![plan(&beta), plan(&alpha)]),
        ("duplicated", vec![plan(&alpha), plan(&alpha)]),
        ("long", vec![plan(&alpha), plan(&beta), plan(&beta)]),
    ] {
        let Err(error) = assert_plan_coverage(&plans, &source) else {
            panic!("a {label} plan array must be rejected");
        };
        assert!(
            matches!(&error, RunHandleError::ToolSettlement { code }
                    if *code == TOOL_PLAN_COVERAGE_MISMATCH),
            "{label}: expected the reserved coverage code, got {error:?}"
        );
    }
}

// -- terminal folds and the deadline bypass -----------------------------

#[test]
fn a_middleware_failure_settles_the_stage_as_failed_instead_of_opening_a_batch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(
        Stage::BeforeToolBatch,
        StageOutcome::Fail(Box::new(
            ErrorDescriptor::new(
                "tool_batch_rejected",
                "fixture",
                ErrorCategory::Middleware,
                false,
            )
            .expect("descriptor"),
        )),
        &calls,
    );
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &TOOL_NAMES, None);
    let sources = sources_at(1_600);

    let opened = block_on(prepare_tool_batch_if_ready(
        &mut coordinator,
        &catalog(),
        &sources,
        Some(&driver),
    ))
    .expect("the failed stage still settles");

    assert!(!opened, "no batch may open when the chain fails the stage");
    assert!(
        store.opened_tool_batch().is_none(),
        "no ToolBatchOpened record may exist"
    );
    // `Fail` at a non-`BeforeFinalize` stage (`decide.rs:1165-1177`)
    // normalizes the stage as failed and drives the run to
    // `BeforeFinalize` carrying the component's own descriptor; the
    // facade's finalize settlement is what commits the terminal.
    assert_eq!(coordinator.state().phase, Some(RunPhase::BeforeFinalize));
    let Some(TerminalCandidate::Failed { error, .. }) =
        coordinator.state().terminal_candidate.as_ref()
    else {
        panic!(
            "the middleware Fail must become the terminal candidate: {:?}",
            coordinator.state().terminal_candidate
        );
    };
    assert_eq!(error.code.as_str(), "tool_batch_rejected");
}

#[test]
fn the_run_deadline_path_bypasses_the_chain_entirely() {
    // Fail closed: a run already out of budget must not spend more of it
    // on middleware.
    let calls = Arc::new(AtomicUsize::new(0));
    let driver = driver_for(Stage::BeforeToolBatch, retain(&[]), &calls);
    let store = Arc::new(MemoryStore::new());
    let mut coordinator =
        coordinator_at_before_tool_batch(&store, &TOOL_NAMES, Some(fixed_timestamp(1_550)));
    let sources = sources_at(1_600);

    let opened = block_on(prepare_tool_batch_if_ready(
        &mut coordinator,
        &catalog(),
        &sources,
        Some(&driver),
    ))
    .expect("the deadline path settles");

    assert!(!opened);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "the deadline path must not invoke any middleware component"
    );
    assert!(store.opened_tool_batch().is_none());
}
