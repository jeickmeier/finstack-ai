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
