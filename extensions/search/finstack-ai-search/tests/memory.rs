//! Real memory-store conformance, federation parity, and correction coverage.

use std::sync::Arc;

use finstack_ai_embeddings::embedder::{HashEmbedder, TextEmbedder};
use finstack_ai_kernel::{Sensitivity, UNIX_EPOCH};
use finstack_ai_memory::record::{
    ExtractionMethod, MemoryBody, MemoryId, MemoryProvenance, MemoryRecord, MemoryScope,
    RetentionPolicy,
};
use finstack_ai_memory::search::MemorySearchSource;
use finstack_ai_memory::store::{
    EmbeddingCoverage, InProcessMemoryStore, MemoryStore, SqliteMemoryStore,
    reconcile_memory_embeddings,
};
use finstack_ai_search::*;
use finstack_ai_search_core::test_support::check_source_contract;

fn record(id: &str, tenant: &str, text: &str) -> MemoryRecord {
    MemoryRecord {
        id: MemoryId::parse(id).unwrap(),
        scope: MemoryScope::try_new(tenant).unwrap(),
        keywords: vec![Arc::from("retention")].into(),
        body: MemoryBody::Inline(text.into()),
        preview: text.into(),
        sensitivity: Sensitivity::Confidential,
        provenance: MemoryProvenance {
            source_session: None,
            source_run: None,
            source_ref: Some("handbook.section.3".into()),
            extraction: ExtractionMethod::Explicit,
            confidence: 100,
        },
        created_at: UNIX_EPOCH,
        last_confirmed_at: UNIX_EPOCH,
        supersedes: None,
        superseded_by: None,
        retention: RetentionPolicy::KeepUntilDeleted,
        tombstoned: false,
    }
}

fn config() -> SearchConfig {
    SearchConfig {
        graph_expansion: None,
        scope: SearchScope::try_new("tenant").unwrap(),
        default_plan: HybridPlan {
            legs: vec![
                HybridLeg {
                    source: "memory".into(),
                    strategy: SearchStrategy::Lexical(LexicalKind::Keyword),
                    weight_micros: 1_000_000,
                },
                HybridLeg {
                    source: "memory".into(),
                    strategy: SearchStrategy::Lexical(LexicalKind::Bm25),
                    weight_micros: 500_000,
                },
            ],
            fusion: Fusion::default(),
        },
        limits: SearchLimits::default(),
        max_concurrency: 2,
        source_timeout_ms: 1000,
    }
}

#[allow(clippy::too_many_lines)] // One ordered correction/forget lifecycle run against both stores.
async fn exercise(store: Arc<dyn MemoryStore>) -> SearchResponse {
    store
        .put(
            "first".into(),
            record("a", "tenant", "retention is seven years"),
        )
        .await
        .unwrap();
    store
        .put(
            "other".into(),
            record("a", "other", "secret other-tenant retention"),
        )
        .await
        .unwrap();
    let scope = SearchScope::try_new("tenant").unwrap();
    let embedder: Arc<dyn TextEmbedder> = Arc::new(HashEmbedder::try_new(64).unwrap());
    let space = embedder.descriptor().embedder_id;
    let source = Arc::new(
        MemorySearchSource::try_new(
            "memory",
            Arc::clone(&store),
            scope.clone(),
            ScopeMapping::Exact,
            SearchLimits::default(),
            Some(Arc::clone(&embedder)),
        )
        .unwrap(),
    );
    let query = SearchQuery::lexical("retention", LexicalKind::Keyword);
    check_source_contract(source.as_ref(), scope.clone(), query.clone())
        .await
        .unwrap();
    let semantic = SearchQuery {
        strategy: SearchStrategy::Semantic {
            space: Arc::clone(&space),
        },
        ..query.clone()
    };
    let pending = source
        .search(scope.clone(), semantic.clone(), 8)
        .await
        .unwrap();
    assert_eq!(pending.status, SourceStatus::Unavailable);
    assert!(pending.hits.is_empty());
    assert!(
        pending
            .reasons
            .iter()
            .any(|reason| reason.as_ref() == "memory_embeddings_pending")
    );
    assert_eq!(
        reconcile_memory_embeddings(store.as_ref(), embedder.as_ref(), 8)
            .await
            .unwrap(),
        2
    );
    let indexed = source
        .search(scope.clone(), semantic.clone(), 8)
        .await
        .unwrap();
    assert_eq!(indexed.status, SourceStatus::Completed);
    assert_eq!(indexed.hits.len(), 1);
    assert_eq!(indexed.hits[0].sensitivity, Sensitivity::Confidential);
    let engine = SearchEngine::try_new(config(), vec![source.clone()]).unwrap();
    let fused = engine
        .search(SearchRequest::text("retention"))
        .await
        .unwrap();
    assert_eq!(fused.hits.len(), 1);
    assert_eq!(fused.hits[0].evidence.len(), 2);
    assert!(
        fused.hits[0]
            .evidence
            .iter()
            .all(|e| e.provenance.scope == scope)
    );
    assert!(
        fused.hits[0].evidence[0]
            .provenance
            .locators
            .iter()
            .any(|l| l.as_ref() == "source:handbook.section.3")
    );
    let memory_scope = MemoryScope::try_new("tenant").unwrap();
    store
        .correct(
            "correct".into(),
            memory_scope.clone(),
            MemoryId::parse("a").unwrap(),
            record("b", "tenant", "retention is ten years"),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .embedding_coverage(memory_scope.clone(), Arc::clone(&space))
            .await
            .unwrap(),
        Some(EmbeddingCoverage {
            live_records: 1,
            indexed_records: 0
        })
    );
    assert!(
        source
            .search(scope.clone(), semantic.clone(), 8)
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    reconcile_memory_embeddings(store.as_ref(), embedder.as_ref(), 8)
        .await
        .unwrap();
    let corrected = source.search(scope.clone(), semantic, 8).await.unwrap();
    assert_eq!(
        corrected.hits[0].reference,
        SourceRef::Memory { id: "b".into() }
    );
    store
        .forget("forget".into(), memory_scope, MemoryId::parse("b").unwrap())
        .await
        .unwrap();
    assert!(
        source
            .search(scope, query, 8)
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    fused
}

#[tokio::test]
async fn memory_sources_share_scoped_lexical_semantic_and_correction_contracts() {
    let mut memory = exercise(Arc::new(InProcessMemoryStore::new())).await;
    let dir = tempfile::tempdir().unwrap();
    let mut disk = exercise(Arc::new(
        SqliteMemoryStore::try_open(&dir.path().join("memory.sqlite")).unwrap(),
    ))
    .await;
    // Store-owned raw full-text scores deliberately differ (match count versus
    // SQLite rank). Verify they survive unchanged while normalized fusion,
    // citations, scope, coverage, and every other result field agree.
    assert_eq!(memory.hits[0].evidence[0].source_score, 10);
    assert_eq!(disk.hits[0].evidence[0].source_score, 1);
    for response in [&mut memory, &mut disk] {
        for hit in &mut response.hits {
            for evidence in &mut hit.evidence {
                evidence.source_score = 0;
            }
        }
    }
    assert_eq!(memory, disk);
}

#[tokio::test]
async fn restrictive_mapping_and_reconstruction_preserve_exact_complete_scope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.sqlite");
    let mut scoped = record("a", "tenant", "retention bound to Alice");
    scoped.scope = scoped
        .scope
        .try_with_user("alice")
        .unwrap()
        .try_with_workspace("research")
        .unwrap();
    let mut scope = SearchScope::try_new("tenant").unwrap();
    scope.user = Some("alice".into());
    scope.workspace = Some("research".into());
    let store = Arc::new(SqliteMemoryStore::try_open(&path).unwrap());
    store.put("scoped".into(), scoped).await.unwrap();
    let source = MemorySearchSource::try_new(
        "memory",
        store,
        scope.clone(),
        ScopeMapping::RestrictToBound,
        SearchLimits::default(),
        None,
    )
    .unwrap();
    let query = SearchQuery::lexical("retention", LexicalKind::Keyword);
    let tenant = SearchScope::try_new("tenant").unwrap();
    let first = source
        .search(tenant.clone(), query.clone(), 8)
        .await
        .unwrap();
    assert_eq!(first.hits[0].provenance.scope, scope);
    let mut denied = scope.clone();
    denied.workspace = Some("other".into());
    assert_eq!(
        source.search(denied, query.clone(), 8).await,
        Err(SearchError::SearchScopeDenied)
    );
    drop(source);
    let reopened = MemorySearchSource::try_new(
        "memory",
        Arc::new(SqliteMemoryStore::try_open(&path).unwrap()),
        scope,
        ScopeMapping::RestrictToBound,
        SearchLimits::default(),
        None,
    )
    .unwrap();
    assert_eq!(reopened.search(tenant, query, 8).await.unwrap(), first);
}

struct RejectDispatchEmbedder;
impl TextEmbedder for RejectDispatchEmbedder {
    fn descriptor(&self) -> finstack_ai_embeddings::embedder::TextEmbedderDescriptor {
        HashEmbedder::try_new(64).unwrap().descriptor()
    }
    fn embed(
        &self,
        _: Vec<Arc<str>>,
    ) -> finstack_ai_runtime::ports::PortFuture<
        Result<
            Vec<finstack_ai_embeddings::vector::EmbeddingVector>,
            finstack_ai_embeddings::embedder::EmbedError,
        >,
    > {
        panic!("byte admission must precede embedder dispatch");
    }
}

#[tokio::test]
async fn semantic_byte_admission_precedes_query_and_rebuild_dispatch() {
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put("one".into(), record("one", "tenant", "retention"))
        .await
        .unwrap();
    let scope = SearchScope::try_new("tenant").unwrap();
    let embedder = Arc::new(RejectDispatchEmbedder);
    let source = MemorySearchSource::try_new(
        "memory",
        store,
        scope.clone(),
        ScopeMapping::Exact,
        SearchLimits {
            max_embedding_bytes: 255,
            ..Default::default()
        },
        Some(embedder.clone()),
    )
    .unwrap();
    let query = SearchQuery {
        strategy: SearchStrategy::Semantic {
            space: embedder.descriptor().embedder_id,
        },
        ..SearchQuery::lexical("retention", LexicalKind::Keyword)
    };
    let expected = SearchError::SearchCapacityExceeded {
        resource: "embedding_bytes".into(),
    };
    assert_eq!(
        source.search(scope.clone(), query, 8).await,
        Err(expected.clone())
    );
    assert_eq!(source.reconcile_embeddings(0, 8).await, Err(expected));
    assert_eq!(
        source
            .search(
                scope,
                SearchQuery::lexical("retention", LexicalKind::Keyword),
                8
            )
            .await
            .unwrap()
            .hits
            .len(),
        1
    );
}
