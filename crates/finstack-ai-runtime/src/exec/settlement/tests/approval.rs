fn pending_prompt(coordinator: &CommitCoordinator) -> String {
    let pending = coordinator
        .state()
        .pending_interaction
        .as_ref()
        .expect("pending approval");
    assert_eq!(pending.request.kind(), &InteractionKind::Approval);
    match pending.request.prompt() {
        [ContentBlock::Text(text)] => text.text().to_owned(),
        other => panic!("expected one text prompt, got {other:?}"),
    }
}

fn pending_metadata(coordinator: &CommitCoordinator) -> String {
    coordinator
        .state()
        .pending_interaction
        .as_ref()
        .expect("pending approval")
        .request
        .metadata()
        .as_raw_json()
        .as_str()
        .to_owned()
}

fn resolve_approval(
    coordinator: &mut CommitCoordinator,
    approved: bool,
    resolution_id: &str,
    record_base: u64,
) {
    let interaction_id = coordinator
        .state()
        .pending_interaction
        .as_ref()
        .expect("pending approval")
        .request
        .interaction_id();
    let response = if approved {
        br#"{"approved":true}"#.as_slice()
    } else {
        br#"{"approved":false}"#
    };
    let resolution = InteractionResolution::try_new(
        interaction_id,
        resolution_id,
        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        RawJson::parse(response).expect("response"),
        None::<&str>,
    )
    .expect("resolution");
    block_on(coordinator.submit(
        env(
            1_700,
            &[record_base, record_base + 1],
            &[record_base, record_base + 1],
            &[],
            &[],
            &[],
            &[],
            record_base + 10,
        ),
        KernelInput::InteractionSettled(InteractionSettled::Resolved(resolution)),
    ))
    .expect("resolved");
}

fn prepare_opened(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    sources: &SettlementSources<ExternalClock, CountingRandom>,
) -> bool {
    block_on(prepare_tool_batch_if_ready(
        coordinator,
        catalog,
        sources,
        None,
    ))
    .expect("prepare")
}

#[test]
fn one_paid_tool_prompt_includes_name_and_canonical_arguments() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch_calls(
        &store,
        &[("paid", br#"{"path":"/tmp/out"}"#)],
        None,
    );
    let catalog = catalog_for(&[("paid", ApprovalRequirement::Policy)]);
    let sources = sources_at(1_600);
    assert!(!prepare_opened(&mut coordinator, &catalog, &sources));
    let prompt = pending_prompt(&coordinator);
    assert!(
        prompt.contains("paid") && prompt.contains(r#"{"path":"/tmp/out"}"#),
        "{prompt}"
    );
    let metadata = pending_metadata(&coordinator);
    assert!(metadata.contains("tool_call_ids"), "{metadata}");
    assert!(store.opened_tool_batch().is_none());
}

#[test]
fn per_call_parks_twice_and_refusal_does_not_authorize_the_other_call() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &["alpha", "beta"], None);
    let catalog = catalog_for(&[
        ("alpha", ApprovalRequirement::Policy),
        ("beta", ApprovalRequirement::Required),
    ]);
    let sources = sources_at(1_600);
    assert!(!prepare_opened(&mut coordinator, &catalog, &sources));
    let first = pending_prompt(&coordinator);
    assert!(first.contains("alpha"), "{first}");
    assert!(!first.contains("beta"), "{first}");
    resolve_approval(&mut coordinator, false, "resolution-1", 900);
    assert!(!prepare_opened(&mut coordinator, &catalog, &sources));
    let second = pending_prompt(&coordinator);
    assert!(second.contains("beta"), "{second}");
    assert!(!second.contains("alpha"), "{second}");
    resolve_approval(&mut coordinator, true, "resolution-2", 920);
    assert!(prepare_opened(&mut coordinator, &catalog, &sources));
    let plans = store.opened_tool_batch().expect("opened").calls;
    assert_eq!(plans.len(), 2);
    assert!(
        matches!(plans[0].plan, ToolCallPlan::SyntheticClosure(_)),
        "{:?}",
        plans[0].plan
    );
    assert!(
        matches!(plans[1].plan, ToolCallPlan::Execute(_)),
        "{:?}",
        plans[1].plan
    );
}

#[test]
fn informed_batch_parks_once_listing_every_unpaid_paid_tool() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &["alpha", "beta"], None);
    let catalog = catalog_for(&[
        ("alpha", ApprovalRequirement::Policy),
        ("beta", ApprovalRequirement::Policy),
    ]);
    let sources = sources_at(1_600);
    sources.set_approval_grant(ApprovalGrantMode::InformedBatch);
    assert!(!prepare_opened(&mut coordinator, &catalog, &sources));
    let prompt = pending_prompt(&coordinator);
    assert!(
        prompt.contains("alpha") && prompt.contains("beta"),
        "{prompt}"
    );
    let metadata = pending_metadata(&coordinator);
    let parsed: serde_json::Value = serde_json::from_str(&metadata).expect("metadata json");
    assert_eq!(
        parsed["tool_call_ids"]
            .as_array()
            .map(Vec::len)
            .expect("tool_call_ids"),
        2,
        "{metadata}"
    );
    resolve_approval(&mut coordinator, true, "resolution-batch", 900);
    assert!(prepare_opened(&mut coordinator, &catalog, &sources));
    let plans = store.opened_tool_batch().expect("opened").calls;
    assert_eq!(plans.len(), 2);
    assert!(
        plans
            .iter()
            .all(|assigned| matches!(assigned.plan, ToolCallPlan::Execute(_))),
        "{plans:?}"
    );
}

#[test]
fn policy_tool_with_host_allow_still_parks() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &["paid"], None);
    let catalog = catalog_for(&[("paid", ApprovalRequirement::Policy)]);
    let sources = sources_at(1_600);
    assert!(!prepare_opened(&mut coordinator, &catalog, &sources));
    assert!(pending_prompt(&coordinator).contains("paid"));
    assert!(store.opened_tool_batch().is_none());
}

#[test]
fn per_call_journaled_grant_applies_after_sources_are_replaced() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &["paid"], None);
    let catalog = catalog_for(&[("paid", ApprovalRequirement::Policy)]);
    let sources = sources_at(1_600);
    assert!(!prepare_opened(&mut coordinator, &catalog, &sources));
    resolve_approval(&mut coordinator, true, "resolution-survives", 900);
    let recovered = sources_at(1_700);
    assert!(prepare_opened(&mut coordinator, &catalog, &recovered));
    let plans = store.opened_tool_batch().expect("opened").calls;
    assert_eq!(plans.len(), 1);
    assert!(matches!(plans[0].plan, ToolCallPlan::Execute(_)));
}

#[test]
fn per_call_journaled_deny_applies_after_sources_are_replaced() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &["paid"], None);
    let catalog = catalog_for(&[("paid", ApprovalRequirement::Policy)]);
    let sources = sources_at(1_600);
    assert!(!prepare_opened(&mut coordinator, &catalog, &sources));
    resolve_approval(&mut coordinator, false, "resolution-denied", 900);
    let recovered = sources_at(1_700);
    assert!(prepare_opened(&mut coordinator, &catalog, &recovered));
    let plans = store.opened_tool_batch().expect("opened").calls;
    assert_eq!(plans.len(), 1);
    assert!(matches!(plans[0].plan, ToolCallPlan::SyntheticClosure(_)));
}

#[test]
fn get_video_not_required_runs_without_a_grant() {
    let store = Arc::new(MemoryStore::new());
    let mut coordinator = coordinator_at_before_tool_batch(&store, &["get_video"], None);
    let catalog = catalog_for(&[("get_video", ApprovalRequirement::NotRequired)]);
    let sources = sources_at(1_600);
    assert!(prepare_opened(&mut coordinator, &catalog, &sources));
    let plans = store.opened_tool_batch().expect("opened").calls;
    assert_eq!(plans.len(), 1);
    assert!(matches!(plans[0].plan, ToolCallPlan::Execute(_)));
}
