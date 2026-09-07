//! Real `SQLite` document indexing, retrieval, liveness, and reconstruction.

use std::sync::Arc;

use finstack_ai_embeddings::embedder::{HashEmbedder, TextEmbedder};
use finstack_ai_index_documents::*;
use finstack_ai_kernel::{Metadata, RunId, Sensitivity, SessionId, UNIX_EPOCH};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, ArtifactStoreLimits, InProcessArtifactStore,
};
use finstack_ai_search_core::test_support::check_source_contract;
use finstack_ai_search_core::*;

fn scope() -> SearchScope {
    SearchScope::try_new("tenant").unwrap()
}

async fn stage(store: &dyn ArtifactStore, bytes: &[u8], media: &str) -> DocumentInput {
    let artifact_scope = ArtifactScope {
        tenant_scope: "tenant".into(),
        session_id: SessionId::from_bytes([1; 16]),
        run_id: Some(RunId::from_bytes([2; 16])),
        sensitivity: Sensitivity::Confidential,
    };
    let artifact = store
        .stage_put(
            artifact_scope.clone(),
            Bytes::copy_from_slice(bytes),
            ArtifactMetadata {
                kind: "document".into(),
                media_type: media.into(),
                name: Some("handbook".into()),
                attributes: Metadata::empty(),
            },
        )
        .await
        .unwrap();
    DocumentInput {
        artifact_scope,
        artifact,
    }
}

#[test]
fn unicode_chunking_preserves_offsets_overlap_and_versioned_boundaries() {
    let text = format!(
        "# First\n\n{}\n\n# Second\n\n{}",
        "é🦀".repeat(2700),
        "retention\n\n".repeat(600)
    );
    let config = ChunkerConfig::default();
    let chunks = chunk_document(&text, config, 100).unwrap();
    let chars: Vec<_> = text.chars().collect();
    let mut covered = 0;
    for chunk in &chunks {
        let start = chunk.start_char as usize;
        let end = chunk.end_char as usize;
        assert!(start <= covered && covered - start <= 512);
        assert!(end > covered);
        assert_eq!(
            chunk.text.as_ref(),
            chars[start..end].iter().collect::<String>()
        );
        assert!(end - start <= 4096);
        covered = end;
    }
    assert_eq!(covered, chars.len());
    assert!(
        chunks
            .iter()
            .any(|chunk| chunk.heading.as_deref() == Some("# Second"))
    );
    assert_ne!(
        config.digest().unwrap(),
        ChunkerConfig {
            overlap_chars: 0,
            ..config
        }
        .digest()
        .unwrap()
    );
    assert!(
        ChunkerConfig {
            version: 2,
            ..config
        }
        .validate()
        .is_err()
    );
    assert!(chunk_document(&text, config, 1).is_err());
}

#[tokio::test]
async fn indexing_is_atomic_idempotent_and_rebuilds_identical_citations() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    let input = stage(
        store.as_ref(),
        b"# Retention\n\nKeep records seven years.\n",
        "text/markdown",
    )
    .await;
    let config = DocumentIndexConfig::new("documents", scope());
    let path = dir.path().join("index.sqlite");
    let source =
        DocumentSearchSource::try_open(&path, Arc::clone(&store), config.clone(), None).unwrap();
    let (first, second) = tokio::join!(
        source.index_document(input.clone()),
        source.index_document(input.clone())
    );
    assert_eq!(first.unwrap(), second.unwrap());
    let query = SearchQuery::lexical("records", LexicalKind::Bm25);
    check_source_contract(&source, scope(), query.clone())
        .await
        .unwrap();
    let before = source.search(scope(), query.clone(), 8).await.unwrap();
    assert_eq!(before.hits.len(), 1);
    assert_eq!(before.hits[0].sensitivity, Sensitivity::Confidential);
    assert!(
        before.hits[0]
            .provenance
            .locators
            .iter()
            .any(|l| l.starts_with("parsed_markdown_characters:"))
    );
    assert_eq!(source.inputs(None, 8).await.unwrap().len(), 1);
    drop(source);
    let reopened =
        DocumentSearchSource::try_open(&path, Arc::clone(&store), config.clone(), None).unwrap();
    assert_eq!(
        reopened.search(scope(), query.clone(), 8).await.unwrap(),
        before
    );
    let rebuilt =
        DocumentSearchSource::try_open(&dir.path().join("rebuilt.sqlite"), store, config, None)
            .unwrap();
    rebuilt.index_document(input).await.unwrap();
    assert_eq!(rebuilt.search(scope(), query, 8).await.unwrap(), before);
}

#[tokio::test]
async fn lexical_semantic_scope_and_source_deletion_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        InProcessArtifactStore::default().with_limits(ArtifactStoreLimits {
            orphan_grace_ms: 0,
            ..ArtifactStoreLimits::default()
        }),
    );
    let input = stage(store.as_ref(), b"retention seven years", "text/plain").await;
    let embedder: Arc<dyn TextEmbedder> = Arc::new(HashEmbedder::try_new(64).unwrap());
    let space = embedder.descriptor().embedder_id;
    let source = DocumentSearchSource::try_open(
        &dir.path().join("index.sqlite"),
        store.clone(),
        DocumentIndexConfig::new("documents", scope()),
        Some(embedder),
    )
    .unwrap();
    source.index_document(input.clone()).await.unwrap();
    for kind in [
        LexicalKind::Keyword,
        LexicalKind::Bm25,
        LexicalKind::Literal,
        LexicalKind::Regex,
    ] {
        let query = SearchQuery::lexical("retention", kind);
        assert_eq!(
            source
                .search(scope(), query.clone(), 8)
                .await
                .unwrap()
                .hits
                .len(),
            1
        );
        assert_eq!(
            source
                .search(SearchScope::try_new("other").unwrap(), query, 8)
                .await,
            Err(SearchError::SearchScopeDenied)
        );
    }
    let semantic = SearchQuery {
        text: "retention".into(),
        strategy: SearchStrategy::Semantic { space },
        journal: JournalFilter::default(),
    };
    assert_eq!(
        source
            .search(scope(), semantic.clone(), 8)
            .await
            .unwrap()
            .status,
        SourceStatus::Unavailable
    );
    assert_eq!(source.reconcile_embeddings(8).await.unwrap(), 1);
    assert_eq!(source.reconcile_embeddings(8).await.unwrap(), 0);
    assert_eq!(
        source
            .search(scope(), semantic.clone(), 8)
            .await
            .unwrap()
            .status,
        SourceStatus::Completed
    );
    store
        .collect_orphans(input.artifact_scope.clone(), UNIX_EPOCH, 8)
        .await
        .unwrap();
    assert_eq!(
        store
            .collect_orphans(input.artifact_scope, UNIX_EPOCH, 8)
            .await
            .unwrap()
            .deleted,
        1
    );
    let deleted = source.search(scope(), semantic, 8).await.unwrap();
    assert!(deleted.hits.is_empty());
    assert!(
        deleted
            .reasons
            .iter()
            .any(|r| r.as_ref() == "document_sources_removed")
    );
    assert!(source.inputs(None, 8).await.unwrap().is_empty());
    let db = rusqlite::Connection::open(dir.path().join("index.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM chunk_vectors", [], |r| r
            .get::<_, usize>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn scanned_documents_preserve_ocr_required_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    let input = stage(
        store.as_ref(),
        include_bytes!("../../../../fixtures/documents/scanned.pdf"),
        "application/pdf",
    )
    .await;
    let source = DocumentSearchSource::try_open(
        &dir.path().join("index.sqlite"),
        store,
        DocumentIndexConfig::new("documents", scope()),
        None,
    )
    .unwrap();
    let report = source.index_document(input).await.unwrap();
    assert!(report.requires_ocr);
    assert_eq!(report.status, DocumentStatus::OcrRequired);
    let result = source
        .search(
            scope(),
            SearchQuery::lexical("retention", LexicalKind::Keyword),
            8,
        )
        .await
        .unwrap();
    assert_ne!(result.status, SourceStatus::Completed);
    assert!(
        result
            .reasons
            .iter()
            .any(|r| r.as_ref() == "document_extraction_incomplete")
    );
}

#[tokio::test]
async fn schema_ownership_and_failed_ingestion_do_not_mutate_index() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    for (name, ddl) in [
        ("future", "PRAGMA user_version=99"),
        ("foreign", "CREATE TABLE external(value TEXT)"),
    ] {
        let path = dir.path().join(name);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(ddl)
            .unwrap();
        assert!(
            DocumentSearchSource::try_open(
                &path,
                store.clone(),
                DocumentIndexConfig::new("documents", scope()),
                None
            )
            .is_err()
        );
    }
    let mut config = DocumentIndexConfig::new("documents", scope());
    config.max_chunks = 1;
    config.chunker.target_chars = 64;
    config.chunker.overlap_chars = 0;
    let path = dir.path().join("index.sqlite");
    let source =
        DocumentSearchSource::try_open(&path, store.clone(), config.clone(), None).unwrap();
    let invalid = stage(store.as_ref(), &[255], "text/plain").await;
    assert!(source.index_document(invalid).await.is_err());
    let oversized = stage(
        store.as_ref(),
        "retention ".repeat(20).as_bytes(),
        "text/plain",
    )
    .await;
    assert!(matches!(
        source.index_document(oversized).await,
        Err(SearchError::SearchCapacityExceeded { .. })
    ));
    assert!(source.inputs(None, 8).await.unwrap().is_empty());
    let input = stage(store.as_ref(), b"retention", "text/plain").await;
    let second = DocumentSearchSource::try_open(&path, store, config, None).unwrap();
    let (a, b) = tokio::join!(
        source.index_document(input.clone()),
        second.index_document(input)
    );
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(source.inputs(None, 8).await.unwrap().len(), 1);
}

#[tokio::test]
async fn processing_changes_preserve_catalog_and_require_reindexing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("artifacts");
    let store: Arc<dyn ArtifactStore> =
        Arc::new(finstack_ai_store_artifact::LocalArtifactStore::try_new(root.clone()).unwrap());
    let input = stage(store.as_ref(), b"retention seven years", "text/plain").await;
    let path = dir.path().join("index.sqlite");
    let config = DocumentIndexConfig::new("documents", scope());
    let source = DocumentSearchSource::try_open(&path, store, config.clone(), None).unwrap();
    let original = source.index_document(input.clone()).await.unwrap();
    drop(source);
    let store: Arc<dyn ArtifactStore> =
        Arc::new(finstack_ai_store_artifact::LocalArtifactStore::try_new(root).unwrap());
    let mut changed = config;
    changed.parse_limits.max_output_bytes = 9;
    let source =
        DocumentSearchSource::try_open(&path, store.clone(), changed.clone(), None).unwrap();
    assert_eq!(source.inputs(None, 8).await.unwrap()[0].1, input);
    let query = SearchQuery::lexical("retention", LexicalKind::Keyword);
    let pending = source.search(scope(), query.clone(), 8).await.unwrap();
    assert_eq!(pending.status, SourceStatus::Unavailable);
    let current = source.index_document(input.clone()).await.unwrap();
    assert_ne!(original.chunker_digest, current.chunker_digest);
    assert!(current.truncated);
    let before = source.search(scope(), query.clone(), 8).await.unwrap();
    assert_eq!(before.hits.len(), 1);
    assert!(
        !before
            .reasons
            .iter()
            .any(|r| r.as_ref() == "document_chunker_reindex_pending")
    );
    let rebuilt =
        DocumentSearchSource::try_open(&dir.path().join("rebuilt.sqlite"), store, changed, None)
            .unwrap();
    rebuilt.index_document(input).await.unwrap();
    assert_eq!(rebuilt.search(scope(), query, 8).await.unwrap(), before);
}

#[tokio::test]
async fn scan_preview_vector_and_result_capacities_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    let mut config = DocumentIndexConfig::new("documents", scope());
    config.chunker.target_chars = 64;
    config.chunker.overlap_chars = 0;
    config.limits.max_scan_records = 1;
    config.limits.max_results = 1;
    config.limits.max_preview_chars = 3;
    config.limits.max_embedding_bytes = 255;
    let source = DocumentSearchSource::try_open(
        &dir.path().join("index.sqlite"),
        store.clone(),
        config,
        Some(Arc::new(HashEmbedder::try_new(64).unwrap())),
    )
    .unwrap();
    let input = stage(
        store.as_ref(),
        "retention 🦀 ".repeat(20).as_bytes(),
        "text/plain",
    )
    .await;
    assert!(source.index_document(input).await.unwrap().chunks > 1);
    for kind in [
        LexicalKind::Keyword,
        LexicalKind::Bm25,
        LexicalKind::Literal,
        LexicalKind::Regex,
    ] {
        let result = source
            .search(scope(), SearchQuery::lexical("retention", kind), 1)
            .await
            .unwrap();
        assert_eq!(result.examined, 1);
        assert_eq!(result.status, SourceStatus::Truncated);
        assert!(result.hits.len() <= 1);
        assert!(
            result
                .hits
                .iter()
                .all(|hit| hit.preview.chars().count() <= 3)
        );
    }
    assert!(
        source
            .search(
                scope(),
                SearchQuery::lexical("retention", LexicalKind::Keyword),
                2
            )
            .await
            .is_err()
    );
    assert!(matches!(
        source.reconcile_embeddings(1).await,
        Err(SearchError::SearchCapacityExceeded { .. })
    ));
    let db = rusqlite::Connection::open(dir.path().join("index.sqlite")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM chunk_vectors", [], |r| r
            .get::<_, usize>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn existing_pdf_and_office_parsers_produce_citable_text() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    let source = DocumentSearchSource::try_open(
        &dir.path().join("index.sqlite"),
        store.clone(),
        DocumentIndexConfig::new("documents", scope()),
        None,
    )
    .unwrap();
    for (bytes, media) in [
        (
            include_bytes!("../../../../fixtures/documents/text.pdf").as_slice(),
            "application/pdf",
        ),
        (
            include_bytes!("../../../../fixtures/documents/sample.docx").as_slice(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
    ] {
        let report = source
            .index_document(stage(store.as_ref(), bytes, media).await)
            .await
            .unwrap();
        assert!(!report.requires_ocr);
        assert!(report.chunks > 0);
    }
    let results = source
        .search(scope(), SearchQuery::lexical(".", LexicalKind::Regex), 8)
        .await
        .unwrap();
    assert_eq!(results.hits.len(), 2);
    assert!(
        results
            .hits
            .iter()
            .all(|hit| !hit.provenance.locators.is_empty())
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Both real systems of record and their fused citation proof.
async fn federated_memory_and_documents_preserve_each_citation() {
    use finstack_ai_memory::{
        record::*,
        search::MemorySearchSource,
        store::{InProcessMemoryStore, MemoryStore},
    };
    use finstack_ai_search::{SearchConfig, SearchEngine, SearchRequest};
    let dir = tempfile::tempdir().unwrap();
    let artifacts: Arc<dyn ArtifactStore> = Arc::new(InProcessArtifactStore::default());
    let documents = Arc::new(
        DocumentSearchSource::try_open(
            &dir.path().join("index.sqlite"),
            artifacts.clone(),
            DocumentIndexConfig::new("documents", scope()),
            None,
        )
        .unwrap(),
    );
    let input = stage(artifacts.as_ref(), b"retention seven years", "text/plain").await;
    documents.index_document(input.clone()).await.unwrap();
    let memory: Arc<dyn MemoryStore> = Arc::new(InProcessMemoryStore::default());
    memory
        .put(
            "write".into(),
            MemoryRecord {
                id: MemoryId::parse("retention").unwrap(),
                scope: MemoryScope::try_new("tenant").unwrap(),
                keywords: vec![Arc::from("retention")].into(),
                body: MemoryBody::Inline("retention seven years".into()),
                preview: "retention seven years".into(),
                sensitivity: Sensitivity::Secret,
                provenance: MemoryProvenance {
                    source_session: None,
                    source_run: None,
                    source_ref: Some("handbook".into()),
                    extraction: ExtractionMethod::Explicit,
                    confidence: 100,
                },
                created_at: UNIX_EPOCH,
                last_confirmed_at: UNIX_EPOCH,
                supersedes: None,
                superseded_by: None,
                retention: RetentionPolicy::KeepUntilDeleted,
                tombstoned: false,
            },
        )
        .await
        .unwrap();
    let memory = Arc::new(
        MemorySearchSource::try_new(
            "memory",
            memory,
            scope(),
            ScopeMapping::Exact,
            SearchLimits::default(),
            None,
        )
        .unwrap(),
    );
    let engine = SearchEngine::try_new(
        SearchConfig {
            graph_expansion: None,
            scope: scope(),
            default_plan: HybridPlan {
                legs: ["documents", "memory"]
                    .into_iter()
                    .map(|source| HybridLeg {
                        source: source.into(),
                        strategy: SearchStrategy::Lexical(LexicalKind::Keyword),
                        weight_micros: 1_000_000,
                    })
                    .collect(),
                fusion: Fusion::default(),
            },
            limits: SearchLimits::default(),
            max_concurrency: 2,
            source_timeout_ms: 1000,
        },
        vec![documents, memory],
    )
    .unwrap();
    let result = engine
        .search(SearchRequest::text("retention"))
        .await
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert!(
        result
            .outcomes
            .iter()
            .all(|o| o.status == SourceStatus::Completed)
    );
    let artifact_hit = result
        .hits
        .iter()
        .find(|hit| hit.source.as_ref() == "documents")
        .unwrap();
    assert!(
        matches!(&artifact_hit.reference,SourceRef::ArtifactChunk { artifact,.. } if artifact.as_ref()==&input.artifact)
    );
    let memory_hit = result
        .hits
        .iter()
        .find(|hit| hit.source.as_ref() == "memory")
        .unwrap();
    assert_eq!(memory_hit.sensitivity, Sensitivity::Secret);
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.evidence.iter().all(|e| e.provenance.scope == scope()))
    );
}
