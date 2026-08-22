#[test]
fn unpaired_projection_is_uncertain() {
    let mut state = finstack_ai_kernel::KernelState::default();
    state.set_phase(Some(RunPhase::AwaitingInteraction));
    assert_eq!(
        interaction_resume_action(&state, timestamp(1_000)),
        InteractionResumeAction::SuspendUncertain
    );
}
