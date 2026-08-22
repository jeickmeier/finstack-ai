use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, FileFailurePersistence, RngSeed};

use super::*;

const PROPERTY_CASES: u32 = 256;
const PROPERTY_SEED: u64 = 0x5eed_0130_0000_0001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReferencePhase {
    Empty,
    BeforeRun,
    PreparingContext,
    BeforeModel,
    AwaitingModel,
    AwaitingExternal,
    AfterModel,
    BeforeFinalize,
    Completed,
}

#[derive(Clone, Copy, Debug)]
enum ReferenceCommand {
    Accept,
    BeforeRun,
    ContextPrepared,
    ModelRequested,
    ModelDeferred,
    ModelCompleted,
    AfterModel,
    Finalize,
}

fn property_config() -> ProptestConfig {
    ProptestConfig {
        cases: PROPERTY_CASES,
        rng_seed: RngSeed::Fixed(PROPERTY_SEED),
        failure_persistence: Some(Box::new(FileFailurePersistence::SourceParallel(
            "proptest-regressions",
        ))),
        max_shrink_iters: 10_000,
        ..ProptestConfig::default()
    }
}

fn reference_transition(phase: ReferencePhase, command: ReferenceCommand) -> ReferencePhase {
    match (phase, command) {
        (ReferencePhase::Empty, ReferenceCommand::Accept) => ReferencePhase::BeforeRun,
        (ReferencePhase::BeforeRun, ReferenceCommand::BeforeRun) => {
            ReferencePhase::PreparingContext
        }
        (ReferencePhase::PreparingContext, ReferenceCommand::ContextPrepared) => {
            ReferencePhase::BeforeModel
        }
        (ReferencePhase::BeforeModel, ReferenceCommand::ModelRequested) => {
            ReferencePhase::AwaitingModel
        }
        (ReferencePhase::AwaitingModel, ReferenceCommand::ModelDeferred) => {
            ReferencePhase::AwaitingExternal
        }
        (
            ReferencePhase::AwaitingModel | ReferencePhase::AwaitingExternal,
            ReferenceCommand::ModelCompleted,
        ) => ReferencePhase::AfterModel,
        (ReferencePhase::AfterModel, ReferenceCommand::AfterModel) => {
            ReferencePhase::BeforeFinalize
        }
        (ReferencePhase::BeforeFinalize, ReferenceCommand::Finalize) => ReferencePhase::Completed,
        (actual, command) => panic!("invalid reference transition {actual:?} / {command:?}"),
    }
}

fn assert_reference_phase(reference: ReferencePhase, harness: &Harness) {
    let actual = match harness.kernel.state().phase() {
        None => ReferencePhase::Empty,
        Some(RunPhase::BeforeRun) => ReferencePhase::BeforeRun,
        Some(RunPhase::PreparingContext) => ReferencePhase::PreparingContext,
        Some(RunPhase::BeforeModel) => ReferencePhase::BeforeModel,
        Some(RunPhase::AwaitingModel) => ReferencePhase::AwaitingModel,
        Some(RunPhase::AwaitingExternal) => ReferencePhase::AwaitingExternal,
        Some(RunPhase::AfterModel) => ReferencePhase::AfterModel,
        Some(RunPhase::BeforeFinalize) => ReferencePhase::BeforeFinalize,
        Some(RunPhase::Completed) => ReferencePhase::Completed,
        other => panic!("unexpected generated model-only phase {other:?}"),
    };
    assert_eq!(actual, reference);
}

fn assert_deterministic_decision(kernel: &Kernel, env: &TransitionEnv, input: &KernelInput) {
    let before = kernel.state().clone();
    let before_hash = before.state_hash().expect("state hash before decision");
    let first = kernel.decide(env, input.clone());
    let second = kernel.decide(env, input.clone());
    assert_eq!(first, second, "equal inputs produced different decisions");
    assert_eq!(kernel.state(), &before, "decide mutated state");
    assert_eq!(
        kernel
            .state()
            .state_hash()
            .expect("state hash after decision"),
        before_hash,
        "decide changed state hash"
    );
}

fn drive_generated_path(deferred: bool, text: &str) {
    let mut harness = Harness::default();
    let mut reference = ReferencePhase::Empty;

    let env = transition_env(1_000, &[1], &[1], &[], &[], &[], &[]);
    let input = accept_input();
    assert_deterministic_decision(&harness.kernel, &env, &input);
    harness.apply_input(env, input);
    reference = reference_transition(reference, ReferenceCommand::Accept);
    assert_reference_phase(reference, &harness);

    settle_before_run(&mut harness);
    reference = reference_transition(reference, ReferenceCommand::BeforeRun);
    assert_reference_phase(reference, &harness);
    prepare_context(&mut harness, 0, false);
    reference = reference_transition(reference, ReferenceCommand::ContextPrepared);
    assert_reference_phase(reference, &harness);
    request_model(&mut harness, 0, false);
    reference = reference_transition(reference, ReferenceCommand::ModelRequested);
    assert_reference_phase(reference, &harness);

    if deferred {
        let input = deferred_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "property-job");
        let env = transition_env(1_400, &[7], &[3], &[], &[], &[], &[]);
        assert_deterministic_decision(&harness.kernel, &env, &input);
        harness.apply_input(env, input);
        reference = reference_transition(reference, ReferenceCommand::ModelDeferred);
        assert_reference_phase(reference, &harness);

        let input = external_completed_input(
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_500,
            "property-external",
            text,
        );
        let env = transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[FINAL_MESSAGE_ONE]);
        assert_deterministic_decision(&harness.kernel, &env, &input);
        harness.apply_input(env, input);
    } else {
        let input = completed_input(
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_400,
            "property-direct",
            text,
        );
        let env = transition_env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[FINAL_MESSAGE_ONE]);
        assert_deterministic_decision(&harness.kernel, &env, &input);
        harness.apply_input(env, input);
    }
    reference = reference_transition(reference, ReferenceCommand::ModelCompleted);
    assert_reference_phase(reference, &harness);

    let record = if deferred { 10 } else { 9 };
    harness.apply_input(
        transition_env(1_600, &[record], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterModel, ReducerStageOutcome::Continue),
    );
    reference = reference_transition(reference, ReferenceCommand::AfterModel);
    assert_reference_phase(reference, &harness);

    let records = if deferred {
        &[11, 12][..]
    } else {
        &[10, 11][..]
    };
    let events = if deferred { &[6][..] } else { &[5][..] };
    harness.apply_input(
        transition_env(1_700, records, events, &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    );
    reference = reference_transition(reference, ReferenceCommand::Finalize);
    assert_reference_phase(reference, &harness);

    let replayed = replay(&harness.batches);
    assert_eq!(harness.kernel.state(), replayed.state());
    assert_eq!(
        harness.kernel.state().state_hash().expect("live hash"),
        replayed.state().state_hash().expect("replay hash")
    );
}

proptest! {
    #![proptest_config(property_config())]

    #[test]
    fn generated_model_paths_match_reference_and_replay(
        scenarios in prop::collection::vec((any::<bool>(), "[a-z0-9]{1,16}"), 1..=8)
    ) {
        for (deferred, text) in scenarios {
            drive_generated_path(deferred, &text);
        }
    }

    #[test]
    fn conflicting_external_completion_is_atomic(
        first in "[a-z]{1,16}",
        second in "[a-z]{1,16}",
    ) {
        prop_assume!(first != second);
        let mut harness = drive_to_awaiting_model();
        harness.apply_input(
            transition_env(1_400, &[7], &[3], &[], &[], &[], &[]),
            deferred_input(TURN_ONE, MODEL_REQUEST_ONE, EFFECT_ONE, "property-conflict"),
        );
        harness.apply_input(
            transition_env(1_500, &[8, 9], &[4, 5], &[], &[], &[], &[FINAL_MESSAGE_ONE]),
            external_completed_input(
                EFFECT_ONE,
                FINAL_MESSAGE_ONE,
                1_500,
                "property-conflict-id",
                &first,
            ),
        );
        let before = harness.kernel.state().clone();
        let before_hash = before.state_hash().expect("before conflict hash");
        assert_error_code(
            harness.kernel.decide(
                &empty_env(1_501),
                external_completed_input(
                    EFFECT_ONE,
                    FINAL_MESSAGE_ONE,
                    1_500,
                    "property-conflict-id",
                    &second,
                ),
            ),
            "conflicting_completion_id",
        );
        prop_assert_eq!(harness.kernel.state(), &before);
        prop_assert_eq!(
            harness.kernel.state().state_hash().expect("after conflict hash"),
            before_hash
        );
    }

    #[test]
    fn interaction_cancellation_pairing_and_json_are_total(
        reason in "[a-z0-9_-]{1,32}",
        include_principal in any::<bool>(),
    ) {
        let principal = PrincipalRef::try_new("issuer", "subject", Some("tenant"))
            .expect("principal");
        let authorization = finstack_ai_kernel::AuthorizationEvidence::try_new(
            "policy-v1",
            "decision-v1",
        )
        .expect("authorization");
        let value = finstack_ai_kernel::InteractionCancelled::try_new(
            id::<finstack_ai_kernel::InteractionTag>(91),
            include_principal.then_some(principal),
            include_principal.then_some(authorization),
            Some(reason.as_str()),
        )
        .expect("paired cancellation");
        assert_json_round_trip_and_unknown_fields(&value);
        prop_assert_eq!(value.principal().is_some(), include_principal);
        prop_assert_eq!(value.authorization().is_some(), include_principal);
    }
}

#[test]
fn interaction_cancellation_rejects_unpaired_authorization() {
    let principal = PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal");
    assert!(
        finstack_ai_kernel::InteractionCancelled::try_new(
            id::<finstack_ai_kernel::InteractionTag>(92),
            Some(principal),
            None,
            None::<&str>,
        )
        .is_err()
    );
}
