use std::sync::Arc;

use finstack_ai_kernel::{
    ContentBlock, Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RunId,
    SessionId, TextBlock,
};
use finstack_ai_runtime::{
    AuthorizationContext, CONTEXT_BUDGET_EXCEEDED, CONTEXT_CONTRIBUTION_INVALID,
    CancellationSignal, ContextAuthority, ContextBudget, ContextCallContext, ContextItemKind,
    ContextOverflowPolicy, ContextProvider, ContextRequest, RunCallContext,
};

use crate::*;

fn id<T>(value: u64, parse: impl FnOnce(&str) -> T) -> T {
    parse(&format!("00000000-0000-7000-8000-{value:012x}"))
}

fn context() -> ContextCallContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    ContextCallContext {
        run: RunCallContext {
            relation_depth: 0,
            locator: OperationLocator::try_new(
                "tenant-a",
                id(1, |value| SessionId::parse(value).expect("session")),
                id(2, |value| LaneId::parse(value).expect("lane")),
                id(3, |value| RunId::parse(value).expect("run")),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal,
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: id(4, |value| EffectId::parse(value).expect("effect")),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        provider_index: 0,
        chain_digest: Digest::raw_json(b"chain"),
    }
}

fn request(query: &str) -> ContextRequest {
    ContextRequest {
        session_id: id(1, |value| SessionId::parse(value).expect("session")),
        lane_id: id(2, |value| LaneId::parse(value).expect("lane")),
        run_id: id(3, |value| RunId::parse(value).expect("run")),
        user_input: Arc::from([ContentBlock::Text(
            TextBlock::try_new(query).expect("query"),
        )]),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 8,
            max_tokens: 1_000,
            max_bytes: 64 * 1024,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    }
}

/// A [`MemoryRecord`] carrying `keywords` and `preview`, scoped to `tenant`,
/// otherwise identical to [`crate::tests::sample_record`].
fn keyworded_record(id: &str, tenant: &str, keywords: &[&str], preview: &str) -> MemoryRecord {
    let mut record = crate::tests::sample_record(id, tenant);
    record.keywords = keywords
        .iter()
        .map(|value| Arc::<str>::from(*value))
        .collect();
    record.preview = Arc::from(preview);
    record.body = MemoryBody::Inline(Arc::from(preview));
    record
}

fn provider_for(store: Arc<InProcessMemoryStore>, tenant: &str) -> MemoryContextProvider {
    MemoryContextProvider::try_new(
        store,
        MemoryScope::try_new(tenant).expect("scope"),
        RecallConfig::default(),
    )
    .expect("provider")
}

#[tokio::test]
async fn keyword_and_full_text_retrieval_use_the_store() {
    let store = Arc::new(InProcessMemoryStore::new());
    let provider = provider_for(Arc::clone(&store), "tenant-a");
    store
        .put(
            Arc::from("k1"),
            keyworded_record(
                "note-1",
                "tenant-a",
                &["alpha", "ledger"],
                "remembered ledger note",
            ),
        )
        .await
        .expect("put");

    let by_keyword = provider
        .collect(context(), request("please recall ledger"))
        .await
        .expect("keyword");
    assert_eq!(by_keyword.items.len(), 1);
    assert_eq!(by_keyword.items[0].kind, ContextItemKind::Reference);
    assert_eq!(by_keyword.items[0].authority, ContextAuthority::Untrusted);
    assert_eq!(
        by_keyword.items[0].provenance.source_ref.as_deref(),
        Some("note-1")
    );

    let miss = provider
        .collect(context(), request("unrelated"))
        .await
        .expect("miss");
    assert!(miss.items.is_empty());
}

#[tokio::test]
async fn foreign_tenant_cannot_collect() {
    let store = Arc::new(InProcessMemoryStore::new());
    let provider = provider_for(store, "tenant-b");
    let error = provider
        .collect(context(), request("ledger"))
        .await
        .expect_err("tenant mismatch");
    assert_eq!(error.code(), CONTEXT_CONTRIBUTION_INVALID);
}

#[tokio::test]
async fn budget_reject_errors_when_max_items_exceeded() {
    let store = Arc::new(InProcessMemoryStore::new());
    let provider = provider_for(Arc::clone(&store), "tenant-a");
    for record_id in ["m-a", "m-b"] {
        store
            .put(
                Arc::from(record_id),
                keyworded_record(record_id, "tenant-a", &["alpha"], "alpha note"),
            )
            .await
            .expect("put");
    }

    let mut over_budget = request("alpha");
    over_budget.budget.max_items = 1;
    over_budget.budget.overflow = ContextOverflowPolicy::Reject;

    let error = provider
        .collect(context(), over_budget)
        .await
        .expect_err("budget reject");
    assert_eq!(error.code(), CONTEXT_BUDGET_EXCEEDED);
}

#[tokio::test]
async fn budget_truncates_with_diagnostic_when_configured() {
    let store = Arc::new(InProcessMemoryStore::new());
    let provider = provider_for(Arc::clone(&store), "tenant-a");
    for record_id in ["m-a", "m-b"] {
        store
            .put(
                Arc::from(record_id),
                keyworded_record(record_id, "tenant-a", &["alpha"], "alpha note"),
            )
            .await
            .expect("put");
    }

    let mut over_budget = request("alpha");
    over_budget.budget.max_items = 1;
    over_budget.budget.overflow = ContextOverflowPolicy::TruncateWithDiagnostic;

    let contribution = provider
        .collect(context(), over_budget)
        .await
        .expect("truncated contribution");
    assert_eq!(contribution.items.len(), 1);
}

#[tokio::test]
async fn recall_order_is_deterministic_by_tier_then_id() {
    let store = Arc::new(InProcessMemoryStore::new());
    let provider = provider_for(Arc::clone(&store), "tenant-a");
    for record_id in ["m-c", "m-a", "m-b"] {
        store
            .put(
                Arc::from(record_id),
                keyworded_record(record_id, "tenant-a", &["alpha"], "alpha note"),
            )
            .await
            .expect("put");
    }

    let first = provider
        .collect(context(), request("alpha"))
        .await
        .expect("first collect");
    let second = provider
        .collect(context(), request("alpha"))
        .await
        .expect("second collect");

    let ids = |contribution: &finstack_ai_runtime::ContextContribution| {
        contribution
            .items
            .iter()
            .map(|item| item.provenance.source_ref.clone())
            .collect::<Vec<_>>()
    };
    let expected = vec![
        Some(Arc::from("m-a")),
        Some(Arc::from("m-b")),
        Some(Arc::from("m-c")),
    ];
    assert_eq!(ids(&first), expected);
    assert_eq!(ids(&second), expected);
    assert_eq!(first, second);
}

#[tokio::test]
async fn cache_key_stable_when_recall_unchanged() {
    let store = Arc::new(InProcessMemoryStore::new());
    let provider = provider_for(Arc::clone(&store), "tenant-a");
    store
        .put(
            Arc::from("m-a"),
            keyworded_record("m-a", "tenant-a", &["alpha"], "alpha note"),
        )
        .await
        .expect("put");

    let first = provider
        .collect(context(), request("alpha"))
        .await
        .expect("first collect");
    let second = provider
        .collect(context(), request("alpha"))
        .await
        .expect("second collect");
    assert!(first.cache_key.is_some());
    assert_eq!(first.cache_key, second.cache_key);

    store
        .put(
            Arc::from("m-b"),
            keyworded_record("m-b", "tenant-a", &["alpha"], "alpha note"),
        )
        .await
        .expect("put");
    let third = provider
        .collect(context(), request("alpha"))
        .await
        .expect("third collect");
    assert_ne!(third.cache_key, second.cache_key);
}

#[tokio::test]
async fn recall_skips_tombstoned_records() {
    let store = Arc::new(InProcessMemoryStore::new());
    let provider = provider_for(Arc::clone(&store), "tenant-a");
    store
        .put(
            Arc::from("m-a"),
            keyworded_record("m-a", "tenant-a", &["alpha"], "alpha note"),
        )
        .await
        .expect("put");
    store
        .forget(
            Arc::from("forget-1"),
            MemoryScope::try_new("tenant-a").expect("scope"),
            MemoryId::parse("m-a").expect("id"),
        )
        .await
        .expect("forget");

    let contribution = provider
        .collect(context(), request("alpha"))
        .await
        .expect("collect");
    assert!(contribution.items.is_empty());
}
