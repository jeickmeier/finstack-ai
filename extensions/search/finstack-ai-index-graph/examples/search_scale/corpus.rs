use super::Error;
use finstack_ai_index_documents::{DocumentInput, DocumentSearchSource, DocumentStatus};
use finstack_ai_index_graph::{EdgeKind, EdgeRule, EntityRule, GraphVocabulary};
use finstack_ai_index_journal::{JournalIndexConfig, JournalSearchSource};
use finstack_ai_kernel::{Metadata, Sensitivity, SessionId, UNIX_EPOCH};
use finstack_ai_memory::{
    record::{
        ExtractionMethod, MemoryBody, MemoryId, MemoryProvenance, MemoryRecord, MemoryScope,
        RetentionPolicy,
    },
    store::{MemoryStore, SqliteMemoryStore},
};
use finstack_ai_runtime::artifact::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, stage_required_artifact,
};
use finstack_ai_search_core::SearchScope;
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
};
use std::fmt::Write as _;
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Instant};

pub async fn memory(store: &SqliteMemoryStore, count: usize) -> Result<(), Error> {
    eprintln!("building {count} real memory records");
    for index in 0..count {
        let text = format!(
            "Company{index:05} owns Company{:05}. sector{:03} {}",
            (index + 1) % count,
            index % 100,
            "Operations revenue cash flow covenant retention. ".repeat(10)
        );
        store
            .put(
                format!("memory-{index:05}").into(),
                MemoryRecord {
                    id: MemoryId::parse(&format!("memory-{index:05}"))?,
                    scope: MemoryScope::try_new("search-benchmark")?,
                    keywords: vec![format!("sector{:03}", index % 100).into()].into(),
                    body: MemoryBody::Inline(text.clone().into()),
                    preview: text.chars().take(256).collect::<String>().into(),
                    sensitivity: Sensitivity::Internal,
                    provenance: MemoryProvenance {
                        source_session: None,
                        source_run: None,
                        source_ref: Some("deterministic-benchmark-v1".into()),
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
            .await?;
    }
    Ok(())
}

pub async fn documents(
    artifacts: &dyn ArtifactStore,
    index: &DocumentSearchSource,
    count: usize,
) -> Result<Vec<DocumentInput>, Error> {
    let mut inputs = Vec::new();
    let mut chunks = 0;
    for document in 0..count / 100 {
        let mut text = String::new();
        for section in 0..100 {
            // Each heading ends the previous section under the default chunker.
            // These are full-sized paragraphs, not tiny synthetic database rows.
            write!(
                text,
                "# Register {document:04} section {section:03}\n\nsector{section:03} "
            )?;
            text.push_str(
                &"Revenue cash flow operations inventories annual covenant retention. ".repeat(55),
            );
            text.push_str("\n\n");
        }
        let artifact_scope = ArtifactScope {
            tenant_scope: "search-benchmark".into(),
            session_id: SessionId::from_bytes([1; 16]),
            run_id: None,
            sensitivity: Sensitivity::Internal,
        };
        let artifact = stage_required_artifact(
            artifacts,
            artifact_scope.clone(),
            text.into_bytes().into(),
            ArtifactMetadata {
                kind: "document".into(),
                media_type: "text/markdown".into(),
                name: Some(format!("register-{document:04}.md").into()),
                attributes: Metadata::empty(),
            },
        )
        .await?;
        let input = DocumentInput {
            artifact_scope,
            artifact,
        };
        let receipt = index.index_document(input.clone()).await?;
        if receipt.status != DocumentStatus::Indexed || receipt.chunks != 100 {
            return Err(format!(
                "document produced {} chunks with {:?}",
                receipt.chunks, receipt.status
            )
            .into());
        }
        chunks += receipt.chunks;
        inputs.push(input);
        if document.is_multiple_of(100) {
            eprintln!("indexed {chunks}/{count} document chunks");
        }
    }
    if chunks != count {
        return Err("chunk count mismatch".into());
    }
    Ok(inputs)
}

pub fn vocabulary() -> GraphVocabulary {
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
            pattern: "(?P<label>Company[0-9]{5})".into(),
            canonical_label: None,
        }],
        edge_rules: vec![EdgeRule {
            kind: "owns".into(),
            pattern: "(?P<source>Company[0-9]{5}) owns (?P<target>Company[0-9]{5})".into(),
        }],
    }
}

pub async fn journal(
    root: &Path,
    scope: &SearchScope,
    count: usize,
    reuse: bool,
) -> Result<(JournalSearchSource, BTreeMap<&'static str, f64>), Error> {
    let store = Arc::new(SqliteJournalStore::try_open(SqliteStoreConfig::new(
        root.join("journal.sqlite"),
        SqliteDurability::Durable,
        SqliteStoreLimits {
            sessions: 4,
            batches_per_session: 30_000,
            records_per_session: 100_000,
            snapshot_bytes: 1_048_576,
        },
    ))?);
    let mut timings = BTreeMap::new();
    let id: SessionId = if reuse {
        serde_json::from_slice(&std::fs::read(root.join("journal-session.json"))?)?
    } else {
        let start = Instant::now();
        let session = finstack_ai::Session::create(store.clone(), "search-benchmark").await?;
        let lane = session.lane("main").await?;
        for i in 0..count {
            lane.append_text(&format!(
                "sector{:03} committed journal note {i:05}: retention covenant report",
                i % 100
            ))
            .await?;
        }
        let id = session.session_id();
        std::fs::write(root.join("journal-session.json"), serde_json::to_vec(&id)?)?;
        timings.insert("journal_fixture_ms", start.elapsed().as_secs_f64() * 1000.0);
        id
    };
    let config = JournalIndexConfig::new("journal", scope.clone(), vec![id]);
    let source = JournalSearchSource::try_open(
        &root.join("journal-index.sqlite"),
        store.clone(),
        config.clone(),
    )?;
    if !reuse {
        let mut samples = Vec::new();
        for repeat in 0..5 {
            let candidate = JournalSearchSource::try_open(
                &root.join(format!("journal-rebuilt-{repeat}.sqlite")),
                store.clone(),
                config.clone(),
            )?;
            let start = Instant::now();
            let mut indexed = 0;
            loop {
                let page = candidate.sync_session(id, 256).await?;
                if !page.historical_complete {
                    return Err("journal historical coverage lost".into());
                }
                indexed += page.indexed;
                if page.complete {
                    break;
                }
            }
            if indexed != count {
                return Err(format!("journal indexed {indexed}, expected {count}").into());
            }
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        timings.insert("journal_backfill_p50_ms", samples[2]);
        timings.insert("journal_backfill_p95_ms", samples[4]);
        timings.insert("journal_backfill_samples", 5.0);
        loop {
            if source.sync_session(id, 256).await?.complete {
                break;
            }
        }
    }
    Ok((source, timings))
}
