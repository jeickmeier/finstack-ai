//! Application-owned search composition and explicit bounded index maintenance.
use crate::{KnowledgeConfig, KnowledgeError, config::compose_error};
use finstack_ai::runtime::{artifact::ArtifactStore, ports::journal::JournalStore};
use finstack_ai_embeddings::embedder::TextEmbedder;
use finstack_ai_index_documents::{DocumentIndexConfig, DocumentSearchSource};
use finstack_ai_index_graph::{GraphIndexConfig, GraphSearchSource};
use finstack_ai_index_journal::{JournalIndexConfig, JournalIndexObserver, JournalSearchSource};
use finstack_ai_memory::{search::MemorySearchSource, store::MemoryStore};
use finstack_ai_search::{
    GraphExpansion, HybridLeg, HybridPlan, LexicalKind, ScopeMapping, SearchConfig, SearchEngine,
    SearchLimits, SearchScope, SearchSource, SearchStrategy,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Bounded host maintenance outcome. Search queries separately preserve every
/// unavailable, pending, stale or historically incomplete source outcome.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SearchMaintenanceReport {
    /// Committed journal envelopes examined.
    pub journal_records: usize,
    /// Live references re-extracted into the optional graph.
    pub graph_references: usize,
    /// Exact sources removed from derived document/graph indexes.
    pub removed: usize,
    /// Explicitly refreshed vectors (zero without an embedder).
    pub embeddings: usize,
    /// A bounded page left additional work for the next maintenance call.
    pub more: bool,
    /// Stable source failure codes, without paths, content or credentials.
    pub failures: Vec<String>,
}

/// Search handles belonging to this knowledge composition. No task is spawned;
/// the host calls `maintain` between runs and drops the handles on shutdown.
pub struct KnowledgeSearch {
    /// One federated query engine used by the search tool and global recall.
    pub engine: Arc<SearchEngine>,
    /// Existing scoped memory source.
    pub memory: Arc<MemorySearchSource>,
    /// Derived document index over the agent's artifact store.
    pub documents: Arc<DocumentSearchSource>,
    /// Derived committed journal index over explicit sessions.
    pub journal: Arc<JournalSearchSource>,
    /// Optional graph; absent unless a vocabulary was configured.
    pub graph: Option<Arc<GraphSearchSource>>,
    /// Read-only hints drained by host maintenance.
    pub observer: Arc<JournalIndexObserver>,
    build_cursor: Option<finstack_ai_index_graph::GraphBuildCursor>,
    document_cursor: Option<String>,
    graph_cursor: Option<String>,
    memory_offset: usize,
    journal_offset: usize,
    embedded: bool,
}
impl KnowledgeSearch {
    pub(crate) fn open(
        config: &KnowledgeConfig,
        store: Arc<dyn MemoryStore>,
        embedder: Option<&Arc<dyn TextEmbedder>>,
        journal: Arc<dyn JournalStore>,
        artifacts: Arc<dyn ArtifactStore>,
    ) -> Result<Self, KnowledgeError> {
        let scope = SearchScope::try_new("local").map_err(compose_error)?;
        let memory = Arc::new(
            MemorySearchSource::try_new(
                "memory",
                store,
                scope.clone(),
                ScopeMapping::Exact,
                SearchLimits::default(),
                embedder.cloned(),
            )
            .map_err(compose_error)?,
        );
        let documents = Arc::new(
            DocumentSearchSource::try_open(
                &config.data_dir.join("documents.sqlite3"),
                artifacts,
                DocumentIndexConfig::new("documents", scope.clone()),
                embedder.cloned(),
            )
            .map_err(compose_error)?,
        );
        let journal = Arc::new(
            JournalSearchSource::try_open(
                &config.data_dir.join("journal-index.sqlite3"),
                journal,
                JournalIndexConfig::new("journal", scope.clone(), config.search_sessions.clone()),
            )
            .map_err(compose_error)?,
        );
        let observer =
            Arc::new(JournalIndexObserver::try_new(journal.clone()).map_err(compose_error)?);
        let sources: Vec<Arc<dyn SearchSource>> =
            vec![memory.clone(), documents.clone(), journal.clone()];
        let graph = config
            .graph_vocabulary
            .as_ref()
            .map(|v| {
                GraphSearchSource::try_open(
                    &config.data_dir.join("graph.sqlite3"),
                    GraphIndexConfig::new("graph", scope.clone(), v.clone()),
                    sources.clone(),
                )
                .map(Arc::new)
            })
            .transpose()
            .map_err(compose_error)?;
        let mut legs: Vec<_> = sources
            .iter()
            .map(|source| HybridLeg {
                source: source.descriptor().source_id,
                strategy: SearchStrategy::Lexical(LexicalKind::Bm25),
                weight_micros: 1_000_000,
            })
            .collect();
        if let Some(embedder) = &embedder {
            for id in ["memory", "documents"] {
                legs.push(HybridLeg {
                    source: id.into(),
                    strategy: SearchStrategy::Semantic {
                        space: embedder.descriptor().embedder_id,
                    },
                    weight_micros: 1_000_000,
                });
            }
        }
        let mut all = sources.clone();
        if let Some(graph) = &graph {
            all.push(graph.clone());
        }
        let engine = Arc::new(
            SearchEngine::try_new(
                SearchConfig {
                    scope,
                    default_plan: HybridPlan {
                        legs,
                        fusion: finstack_ai_search::Fusion::default(),
                    },
                    graph_expansion: graph.as_ref().map(|_| GraphExpansion::new("graph")),
                    limits: SearchLimits::default(),
                    max_concurrency: 4,
                    source_timeout_ms: 10_000,
                },
                all,
            )
            .map_err(compose_error)?,
        );
        Ok(Self {
            engine,
            memory,
            documents,
            journal,
            graph,
            observer,
            build_cursor: None,
            document_cursor: None,
            graph_cursor: None,
            memory_offset: 0,
            journal_offset: 0,
            embedded: embedder.is_some(),
        })
    }

    /// Advance bounded maintenance between turns. Each source catalog contributes
    /// at most `limit` references; journal backfill consumes at most `limit`
    /// records plus a same-sized hint drain. Limits are 1..=256. Source failures
    /// are returned in the report, never interpreted as empty successful work.
    /// Embedding egress occurs only when an embedder was explicitly configured.
    ///
    /// # Errors
    /// Rejects invalid host limits. Authorization errors remain explicit failures.
    pub async fn maintain(
        &mut self,
        limit: usize,
    ) -> Result<SearchMaintenanceReport, KnowledgeError> {
        if limit == 0 || limit > 256 {
            return Err(KnowledgeError::Config {
                reason: "search_maintenance_limit",
            });
        }
        let mut report = SearchMaintenanceReport::default();
        match self.observer.drain(1, limit).await {
            Ok(pages) => {
                for page in pages {
                    report.journal_records += page.examined;
                    report.more |= !page.complete;
                }
            }
            Err(e) => report.failures.push(e.code().into()),
        }
        let sessions = self.journal.config().sessions.clone();
        let mut budget = limit;
        for _ in 0..sessions.len() {
            if budget == 0 {
                report.more = true;
                break;
            }
            let Some(session) = sessions.get(self.journal_offset % sessions.len()) else {
                break;
            };
            match self.journal.sync_session(*session, budget).await {
                Ok(page) => {
                    budget = budget.saturating_sub(page.examined);
                    report.journal_records += page.examined;
                    if !page.complete {
                        report.more = true;
                        break;
                    }
                }
                Err(e) => report.failures.push(e.code().into()),
            }
            self.journal_offset = (self.journal_offset + 1) % sessions.len();
        }
        match self
            .documents
            .reconcile_sources(self.document_cursor.take(), limit)
            .await
        {
            Ok(page) => {
                report.removed += page.removed;
                report.more |= page.truncated;
                self.document_cursor = page.next_after;
                if page.unavailable > 0 {
                    report.failures.push("document_sources_unavailable".into());
                }
            }
            Err(e) => report.failures.push(e.code().into()),
        }
        if self.embedded {
            match self
                .memory
                .reconcile_embeddings(self.memory_offset, limit)
                .await
            {
                Ok(page) => {
                    report.embeddings += page.examined;
                    report.more |= page.next_offset.is_some();
                    self.memory_offset = page.next_offset.unwrap_or_default();
                }
                Err(e) => report.failures.push(e.code().into()),
            }
            match self.documents.reconcile_embeddings(limit.min(64)).await {
                Ok(count) => {
                    report.embeddings += count;
                    report.more |= count == limit.min(64);
                }
                Err(e) => report.failures.push(e.code().into()),
            }
        }
        self.maintain_graph(limit, &mut report).await;
        report.failures.sort();
        report.failures.dedup();
        Ok(report)
    }
    async fn maintain_graph(&mut self, limit: usize, report: &mut SearchMaintenanceReport) {
        if let Some(graph) = &self.graph {
            match graph.reconcile(self.graph_cursor.take(), limit).await {
                Ok(page) => {
                    report.removed += page.removed;
                    report.more |= page.next_cursor.is_some();
                    self.graph_cursor = page.next_cursor;
                    if page.unavailable > 0 {
                        report.failures.push("graph_sources_unavailable".into());
                    }
                }
                Err(e) => report.failures.push(e.code().into()),
            }
            match graph.rebuild_step(self.build_cursor.take(), limit).await {
                Ok(page) => {
                    report.graph_references += page.indexed;
                    report.removed += page.removed;
                    report.more |= page.next_cursor.is_some();
                    self.build_cursor = page.next_cursor;
                    if page.unavailable > 0 {
                        report.failures.push("graph_sources_unavailable".into());
                    }
                }
                Err(e) => report.failures.push(e.code().into()),
            }
        }
    }
}
