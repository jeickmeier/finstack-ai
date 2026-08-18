use super::*;
use finstack_ai_kernel::{
    AllocatedIds, AuthorizationEvidence, ComponentId, ComponentRef, EffectKind, EffectOutputKind,
    EffectTag, EventTag, InteractionCancelled, InteractionExpired, InteractionKind,
    InteractionRequest, InteractionResolution, InteractionSettled, InteractionTag,
    InteractionTerminalOutcome, KernelInput, RecordTag, RequestInteraction, Version,
};

const INTERACTION_ONE: u64 = 501;
const INTERACTION_EFFECT: u64 = 502;
const REQUEST_RECORDS: [u64; 2] = [80, 81];
const REQUEST_EVENTS: [u64; 2] = [80, 81];
const RESOLVE_RECORDS: [u64; 2] = [82, 83];
const RESOLVE_EVENTS: [u64; 2] = [82, 83];

fn approval_schema() -> RawJson {
    RawJson::parse(
        r#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
    )
    .expect("approval schema")
}

fn approval_response(approved: bool) -> RawJson {
    RawJson::parse(if approved {
        r#"{"approved":true}"#
    } else {
        r#"{"approved":false}"#
    })
    .expect("approval response")
}

fn policy_component() -> ComponentRef {
    ComponentRef::new(
        ComponentId::parse("finstack.policy.approval").expect("component"),
        Some(Version {
            major: 1,
            minor: 0,
            patch: 0,
        }),
    )
}

fn approval_request(kind: InteractionKind) -> InteractionRequest {
    InteractionRequest::try_new(
        1,
        id::<InteractionTag>(INTERACTION_ONE),
        id::<EffectTag>(INTERACTION_EFFECT),
        kind,
        vec![ContentBlock::Text(
            TextBlock::try_new("approve the next action").expect("prompt"),
        )],
        approval_schema(),
        policy_component(),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        None,
        false,
        Metadata::empty(),
    )
    .expect("interaction request")
}

fn interaction_env(
    now_ms: i64,
    record_ids: &[u64],
    event_ids: &[u64],
    effect_ids: &[u64],
    interaction_ids: &[u64],
) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now_ms),
        ids: AllocatedIds::try_new(
            record_ids.iter().copied().map(id::<RecordTag>).collect(),
            event_ids.iter().copied().map(id::<EventTag>).collect(),
            effect_ids.iter().copied().map(id::<EffectTag>).collect(),
            interaction_ids
                .iter()
                .copied()
                .map(id::<InteractionTag>)
                .collect(),
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .expect("interaction allocated ids"),
    }
}

fn request_env(now_ms: i64) -> TransitionEnv {
    interaction_env(
        now_ms,
        &REQUEST_RECORDS,
        &REQUEST_EVENTS,
        &[INTERACTION_EFFECT],
        &[INTERACTION_ONE],
    )
}

fn resolve_env(now_ms: i64) -> TransitionEnv {
    interaction_env(now_ms, &RESOLVE_RECORDS, &RESOLVE_EVENTS, &[], &[])
}

fn accepted_resolution(approved: bool) -> InteractionResolution {
    let accepted = root_acceptance();
    InteractionResolution::try_new(
        id::<InteractionTag>(INTERACTION_ONE),
        "resolution-1",
        accepted.security().principal().clone(),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization"),
        approval_response(approved),
        None::<&str>,
    )
    .expect("resolution")
}

pub(super) fn request_and_await(kind: InteractionKind) -> Harness {
    let mut harness = Harness::default();
    accept(&mut harness);
    harness.apply_input(
        request_env(1_500),
        KernelInput::RequestInteraction(RequestInteraction {
            request: approval_request(kind),
        }),
    );
    harness
}

#[test]
fn request_interaction_from_before_run_does_not_consume_the_stage_cursor() {
    let harness = request_and_await(InteractionKind::Approval);
    let state = harness.kernel.state();
    assert_eq!(state.phase, Some(RunPhase::AwaitingInteraction));
    assert_eq!(state.state_version, 6);
    assert!(state.stage_settlements.is_empty());
    let pending = state
        .pending_interaction
        .as_ref()
        .expect("pending interaction");
    assert_eq!(pending.prior_phase, RunPhase::BeforeRun);
    assert_eq!(pending.cursor.stage, Stage::BeforeRun);
    assert_eq!(pending.request.kind(), &InteractionKind::Approval);
    assert_eq!(
        pending.request.interaction_id(),
        id::<InteractionTag>(INTERACTION_ONE)
    );
    assert_eq!(
        pending.request.effect_id(),
        id::<EffectTag>(INTERACTION_EFFECT)
    );
    let bodies: Vec<_> = harness
        .batches
        .last()
        .expect("request batch")
        .records
        .iter()
        .map(|record| match record.body() {
            RecordBody::EffectRequested(requested) => {
                assert_eq!(requested.kind(), EffectKind::Interaction);
                assert_eq!(
                    requested.output_contract().kind,
                    EffectOutputKind::InteractionResolution
                );
                "effect"
            }
            RecordBody::InteractionRequested(_) => "interaction",
            other => panic!("unexpected request body {other:?}"),
        })
        .collect();
    assert_eq!(bodies, ["effect", "interaction"]);
}

#[test]
fn granting_resolution_restores_prior_phase_and_leaves_before_run_unset() {
    let mut harness = request_and_await(InteractionKind::Approval);
    let decision = harness.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(true))),
    );
    assert!(decision.actions.is_empty());
    let state = harness.kernel.state();
    assert_eq!(state.phase, Some(RunPhase::BeforeRun));
    assert!(state.pending_interaction.is_none());
    assert!(state.stage_settlements.is_empty());
    let terminal = state.last_interaction_terminal.as_ref().expect("terminal");
    assert_eq!(terminal.outcome, InteractionTerminalOutcome::Granted);
    assert!(state.resolution_identities.contains_key("resolution-1"));
    settle_before_run(&mut harness);
    assert_eq!(
        harness.kernel.state().phase,
        Some(RunPhase::PreparingContext)
    );
}

#[test]
fn denied_approval_is_a_schema_valid_resolution() {
    let mut harness = request_and_await(InteractionKind::Approval);
    harness.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(false))),
    );
    let terminal = harness
        .kernel
        .state()
        .last_interaction_terminal
        .as_ref()
        .expect("terminal");
    assert_eq!(terminal.outcome, InteractionTerminalOutcome::Denied);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::BeforeRun));
}

#[test]
fn duplicate_equivalent_resolution_is_idempotent() {
    let mut harness = request_and_await(InteractionKind::Approval);
    harness.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(true))),
    );
    let before = harness.kernel.state().clone();
    let decision = harness
        .kernel
        .decide(
            &empty_env(1_700),
            KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(
                true,
            ))),
        )
        .expect("duplicate");
    assert!(decision.records.is_empty());
    assert_eq!(harness.kernel.state(), &before);
}

#[test]
fn conflicting_resolution_fails_closed() {
    let harness = request_and_await(InteractionKind::Approval);
    let first = accepted_resolution(true);
    let conflict = InteractionResolution::try_new(
        first.interaction_id(),
        first.resolution_id(),
        first.principal().clone(),
        first.authorization().clone(),
        approval_response(false),
        None::<&str>,
    )
    .expect("conflict");
    let mut accepted = harness;
    accepted.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(first)),
    );
    let error = accepted
        .kernel
        .decide(
            &empty_env(1_700),
            KernelInput::InteractionSettled(InteractionSettled::Resolved(conflict)),
        )
        .expect_err("conflict");
    assert_eq!(error.code(), "conflicting_settlement");
}

#[test]
fn expiry_restores_prior_phase_without_a_grant() {
    let mut harness = request_and_await(InteractionKind::Approval);
    harness.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
            interaction_id: id::<InteractionTag>(INTERACTION_ONE),
            expired_at: timestamp(1_600),
        })),
    );
    let state = harness.kernel.state();
    assert_eq!(state.phase, Some(RunPhase::BeforeRun));
    assert_eq!(
        state
            .last_interaction_terminal
            .as_ref()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Expired
    );
}

#[test]
fn expiry_is_recorded_when_the_run_deadline_has_also_been_reached() {
    let mut harness = Harness::default();
    harness.apply_input(
        transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
        KernelInput::AcceptRun(AcceptRun {
            session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
            lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
            accepted: root_acceptance_with(RunLimits::empty(), Some(timestamp(1_600))),
        }),
    );
    harness.apply_input(
        request_env(1_500),
        KernelInput::RequestInteraction(RequestInteraction {
            request: approval_request(InteractionKind::Approval),
        }),
    );
    harness.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
            interaction_id: id::<InteractionTag>(INTERACTION_ONE),
            expired_at: timestamp(1_600),
        })),
    );
    let state = harness.kernel.state();
    assert_eq!(state.phase, Some(RunPhase::BeforeRun));
    assert!(state.terminal.is_none());
    assert_eq!(
        state
            .last_interaction_terminal
            .as_ref()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Expired
    );
}

#[test]
fn cancel_requested_while_awaiting_interaction_includes_the_interaction_effect() {
    let mut harness = request_and_await(InteractionKind::Approval);
    let decision = harness.apply_input(
        cancellation_env(1_700, &[90], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    assert_eq!(
        decision.actions,
        vec![PostCommitAction::CancelEffect {
            effect_id: id::<EffectTag>(INTERACTION_EFFECT),
        }]
    );
    let state = harness.kernel.state();
    assert_eq!(state.phase, Some(RunPhase::Cancelling));
    assert!(state.pending_interaction.is_some());
    assert_eq!(
        state
            .cancellation
            .as_ref()
            .expect("cancellation")
            .outstanding_effects
            .as_ref(),
        [id::<EffectTag>(INTERACTION_EFFECT)]
    );
}

#[test]
fn reconcile_cancelled_interaction_emits_the_pair_and_keeps_cancelling_until_run_cancelled() {
    let mut harness = request_and_await(InteractionKind::Approval);
    harness.apply_input(
        cancellation_env(1_700, &[90], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    let decision = harness.apply_input(
        transition_env(1_800, &[91, 92, 93, 94], &[90, 91, 92], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([id::<EffectTag>(INTERACTION_EFFECT)]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(
        decision_body_names(&decision),
        [
            "interaction_cancelled",
            "effect_cancelled",
            "cancellation_reconciled",
            "run_cancelled",
        ]
    );
    let state = harness.kernel.state();
    assert_eq!(state.phase, Some(RunPhase::Cancelled));
    assert!(state.pending_interaction.is_none());
    assert_eq!(
        state
            .last_interaction_terminal
            .as_ref()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Cancelled
    );
    assert!(matches!(state.terminal, Some(TerminalState::Cancelled(_))));
}

#[test]
fn interaction_settled_after_run_cancel_is_rejected() {
    let mut harness = request_and_await(InteractionKind::Approval);
    harness.apply_input(
        cancellation_env(1_700, &[90], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: Some(Arc::from("shutdown")),
        }),
    );
    let error = harness
        .kernel
        .decide(
            &resolve_env(1_800),
            KernelInput::InteractionSettled(InteractionSettled::Cancelled(
                InteractionCancelled::try_new(
                    id::<InteractionTag>(INTERACTION_ONE),
                    None,
                    None,
                    Some("late-cancel"),
                )
                .expect("cancelled"),
            )),
        )
        .expect_err("late settlement");
    assert_eq!(error.code(), "invalid_phase_input");
}

#[test]
fn cancelled_while_waiting_restores_prior_phase() {
    let mut harness = request_and_await(InteractionKind::Approval);
    harness.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Cancelled(
            InteractionCancelled::try_new(
                id::<InteractionTag>(INTERACTION_ONE),
                None,
                None,
                Some("operator-cancel"),
            )
            .expect("cancelled"),
        )),
    );
    assert_eq!(
        harness
            .kernel
            .state()
            .last_interaction_terminal
            .as_ref()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Cancelled
    );
}

#[test]
fn supported_kinds_can_request_and_resolve() {
    for kind in [
        InteractionKind::Approval,
        InteractionKind::Choice,
        InteractionKind::Form,
        InteractionKind::FreeText,
        InteractionKind::Review,
        InteractionKind::Correction,
        InteractionKind::Custom {
            name: std::sync::Arc::from("team.custom"),
        },
    ] {
        let mut harness = request_and_await(kind.clone());
        assert_eq!(
            harness.kernel.state().phase,
            Some(RunPhase::AwaitingInteraction),
            "{kind:?}"
        );
        harness.apply_input(
            resolve_env(1_600),
            KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(
                true,
            ))),
        );
        assert_eq!(
            harness.kernel.state().phase,
            Some(RunPhase::BeforeRun),
            "{kind:?}"
        );
    }
}

#[test]
fn second_outstanding_request_is_rejected() {
    let harness = request_and_await(InteractionKind::Approval);
    let error = harness
        .kernel
        .decide(
            &request_env(1_550),
            KernelInput::RequestInteraction(RequestInteraction {
                request: approval_request(InteractionKind::Choice),
            }),
        )
        .expect_err("second request");
    assert_eq!(error.code(), "invalid_phase_input");
}

#[test]
#[ignore = "prints public-rust-api kernel-input fixture JSON"]
fn print_kernel_input_interaction_fixtures() {
    for (name, input) in [
        (
            "request",
            KernelInput::RequestInteraction(RequestInteraction {
                request: approval_request(InteractionKind::Approval),
            }),
        ),
        (
            "resolved",
            KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(
                true,
            ))),
        ),
        (
            "expired",
            KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
                interaction_id: id::<InteractionTag>(INTERACTION_ONE),
                expired_at: timestamp(1_600),
            })),
        ),
        (
            "cancelled",
            KernelInput::InteractionSettled(InteractionSettled::Cancelled(
                InteractionCancelled::try_new(
                    id::<InteractionTag>(INTERACTION_ONE),
                    None,
                    None,
                    Some("operator-cancel"),
                )
                .expect("cancelled"),
            )),
        ),
    ] {
        println!(
            "{name}={}",
            serde_json::to_string_pretty(&input).expect("json")
        );
    }
}

#[test]
fn kernel_input_interaction_commands_round_trip() {
    let request = KernelInput::RequestInteraction(RequestInteraction {
        request: approval_request(InteractionKind::Approval),
    });
    let encoded = serde_json::to_value(&request).expect("encode request");
    let decoded = serde_json::from_value::<KernelInput>(encoded).expect("decode request");
    assert_eq!(decoded, request);

    let resolved =
        KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(true)));
    let encoded = serde_json::to_value(&resolved).expect("encode resolved");
    let decoded = serde_json::from_value::<KernelInput>(encoded).expect("decode resolved");
    assert_eq!(decoded, resolved);

    let expired =
        KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
            interaction_id: id::<InteractionTag>(INTERACTION_ONE),
            expired_at: timestamp(1_600),
        }));
    let encoded = serde_json::to_value(&expired).expect("encode expired");
    let decoded = serde_json::from_value::<KernelInput>(encoded).expect("decode expired");
    assert_eq!(decoded, expired);

    let cancelled = KernelInput::InteractionSettled(InteractionSettled::Cancelled(
        InteractionCancelled::try_new(
            id::<InteractionTag>(INTERACTION_ONE),
            None,
            None,
            Some("operator-cancel"),
        )
        .expect("cancelled"),
    ));
    let encoded = serde_json::to_value(&cancelled).expect("encode cancelled");
    let decoded = serde_json::from_value::<KernelInput>(encoded).expect("decode cancelled");
    assert_eq!(decoded, cancelled);
}

#[test]
fn conflicting_resolution_is_rejected_without_mutation() {
    let mut harness = request_and_await(InteractionKind::Approval);
    harness.apply_input(
        resolve_env(1_600),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(true))),
    );
    let before = harness.kernel.state().clone();
    assert_error_code(
        harness.kernel.decide(
            &resolve_env(1_601),
            KernelInput::InteractionSettled(InteractionSettled::Resolved(accepted_resolution(
                false,
            ))),
        ),
        "conflicting_settlement",
    );
    assert_eq!(harness.kernel.state(), &before);
    let replayed = replay(&harness.batches);
    assert_eq!(replayed.state(), harness.kernel.state());
    assert_eq!(
        replayed.state().state_hash().expect("replay hash"),
        harness.kernel.state().state_hash().expect("live hash")
    );
}

#[test]
fn non_interaction_histories_keep_their_existing_state_version() {
    let mut harness = Harness::default();
    accept(&mut harness);
    assert_eq!(harness.kernel.state().state_version, 1);
    let before = harness.kernel.state().state_hash().expect("hash");
    settle_before_run(&mut harness);
    assert_eq!(harness.kernel.state().state_version, 1);
    assert_ne!(harness.kernel.state().state_hash().expect("hash"), before);
}
