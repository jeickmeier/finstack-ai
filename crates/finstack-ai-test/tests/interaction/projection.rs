#[test]
fn unpaired_projection_is_uncertain() {
    let state = finstack_ai_kernel::KernelState {
        phase: Some(RunPhase::AwaitingInteraction),
        ..finstack_ai_kernel::KernelState::default()
    };
    assert_eq!(
        interaction_resume_action(&state, timestamp(1_000)),
        InteractionResumeAction::SuspendUncertain
    );
}
