//! Deterministic extraction against real memory stores and scoped live evidence.
use finstack_ai_index_graph::{
    EdgeKind, EdgeRule, EntityRule, GraphIndexConfig, GraphSearchSource, GraphVocabulary,
};
use finstack_ai_kernel::{Sensitivity, UNIX_EPOCH};
use finstack_ai_memory::{
    record::{
        ExtractionMethod, MemoryBody, MemoryId, MemoryProvenance, MemoryRecord, MemoryScope,
        RetentionPolicy,
    },
    search::MemorySearchSource,
    store::{InProcessMemoryStore, MemoryStore},
};
use finstack_ai_search_core::*;
use std::{path::Path, sync::Arc};

fn vocabulary() -> GraphVocabulary {
    GraphVocabulary {
        version: 1,
        entity_kinds: vec!["company".into()],
        edge_kinds: vec![EdgeKind {
            kind: "owns".into(),
            source_kind: "company".into(),
            target_kind: "company".into(),
        }],
        entity_rules: vec![EntityRule {
            kind: "company".into(),
            pattern: r"(?i)(?P<label>Acme Corp|ACME)\b".into(),
            canonical_label: Some("Acme".into()),
        }],
        edge_rules: vec![EdgeRule {
            kind: "owns".into(),
            pattern: r"(?P<source>[A-Z][A-Za-z ]*?) owns (?P<target>[A-Z][A-Za-z ]*)\.".into(),
        }],
    }
}
fn scope() -> SearchScope {
    SearchScope::try_new("tenant").unwrap()
}
fn reference(id: &str) -> SourceRef {
    SourceRef::Memory { id: id.into() }
}
fn record(id: &str, text: &str) -> MemoryRecord {
    MemoryRecord {
        id: MemoryId::parse(id).unwrap(),
        scope: MemoryScope::try_new("tenant").unwrap(),
        keywords: vec!["owns".into()].into(),
        body: MemoryBody::Inline(text.into()),
        preview: text.into(),
        sensitivity: Sensitivity::Confidential,
        provenance: MemoryProvenance {
            source_session: None,
            source_run: None,
            source_ref: Some("business-register".into()),
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
fn memory_source(store: Arc<dyn MemoryStore>) -> Arc<dyn SearchSource> {
    Arc::new(
        MemorySearchSource::try_new(
            "memory",
            store,
            scope(),
            ScopeMapping::Exact,
            SearchLimits::default(),
            None,
        )
        .unwrap(),
    )
}
fn open(path: &Path, store: Arc<dyn MemoryStore>) -> GraphSearchSource {
    GraphSearchSource::try_open(
        path,
        GraphIndexConfig::new("graph", scope(), vocabulary()),
        vec![memory_source(store)],
    )
    .unwrap()
}
async fn query(source: &GraphSearchSource, text: &str, graph: GraphQuery) -> SourceResult {
    source
        .search(
            scope(),
            SearchQuery {
                strategy: SearchStrategy::Graph(graph),
                ..SearchQuery::lexical(text, LexicalKind::Literal)
            },
            32,
        )
        .await
        .unwrap()
}
fn neighborhood(depth: u8) -> GraphQuery {
    GraphQuery::Neighborhood {
        depth,
        max_nodes: 100,
        max_edges: 1000,
    }
}

#[tokio::test]
async fn aliases_relationships_cycles_direction_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.sqlite");
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put(
            "one".into(),
            record(
                "a",
                "Acme Corp owns Beta. Beta owns Gamma. Gamma owns Acme Corp.",
            ),
        )
        .await
        .unwrap();
    let source = open(&path, store.clone());
    let first = source
        .index_reference("memory", reference("a"))
        .await
        .unwrap();
    assert_eq!((first.entities, first.edges), (3, 3));
    assert_eq!(
        first,
        source
            .index_reference("memory", reference("a"))
            .await
            .unwrap()
    );
    let alias = query(&source, "ACME", GraphQuery::Entity).await;
    assert_eq!(alias.hits.len(), 1);
    assert_eq!(alias.hits[0].preview.as_ref(), "company: acme");
    let all = query(&source, "Tell me about Acme Corp", neighborhood(2)).await;
    assert_eq!(all.hits.len(), 3);
    assert_eq!(all.status, SourceStatus::Completed);
    assert!(all.hits.iter().all(|h| h.provenance.citations.len() == 1
        && h.provenance.citations[0].reference == reference("a")
        && h.provenance.scope == scope()));
    let path_query = GraphQuery::Path {
        target: "Gamma".into(),
        depth: 2,
        max_nodes: 100,
        max_edges: 1000,
    };
    let result = query(&source, "Acme", path_query.clone()).await;
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].preview.as_ref(), "company: gamma");
    assert_eq!(result.hits[0].provenance.locators.len(), 2);
    assert!(
        query(
            &source,
            "Acme",
            GraphQuery::Path {
                target: "Gamma".into(),
                depth: 1,
                max_nodes: 100,
                max_edges: 1000
            }
        )
        .await
        .hits
        .is_empty()
    );
    let reopened = open(&path, store);
    assert_eq!(result, query(&reopened, "Acme", path_query).await);
    finstack_ai_search_core::test_support::check_source_contract(
        &source,
        scope(),
        SearchQuery {
            strategy: SearchStrategy::Graph(GraphQuery::Entity),
            ..SearchQuery::lexical("Acme", LexicalKind::Literal)
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn corrections_forgetting_independent_support_and_fresh_sensitivity() {
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put("one".into(), record("a", "Acme owns Beta."))
        .await
        .unwrap();
    store
        .put("two".into(), record("b", "Acme owns Beta."))
        .await
        .unwrap();
    let source = open(Path::new(":memory:"), store.clone());
    source
        .index_reference("memory", reference("a"))
        .await
        .unwrap();
    source
        .index_reference("memory", reference("b"))
        .await
        .unwrap();
    assert_eq!(
        query(&source, "Acme", neighborhood(1)).await.hits[1]
            .provenance
            .citations
            .len(),
        2
    );
    let mut changed = record("c", "Acme owns Delta.");
    changed.sensitivity = Sensitivity::Secret;
    store
        .correct(
            "correction".into(),
            MemoryScope::try_new("tenant").unwrap(),
            MemoryId::parse("a").unwrap(),
            changed,
        )
        .await
        .unwrap();
    let stale = query(&source, "Beta", GraphQuery::Entity).await;
    assert_eq!(stale.status, SourceStatus::Truncated);
    assert_eq!(stale.hits.len(), 1);
    assert_eq!(stale.hits[0].provenance.citations.len(), 1);
    assert_eq!(
        stale.hits[0].provenance.citations[0].reference,
        reference("b")
    );
    source
        .index_reference("memory", reference("c"))
        .await
        .unwrap();
    let delta = query(&source, "Delta", GraphQuery::Entity).await;
    assert_eq!(delta.hits.len(), 1);
    assert_eq!(delta.hits[0].sensitivity, Sensitivity::Secret);
    store
        .forget(
            "forget".into(),
            MemoryScope::try_new("tenant").unwrap(),
            MemoryId::parse("b").unwrap(),
        )
        .await
        .unwrap();
    assert!(
        query(&source, "Beta", GraphQuery::Entity)
            .await
            .hits
            .is_empty()
    );
    let report = source.reconcile(None, 1).await.unwrap();
    assert_eq!(report.examined, 1);
    assert_eq!(report.unavailable, 0);
}

#[tokio::test]
async fn storage_query_authority_and_schema_limits_fail_closed() {
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put(
            "one".into(),
            record("a", "Acme owns Beta. Acme owns Delta. Beta owns Gamma."),
        )
        .await
        .unwrap();
    let mut config = GraphIndexConfig::new("graph", scope(), vocabulary());
    config.max_entities = 2;
    let source = GraphSearchSource::try_open(
        Path::new(":memory:"),
        config,
        vec![memory_source(store.clone())],
    )
    .unwrap();
    assert!(matches!(
        source.index_reference("memory", reference("a")).await,
        Err(SearchError::SearchCapacityExceeded { .. })
    ));
    assert!(
        query(&source, "Acme", GraphQuery::Entity)
            .await
            .hits
            .is_empty()
    );
    let source = open(Path::new(":memory:"), store.clone());
    source
        .index_reference("memory", reference("a"))
        .await
        .unwrap();
    let bounded = query(
        &source,
        "Acme",
        GraphQuery::Neighborhood {
            depth: 2,
            max_nodes: 2,
            max_edges: 1,
        },
    )
    .await;
    assert!(bounded.hits.len() <= 2);
    assert_eq!(bounded.status, SourceStatus::Truncated);
    let error = source
        .search(
            SearchScope::try_new("foreign").unwrap(),
            SearchQuery {
                strategy: SearchStrategy::Graph(GraphQuery::Entity),
                ..SearchQuery::lexical("Acme", LexicalKind::Literal)
            },
            10,
        )
        .await;
    assert_eq!(error, Err(SearchError::SearchScopeDenied));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.pragma_update(None, "user_version", 999).unwrap();
    drop(c);
    assert!(
        GraphSearchSource::try_open(
            &path,
            GraphIndexConfig::new("graph", scope(), vocabulary()),
            vec![memory_source(store.clone())]
        )
        .is_err()
    );
    let path = dir.path().join("foreign.sqlite");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute("CREATE TABLE foreign_data (id INTEGER)", [])
        .unwrap();
    drop(c);
    assert!(
        GraphSearchSource::try_open(
            &path,
            GraphIndexConfig::new("graph", scope(), vocabulary()),
            vec![memory_source(store)]
        )
        .is_err()
    );
}

#[tokio::test]
async fn changed_vocabulary_is_unservable_until_reindexed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph.sqlite");
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put("one".into(), record("a", "Acme owns Beta."))
        .await
        .unwrap();
    let original = open(&path, store.clone());
    original
        .index_reference("memory", reference("a"))
        .await
        .unwrap();
    drop(original);
    let mut config = GraphIndexConfig::new("graph", scope(), vocabulary());
    config.vocabulary.edge_rules.clear();
    let source = GraphSearchSource::try_open(&path, config, vec![memory_source(store)]).unwrap();
    let stale = query(&source, "Beta", GraphQuery::Entity).await;
    assert!(stale.hits.is_empty());
    assert_eq!(stale.status, SourceStatus::Truncated);
    source
        .index_reference("memory", reference("a"))
        .await
        .unwrap();
    assert!(
        query(&source, "Beta", GraphQuery::Entity)
            .await
            .hits
            .is_empty()
    );
    assert_eq!(query(&source, "Acme", neighborhood(2)).await.hits.len(), 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One real four-source federation and expansion lifecycle.
async fn one_query_retrieves_four_sources_with_opt_in_evidenced_expansion() {
    use finstack_ai_index_documents::{DocumentIndexConfig, DocumentInput, DocumentSearchSource};
    use finstack_ai_index_journal::{JournalIndexConfig, JournalSearchSource};
    use finstack_ai_kernel::{Metadata, SessionId};
    use finstack_ai_runtime::{
        Bytes,
        artifact::{ArtifactMetadata, ArtifactScope, ArtifactStore, InProcessArtifactStore},
    };
    use finstack_ai_search::{GraphExpansion, SearchConfig, SearchEngine, SearchRequest};
    let dir = tempfile::tempdir().unwrap();
    let memories = Arc::new(InProcessMemoryStore::new());
    let mut fact = record("a", "Acme owns Beta.");
    fact.sensitivity = Sensitivity::Secret;
    memories.put("fact".into(), fact).await.unwrap();
    let memory = memory_source(memories.clone());
    let artifacts = Arc::new(InProcessArtifactStore::default());
    let artifact_scope = ArtifactScope {
        tenant_scope: "tenant".into(),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: None,
        sensitivity: Sensitivity::Confidential,
    };
    let artifact = artifacts
        .stage_put(
            artifact_scope.clone(),
            Bytes::from_static(b"Beta owns Gamma."),
            ArtifactMetadata {
                kind: "document".into(),
                media_type: "text/plain".into(),
                name: Some("business register".into()),
                attributes: Metadata::empty(),
            },
        )
        .await
        .unwrap();
    let documents = Arc::new(
        DocumentSearchSource::try_open(
            &dir.path().join("docs.sqlite"),
            artifacts,
            DocumentIndexConfig::new("documents", scope()),
            None,
        )
        .unwrap(),
    );
    documents
        .index_document(DocumentInput {
            artifact_scope,
            artifact,
        })
        .await
        .unwrap();
    let store = Arc::new(
        finstack_ai_store_memory::MemoryJournalStore::try_new(
            finstack_ai_store_memory::MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 32,
                records_per_session: 128,
                snapshot_bytes: 1_048_576,
            },
        )
        .unwrap(),
    );
    let session = finstack_ai::Session::create(store.clone(), "tenant")
        .await
        .unwrap();
    session
        .lane("main")
        .await
        .unwrap()
        .append_text("Gamma owns Delta.")
        .await
        .unwrap();
    let journal = Arc::new(
        JournalSearchSource::try_open(
            &dir.path().join("journal.sqlite"),
            store,
            JournalIndexConfig::new("journal", scope(), vec![session.session_id()]),
        )
        .unwrap(),
    );
    journal
        .sync_session(session.session_id(), 128)
        .await
        .unwrap();
    let graph = Arc::new(
        GraphSearchSource::try_open(
            &dir.path().join("graph.sqlite"),
            GraphIndexConfig::new("graph", scope(), vocabulary()),
            vec![memory.clone(), documents.clone(), journal.clone()],
        )
        .unwrap(),
    );
    for (source, text) in [
        (memory.clone(), "Acme"),
        (documents.clone(), "Beta"),
        (journal.clone(), "Gamma"),
    ] {
        let results = source
            .search(scope(), SearchQuery::lexical(text, LexicalKind::Bm25), 8)
            .await
            .unwrap();
        assert!(!results.hits.is_empty());
        for hit in results.hits {
            graph
                .index_reference(&hit.source, hit.reference)
                .await
                .unwrap();
        }
    }
    let config = SearchConfig {
        scope: scope(),
        graph_expansion: None,
        default_plan: HybridPlan {
            legs: ["memory", "documents", "journal"]
                .into_iter()
                .map(|source| HybridLeg {
                    source: source.into(),
                    strategy: SearchStrategy::Lexical(LexicalKind::Bm25),
                    weight_micros: 1_000_000,
                })
                .collect(),
            fusion: Fusion::default(),
        },
        limits: SearchLimits::default(),
        max_concurrency: 3,
        source_timeout_ms: 2000,
    };
    let sources: Vec<Arc<dyn SearchSource>> = vec![memory, documents, journal, graph];
    let plain = SearchEngine::try_new(config.clone(), sources.clone())
        .unwrap()
        .search(SearchRequest::text("Acme"))
        .await
        .unwrap();
    assert_eq!(plain.outcomes.len(), 3);
    assert!(plain.hits.iter().all(|h| h.source.as_ref() == "memory"));
    let engine = SearchEngine::try_new(
        SearchConfig {
            graph_expansion: Some(GraphExpansion::new("graph")),
            ..config
        },
        sources,
    )
    .unwrap();
    let expanded = engine.search(SearchRequest::text("Acme")).await.unwrap();
    assert_eq!(expanded.outcomes.len(), 4);
    assert!(
        expanded
            .outcomes
            .iter()
            .all(|o| o.status == SourceStatus::Completed)
    );
    let source_names: std::collections::BTreeSet<_> =
        expanded.hits.iter().map(|h| h.source.as_ref()).collect();
    assert_eq!(
        source_names,
        std::collections::BTreeSet::from(["memory", "documents", "journal", "graph"])
    );
    for hit in expanded
        .hits
        .iter()
        .filter(|h| h.source.as_ref() != "graph")
    {
        assert_eq!(hit.sensitivity, Sensitivity::Secret);
        assert!(hit.evidence.iter().all(|e| {
            e.provenance.scope == scope()
                && e.provenance
                    .citations
                    .iter()
                    .any(|c| c.source.as_ref() == "memory")
        }));
    }
    // Source selection and exact literal semantics do not perform expansion.
    let selected = engine
        .search(SearchRequest {
            sources: Some(vec!["documents".into()]),
            ..SearchRequest::text("Acme")
        })
        .await
        .unwrap();
    assert!(selected.hits.is_empty());
    assert_eq!(selected.outcomes.len(), 1);
    let literal = engine
        .search(SearchRequest {
            strategy: Some(SearchStrategy::Lexical(LexicalKind::Literal)),
            ..SearchRequest::text("Acme")
        })
        .await
        .unwrap();
    assert_eq!(literal.outcomes.len(), 3);
    assert!(literal.hits.iter().all(|h| h.source.as_ref() == "memory"));
    memories
        .forget(
            "forget".into(),
            MemoryScope::try_new("tenant").unwrap(),
            MemoryId::parse("a").unwrap(),
        )
        .await
        .unwrap();
    let gone = engine.search(SearchRequest::text("Acme")).await.unwrap();
    assert!(gone.hits.is_empty());
    assert!(
        gone.outcomes
            .iter()
            .any(|o| o.source.as_ref() == "graph" && o.status == SourceStatus::Truncated)
    );
}

#[derive(Clone)]
struct EvidenceFault {
    inner: Arc<dyn SearchSource>,
    mode: Arc<std::sync::atomic::AtomicU8>,
}
impl SearchSource for EvidenceFault {
    fn descriptor(&self) -> SearchSourceDescriptor {
        self.inner.descriptor()
    }
    fn authorize(&self, scope: &SearchScope, query: &SearchQuery) -> Result<(), SearchError> {
        self.inner.authorize(scope, query)
    }
    fn search(
        &self,
        scope: SearchScope,
        query: SearchQuery,
        limit: usize,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<SourceResult, SearchError>> {
        self.inner.search(scope, query, limit)
    }
    fn read_evidence(
        &self,
        scope: SearchScope,
        reference: SourceRef,
    ) -> finstack_ai_runtime::ports::PortFuture<Result<Option<SearchEvidence>, SearchError>> {
        let this = self.clone();
        Box::pin(async move {
            let mode = this.mode.load(std::sync::atomic::Ordering::SeqCst);
            if mode == 1 {
                return Err(SearchError::SearchUnavailable);
            }
            let mut result = this.inner.read_evidence(scope, reference).await?;
            if mode == 2
                && let Some(evidence) = &mut result
            {
                evidence.hit.sensitivity = Sensitivity::Secret;
            }
            Ok(result)
        })
    }
}

#[tokio::test]
async fn source_failures_fresh_classification_and_evidence_read_bounds_remain_visible() {
    use std::sync::atomic::{AtomicU8, Ordering};
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put("one".into(), record("a", "Acme owns Beta."))
        .await
        .unwrap();
    store
        .put("two".into(), record("b", "Beta owns Gamma."))
        .await
        .unwrap();
    let mode = Arc::new(AtomicU8::new(0));
    let fault = Arc::new(EvidenceFault {
        inner: memory_source(store),
        mode: mode.clone(),
    });
    let mut config = GraphIndexConfig::new("graph", scope(), vocabulary());
    config.max_evidence_reads = 1;
    let source = GraphSearchSource::try_open(Path::new(":memory:"), config, vec![fault]).unwrap();
    source
        .index_reference("memory", reference("a"))
        .await
        .unwrap();
    source
        .index_reference("memory", reference("b"))
        .await
        .unwrap();
    mode.store(1, Ordering::SeqCst);
    let failed = query(&source, "Acme", GraphQuery::Entity).await;
    assert_eq!(failed.status, SourceStatus::Unavailable);
    assert!(failed.hits.is_empty());
    let maintenance = source.reconcile(None, 1).await.unwrap();
    assert_eq!(maintenance.unavailable, 1);
    assert!(maintenance.next_cursor.is_some());
    mode.store(2, Ordering::SeqCst);
    let restored = query(&source, "Acme", GraphQuery::Entity).await;
    assert_eq!(restored.status, SourceStatus::Completed);
    assert_eq!(restored.hits[0].sensitivity, Sensitivity::Secret);
    let bounded = query(&source, "Acme", neighborhood(2)).await;
    assert_eq!(bounded.status, SourceStatus::Truncated);
    assert!(
        bounded
            .reasons
            .iter()
            .any(|r| r.as_ref() == "graph_evidence_read_limit")
    );
    assert!(
        bounded
            .hits
            .iter()
            .all(|h| h.preview.as_ref() != "company: gamma")
    );
}

#[tokio::test]
async fn version_one_migrates_ordered_indexes_without_changing_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("migration.sqlite");
    let store = Arc::new(InProcessMemoryStore::new());
    store
        .put(
            "seed".into(),
            record("a", "Acme Corp owns Beta. Beta owns Gamma."),
        )
        .await
        .unwrap();
    let source = memory_source(store);
    let config = GraphIndexConfig::new("graph", scope(), vocabulary());
    let graph = GraphSearchSource::try_open(&path, config.clone(), vec![source.clone()]).unwrap();
    graph
        .index_reference("memory", reference("a"))
        .await
        .unwrap();
    let expected = query(&graph, "Acme", neighborhood(2)).await;
    drop(graph);
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        // The v1 tables and records are identical; only v2's additive indexes differ.
        db.execute_batch("DROP INDEX entities_scope_id; DROP INDEX aliases_entity_alias; UPDATE graph_index_meta SET kind='finstack.graph-index.v1'; PRAGMA user_version=1;").unwrap();
    }
    let migrated =
        GraphSearchSource::try_open(&path, config.clone(), vec![source.clone()]).unwrap();
    assert_eq!(query(&migrated, "Acme", neighborhood(2)).await, expected);
    drop(migrated);
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.pragma_query_value::<u32, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            2
        );
        assert_eq!(db.query_row::<u32, _, _>("SELECT count(*) FROM sqlite_master WHERE name IN ('entities_scope_id','aliases_entity_alias')", [], |r| r.get(0)).unwrap(), 2);
    }
    let reopened = GraphSearchSource::try_open(&path, config, vec![source]).unwrap();
    assert_eq!(query(&reopened, "Acme", neighborhood(2)).await, expected);
}
