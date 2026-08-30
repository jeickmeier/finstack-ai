use std::sync::Arc;

use finstack_ai_embeddings::embedder::{
    EmbedError, HashEmbedder, TextEmbedder, TextEmbedderDescriptor,
};
use finstack_ai_embeddings::vector::EmbeddingVector;
use finstack_ai_kernel::{
    ContentBlock, Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RunId,
    SessionId, TextBlock,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::context::{
    CONTEXT_BUDGET_EXCEEDED, CONTEXT_CONTRIBUTION_INVALID, ContextAuthority, ContextBudget,
    ContextCallContext, ContextItemKind, ContextOverflowPolicy, ContextProvider, ContextRequest,
};
use finstack_ai_runtime::ports::model::{AuthorizationContext, CancellationSignal, RunCallContext};

use crate::{provider::*, record::*, store::*};

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
        &InProcessArtifactStore::default(),
        MemoryScope::try_new(tenant).expect("scope"),
        RecallConfig::default(),
    )
    .expect("provider")
}

#[test]
fn provider_configuration_identity_covers_scope_and_recall_config() {
    let store = Arc::new(InProcessMemoryStore::new());
    let artifacts = InProcessArtifactStore::default();
    let build = |tenant: &str, max_hits: usize| {
        MemoryContextProvider::try_new(
            store.clone(),
            &artifacts,
            MemoryScope::try_new(tenant).expect("scope"),
            RecallConfig { max_hits },
        )
        .expect("provider")
        .descriptor()
        .invocation
        .configuration_digest
    };

    assert_eq!(build("tenant-a", 8), build("tenant-a", 8));
    assert_ne!(build("tenant-a", 8), build("tenant-b", 8));
    assert_ne!(build("tenant-a", 8), build("tenant-a", 4));
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

    let ids = |contribution: &finstack_ai_runtime::ports::context::ContextContribution| {
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

#[test]
fn semantic_evidence_recalls_in_its_own_tier_after_full_text() {
    assert_eq!(tier_of(&MatchEvidence::ExactId), 0);
    assert_eq!(tier_of(&MatchEvidence::Keyword(Arc::from("alpha"))), 0);
    assert_eq!(tier_of(&MatchEvidence::FullText), 1);
    assert_eq!(tier_of(&MatchEvidence::Semantic), 2);
}

/// A [`TextEmbedder`] whose every `embed` call fails, standing in for an
/// unreachable embedding backend.
struct FailingEmbedder;

impl TextEmbedder for FailingEmbedder {
    fn descriptor(&self) -> TextEmbedderDescriptor {
        TextEmbedderDescriptor {
            embedder_id: Arc::from("embed.test-failing.8"),
            dimensions: 8,
            max_input_bytes: 1024,
        }
    }

    fn embed(&self, _texts: Vec<Arc<str>>) -> PortFuture<Result<Vec<EmbeddingVector>, EmbedError>> {
        Box::pin(async {
            Err(EmbedError::Unavailable {
                message: Arc::from("test_embedder_down"),
            })
        })
    }
}

/// Delegates to an [`InProcessMemoryStore`] but fails every `Embedding`
/// search with the configured error: the stand-in for a store with no
/// embedding index (`InvalidRequest`, the trait's default-impl posture) or
/// a failing semantic backend (`Unavailable`).
struct EmbeddingRejectingStore {
    inner: Arc<InProcessMemoryStore>,
    error: MemoryStoreError,
}

impl MemoryStore for EmbeddingRejectingStore {
    fn put(
        &self,
        idempotency_key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>> {
        self.inner.put(idempotency_key, record)
    }

    fn get(
        &self,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>> {
        self.inner.get(scope, id)
    }

    fn search(
        &self,
        scope: MemoryScope,
        query: MemoryQuery,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>> {
        if matches!(query, MemoryQuery::Embedding { .. }) {
            let error = self.error.clone();
            return Box::pin(async move { Err(error) });
        }
        self.inner.search(scope, query, limit)
    }

    fn forget(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        self.inner.forget(idempotency_key, scope, id)
    }

    fn correct(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        replacement: MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        self.inner.correct(idempotency_key, scope, old, replacement)
    }

    fn list(
        &self,
        scope: MemoryScope,
        page: MemoryPage,
    ) -> PortFuture<Result<MemoryListing, MemoryStoreError>> {
        self.inner.list(scope, page)
    }
}

fn hash_embedder(dimensions: usize) -> Arc<dyn TextEmbedder> {
    Arc::new(HashEmbedder::try_new(dimensions).expect("embedder"))
}

fn embedder_provider(
    store: Arc<dyn MemoryStore>,
    tenant: &str,
    embedder: Arc<dyn TextEmbedder>,
) -> MemoryContextProvider {
    MemoryContextProvider::try_new_with_embedder(
        store,
        &InProcessArtifactStore::default(),
        MemoryScope::try_new(tenant).expect("scope"),
        RecallConfig::default(),
        embedder,
    )
    .expect("provider")
}

/// Seed the three-tier fixture: a keyword hit, a full-text hit, and a
/// record with no lexical overlap for the query `"alpha"` at all, then
/// index every record into the embedder's space.
async fn seed_three_tiers(store: &InProcessMemoryStore, embedder: &dyn TextEmbedder) {
    for (id, keywords, text) in [
        ("m-keyword", &["alpha"][..], "alpha note"),
        ("m-fulltext", &["gamma"][..], "alphabet chart"),
        ("m-semantic", &["appearance"][..], "night colors on screens"),
    ] {
        store
            .put(
                Arc::from(id),
                keyworded_record(id, "tenant-a", keywords, text),
            )
            .await
            .expect("put");
    }
    let indexed = reconcile_memory_embeddings(store, embedder, 16)
        .await
        .expect("reconcile");
    assert_eq!(indexed, 3);
}

fn source_refs(
    contribution: &finstack_ai_runtime::ports::context::ContextContribution,
) -> Vec<Option<Arc<str>>> {
    contribution
        .items
        .iter()
        .map(|item| item.provenance.source_ref.clone())
        .collect()
}

/// The semantic leg recalls a record sharing no keyword and no substring
/// with the query, once and only after every lexical hit; records hit by a
/// lexical leg and the semantic leg appear once with the lexical evidence
/// (which is what keeps them in the earlier tiers).
#[tokio::test]
async fn semantic_leg_recalls_unmatched_records_after_lexical_hits() {
    let store = Arc::new(InProcessMemoryStore::new());
    let embedder = hash_embedder(64);
    seed_three_tiers(&store, embedder.as_ref()).await;
    let provider = embedder_provider(store, "tenant-a", embedder);

    let contribution = provider
        .collect(context(), request("alpha"))
        .await
        .expect("collect");
    assert_eq!(
        source_refs(&contribution),
        vec![
            Some(Arc::from("m-keyword")),
            Some(Arc::from("m-fulltext")),
            Some(Arc::from("m-semantic")),
        ]
    );
}

/// A failing embedder silently degrades the provider to its lexical legs:
/// same items as an embedder-less provider, and no error.
#[tokio::test]
async fn failing_embedder_degrades_to_lexical_recall() {
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put(
            Arc::from("m-keyword"),
            keyworded_record("m-keyword", "tenant-a", &["alpha"], "alpha note"),
        )
        .await
        .expect("put");
    let provider = embedder_provider(store, "tenant-a", Arc::new(FailingEmbedder));

    let contribution = provider
        .collect(context(), request("alpha"))
        .await
        .expect("embedder failure must not fail recall");
    assert_eq!(
        source_refs(&contribution),
        vec![Some(Arc::from("m-keyword"))]
    );
}

/// A store that rejects `Embedding` queries — whether as unsupported
/// (`InvalidRequest`, the trait default posture) or unavailable — likewise
/// degrades to the lexical legs without an error.
#[tokio::test]
async fn embedding_rejecting_store_degrades_to_lexical_recall() {
    for error in [
        MemoryStoreError::InvalidRequest {
            reason: "memory_embeddings_unsupported",
        },
        MemoryStoreError::Unavailable {
            message: Arc::from("test_backend_down"),
        },
    ] {
        let inner = Arc::new(InProcessMemoryStore::new());
        inner
            .put(
                Arc::from("m-keyword"),
                keyworded_record("m-keyword", "tenant-a", &["alpha"], "alpha note"),
            )
            .await
            .expect("put");
        let store = Arc::new(EmbeddingRejectingStore { inner, error });
        let provider = embedder_provider(store, "tenant-a", hash_embedder(64));

        let contribution = provider
            .collect(context(), request("alpha"))
            .await
            .expect("store rejection must not fail recall");
        assert_eq!(
            source_refs(&contribution),
            vec![Some(Arc::from("m-keyword"))]
        );
    }
}

/// The embedder is part of the provider's configured identity: presence and
/// embedder id both change the configuration digest, and the same embedder
/// reproduces the same digest.
#[test]
fn configuration_digest_covers_the_embedder_identity() {
    let store = Arc::new(InProcessMemoryStore::new());
    let artifacts = InProcessArtifactStore::default();
    let scope = || MemoryScope::try_new("tenant-a").expect("scope");
    let digest_without = || {
        MemoryContextProvider::try_new(
            store.clone(),
            &artifacts,
            scope(),
            RecallConfig::default(),
        )
        .expect("provider")
        .descriptor()
        .invocation
        .configuration_digest
    };
    let digest_with = |dimensions: usize| {
        MemoryContextProvider::try_new_with_embedder(
            store.clone(),
            &artifacts,
            scope(),
            RecallConfig::default(),
            hash_embedder(dimensions),
        )
        .expect("provider")
        .descriptor()
        .invocation
        .configuration_digest
    };

    assert_ne!(digest_without(), digest_with(64));
    assert_ne!(digest_with(64), digest_with(32));
    assert_eq!(digest_with(64), digest_with(64));
    assert_eq!(digest_without(), digest_without());
}

/// The recall contract changed with the semantic leg, so the descriptor
/// version is 0.2.0 with and without an embedder.
#[test]
fn descriptor_version_is_bumped_unconditionally() {
    let store = Arc::new(InProcessMemoryStore::new());
    let artifacts = InProcessArtifactStore::default();
    let versions = [
        MemoryContextProvider::try_new(
            store.clone(),
            &artifacts,
            MemoryScope::try_new("tenant-a").expect("scope"),
            RecallConfig::default(),
        )
        .expect("provider")
        .descriptor()
        .invocation
        .version,
        MemoryContextProvider::try_new_with_embedder(
            store,
            &artifacts,
            MemoryScope::try_new("tenant-a").expect("scope"),
            RecallConfig::default(),
            hash_embedder(64),
        )
        .expect("provider")
        .descriptor()
        .invocation
        .version,
    ];
    for version in versions {
        assert_eq!((version.major, version.minor, version.patch), (0, 2, 0));
    }
}

/// Identical collects through the semantic leg keep the contribution cache
/// key stable: the leg is a pure function of the store's records and index.
#[tokio::test]
async fn cache_key_stable_across_identical_semantic_collects() {
    let store = Arc::new(InProcessMemoryStore::new());
    let embedder = hash_embedder(64);
    seed_three_tiers(&store, embedder.as_ref()).await;
    let provider = embedder_provider(store, "tenant-a", embedder);

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
    assert_eq!(first, second);
}
