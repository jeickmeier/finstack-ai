#[tokio::test]
async fn prefix_a1_through_a2() {
    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let activation = CapabilitiesActivated {
        prior_plan_digest: None,
        resolved_plan_digest: Digest::raw_json(br#"{"plan":1}"#),
        active: Arc::from([ActiveCapability {
            capability_id: finstack_ai_kernel::CapabilityId::parse("finstack.capability.alpha")
                .expect("capability"),
            source: CapabilityActivationSource::Always,
        }]),
    };
    coordinator
        .submit(
            env(1_050, &[2], &[], &[], &[], &[], &[], 102),
            KernelInput::CapabilitiesActivated(activation.clone()),
        )
        .await
        .expect("activate");
    let catalog = coordinator.state().active_capabilities.clone();
    drop(coordinator);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().active_capabilities, catalog);
    assert_legal("A1", recovered.state().phase, LegalRestore::Retryable);
    let duplicate = recovered
        .submit(
            env(1_060, &[3], &[], &[], &[], &[], &[], 103),
            KernelInput::CapabilitiesActivated(activation),
        )
        .await
        .expect("equal activation");
    assert!(duplicate.committed.is_none(), "A2 no second activation");
    assert_eq!(recovered.state().active_capabilities, catalog);
}
