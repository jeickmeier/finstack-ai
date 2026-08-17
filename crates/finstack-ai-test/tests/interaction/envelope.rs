async fn park_on_kind(kind: InteractionKind) -> (ApprovalPorts, RunTaskOwner, InteractionId) {
    let ports = approval_ports();
    let owner = spawn_owner(
        CommitCoordinator::new(ports.store.clone()),
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_500,
        710,
    )
    .await
    .expect("owner");
    drive_to_after_model(
        &owner.handle(),
        &ports.store,
        Arc::clone(&ports.tools),
        None,
    )
    .await;
    owner
        .handle()
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(kind),
            }),
        )
        .await
        .expect("request");
    wait_phase(&ports.store, RunPhase::AwaitingInteraction).await;
    (ports, owner, id::<InteractionTag>(501))
}

async fn resolve_kind_envelope(kind: InteractionKind) -> Vec<&'static str> {
    let (ports, owner, interaction_id) = park_on_kind(kind).await;
    drop(owner);
    router(ports.store.clone())
        .await
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("resolve");
    record_kinds(&ports.store).await
}

fn assert_shared_envelope(kinds: &[&str]) {
    for required in [
        "effect_requested_interaction",
        "interaction_requested",
        "interaction_resolved",
        "effect_completed",
    ] {
        assert!(kinds.contains(&required), "missing {required} in {kinds:?}");
    }
}

#[tokio::test]
async fn approval_choice_and_review_share_the_same_envelope() {
    for kind in [
        InteractionKind::Approval,
        InteractionKind::Choice,
        InteractionKind::Review,
    ] {
        let kinds = resolve_kind_envelope(kind.clone()).await;
        assert_shared_envelope(&kinds);
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == "interaction_requested")
                .count(),
            1,
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn remaining_kinds_request_and_resolve() {
    for kind in [
        InteractionKind::Form,
        InteractionKind::FreeText,
        InteractionKind::Correction,
        InteractionKind::Custom {
            name: Arc::from("team.custom"),
        },
    ] {
        let kinds = resolve_kind_envelope(kind.clone()).await;
        assert_shared_envelope(&kinds);
    }
}
