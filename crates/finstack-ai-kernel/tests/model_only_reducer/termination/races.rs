#[test]
fn terminal_race_permutations_follow_committed_journal_precedence() {
    let completed_first = drive_to_completed();
    let late_cancel = completed_first.kernel.decide(
        &empty_env(2_000),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    assert!(matches!(
        late_cancel,
        Err(KernelError::TerminalStateImmutable)
    ));

    let mut cancellation_first = drive_to_awaiting_model();
    cancellation_first.apply_input(
        cancellation_env(1_350, &[7], &[], &[700]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    cancellation_first.apply_input(
        transition_env(1_400, &[8, 9], &[3], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(700),
            completed_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([]),
        }),
    );
    assert_eq!(
        cancellation_first.kernel.state().phase(),
        Some(RunPhase::Cancelled)
    );
    assert!(
        cancellation_first
            .kernel
            .state()
            .messages()
            .iter()
            .all(|message| message.role() != MessageRole::Assistant)
    );

    let mut uncertain = drive_to_awaiting_model();
    uncertain.apply_input(
        cancellation_env(1_350, &[7], &[], &[701]),
        KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
            initiator: finstack_ai_kernel::CancellationInitiator::RuntimeShutdown,
            reason: None,
        }),
    );
    uncertain.apply_input(
        transition_env(1_400, &[8, 9], &[3], &[], &[], &[], &[]),
        KernelInput::CancellationReconciled(finstack_ai_kernel::CancellationReconciledInput {
            request_id: id::<finstack_ai_kernel::CancellationRequestTag>(701),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([id::<finstack_ai_kernel::EffectTag>(EFFECT_ONE)]),
        }),
    );
    assert_eq!(uncertain.kernel.state().phase(), Some(RunPhase::Suspended));
    assert!(uncertain.kernel.state().terminal().is_none());

    for harness in [&cancellation_first, &uncertain] {
        let replayed = replay(&harness.batches);
        assert_eq!(replayed.state(), harness.kernel.state());
        assert_eq!(
            replayed.state().state_hash().expect("replay hash"),
            harness.kernel.state().state_hash().expect("live hash")
        );
    }
}
