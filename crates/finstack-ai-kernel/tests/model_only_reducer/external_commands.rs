use super::*;
use finstack_ai_kernel::{
    AuthorizationEvidence, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    OperationLocator, RecordExternalCommandRejected,
};

fn rejection_input(principal: PrincipalRef, authorization: AuthorizationEvidence) -> KernelInput {
    KernelInput::RecordExternalCommandRejected(RecordExternalCommandRejected {
        locator: OperationLocator::try_new(
            "tenant-a",
            id::<finstack_ai_kernel::SessionTag>(SESSION),
            id::<finstack_ai_kernel::LaneTag>(LANE),
            id::<finstack_ai_kernel::RunTag>(RUN),
        )
        .expect("locator"),
        rejection: ExternalCommandRejected::try_new(
            ExternalCommandKind::EffectCompletion,
            "late-completion-1",
            ExternalCommandTarget::Effect(id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)),
            principal,
            authorization,
            "terminal_run",
            Digest::effect_output(br#"{"result":"late"}"#),
            Some(Digest::effect_output(br#"{"result":"accepted"}"#)),
        )
        .expect("rejection"),
    })
}

#[test]
fn durable_external_rejection_advances_only_the_journal_sequence() {
    let mut harness = drive_to_completed();
    let before = harness.kernel.state().clone();
    let events_before = harness.events.len();
    let input = rejection_input(
        before
            .accepted()
            .expect("accepted")
            .security()
            .principal()
            .clone(),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization"),
    );

    let decision =
        harness.apply_input(transition_env(1_800, &[12], &[], &[], &[], &[], &[]), input);

    assert!(decision.actions.is_empty());
    assert!(decision.diagnostics.is_empty());
    assert!(matches!(
        decision.records.as_slice(),
        [record] if matches!(record.body(), RecordBody::ExternalCommandRejected(_))
            && record.derived_event_ids().is_empty()
    ));
    assert_eq!(harness.events.len(), events_before);
    let mut expected = before;
    expected.set_last_applied_sequence(expected.last_applied_sequence() + 1);
    assert_eq!(harness.kernel.state(), &expected);
    assert_eq!(
        harness.kernel.state().phase(),
        Some(RunPhase::Completed),
        "audit evidence must not reopen a terminal run"
    );
}

#[test]
fn external_rejection_requires_the_accepted_principal_and_authorization() {
    let harness = drive_to_completed();
    let wrong_principal =
        PrincipalRef::try_new("issuer-a", "other", Some("tenant-a")).expect("principal");
    let accepted_authorization =
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
    assert_eq!(
        harness
            .kernel
            .decide(
                &transition_env(1_800, &[12], &[], &[], &[], &[], &[]),
                rejection_input(wrong_principal, accepted_authorization),
            )
            .expect_err("principal mismatch")
            .code(),
        "invalid_run_acceptance"
    );

    let accepted_principal = harness
        .kernel
        .state()
        .accepted()
        .expect("accepted")
        .security()
        .principal()
        .clone();
    let wrong_authorization =
        AuthorizationEvidence::try_new("policy-v1", "different-decision").expect("authorization");
    assert_eq!(
        harness
            .kernel
            .decide(
                &transition_env(1_800, &[12], &[], &[], &[], &[], &[]),
                rejection_input(accepted_principal, wrong_authorization),
            )
            .expect_err("authorization mismatch")
            .code(),
        "invalid_run_acceptance"
    );
}
