//! Deterministic native search operating-envelope benchmark; no provider calls.
//! Use `mise run bench-search`; JSON output includes observed source coverage.
use clap::Parser;
use finstack_ai_embeddings::embedder::{HashEmbedder, TextEmbedder};
use finstack_ai_index_documents::{DocumentIndexConfig, DocumentSearchSource};
use finstack_ai_index_graph::{GraphIndexConfig, GraphSearchSource};
use finstack_ai_memory::{search::MemorySearchSource, store::SqliteMemoryStore};
use finstack_ai_search::{SearchConfig, SearchEngine};
use finstack_ai_search_core::{
    Fusion, GraphQuery, HybridLeg, HybridPlan, LexicalKind, ScopeMapping, SearchLimits,
    SearchScope, SearchSource, SearchStrategy,
};
use finstack_ai_store_artifact::LocalArtifactStore;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
#[path = "search_scale/corpus.rs"]
mod corpus;
#[path = "search_scale/measure.rs"]
mod measure;
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Parser)]
struct Args {
    /// Empty application-owned directory, or the existing corpus with --reuse.
    #[arg(long)]
    root: PathBuf,
    /// Exact document chunks; multiples of 100, at most 100,000.
    #[arg(long, default_value_t = 100_000)]
    chunks: usize,
    #[arg(long, default_value_t = 10_000)]
    memories: usize,
    #[arg(long, default_value_t = 1000)]
    journal_entries: usize,
    #[arg(long, default_value_t = 30)]
    samples: usize,
    /// Query an existing generated corpus, preserving its indexed records.
    #[arg(long)]
    reuse: bool,
    /// Isolate memory query peak RSS from document parsing and indexing.
    #[arg(long)]
    memory_only: bool,
}

impl Args {
    fn validate(&self) -> Result<(), Error> {
        let valid = (1..=100_000).contains(&self.chunks)
            && self.chunks.is_multiple_of(100)
            && (100..=10_000).contains(&self.memories)
            && (100..=10_000).contains(&self.journal_entries)
            && (5..=200).contains(&self.samples);
        if valid {
            Ok(())
        } else {
            Err("invalid benchmark bounds".into())
        }
    }

    async fn prepare_root(&self) -> Result<(), Error> {
        if !self.reuse && tokio::fs::metadata(&self.root).await.is_ok() {
            let mut entries = tokio::fs::read_dir(&self.root).await?;
            if entries.next_entry().await?.is_some() {
                return Err("benchmark requires an empty directory unless --reuse is set".into());
            }
        }
        tokio::fs::create_dir_all(&self.root).await?;
        Ok(())
    }
}

/// Shared benchmark state: parsed bounds plus the timing and query reports.
struct Bench {
    args: Args,
    scope: SearchScope,
    embedder: Arc<dyn TextEmbedder>,
    timings: BTreeMap<&'static str, f64>,
    queries: BTreeMap<&'static str, Value>,
}

impl Bench {
    fn record(&mut self, label: &'static str, start: Instant) {
        self.timings
            .insert(label, start.elapsed().as_secs_f64() * 1000.0);
    }

    fn semantic(&self) -> SearchStrategy {
        SearchStrategy::Semantic {
            space: self.embedder.descriptor().embedder_id.clone(),
        }
    }

    async fn query(
        &mut self,
        label: &'static str,
        source: &dyn SearchSource,
        strategy: SearchStrategy,
    ) -> Result<(), Error> {
        let report = measure::source(
            source,
            &self.scope,
            strategy,
            self.args.samples,
            self.args.memories,
        )
        .await?;
        self.queries.insert(label, report);
        Ok(())
    }

    async fn memory_corpus(
        &mut self,
        store: &SqliteMemoryStore,
        memory: &MemorySearchSource,
        manifest: &Path,
        identity: &Value,
    ) -> Result<(), Error> {
        if self.args.reuse {
            let saved: Value = serde_json::from_slice(&tokio::fs::read(manifest).await?)?;
            if &saved != identity {
                return Err("benchmark corpus parameters differ".into());
            }
            return Ok(());
        }
        let start = Instant::now();
        corpus::memory(store, self.args.memories).await?;
        self.record("memory_build_ms", start);
        let start = Instant::now();
        for offset in (0..self.args.memories).step_by(256) {
            memory.reconcile_embeddings(offset, 256).await?;
        }
        self.record("memory_embeddings_ms", start);
        Ok(())
    }

    async fn document_corpus(
        &mut self,
        artifacts: Arc<LocalArtifactStore>,
        documents: &DocumentSearchSource,
        config: DocumentIndexConfig,
    ) -> Result<(), Error> {
        let start = Instant::now();
        let inputs = corpus::documents(artifacts.as_ref(), documents, self.args.chunks).await?;
        self.record("document_build_ms", start);
        tokio::fs::write(
            self.args.root.join("document-inputs.json"),
            serde_json::to_vec(&inputs)?,
        )
        .await?;
        let start = Instant::now();
        let mut vector_count = 0;
        loop {
            let count = documents.reconcile_embeddings(64).await?;
            vector_count += count;
            if count == 0 {
                break;
            }
        }
        if vector_count != self.args.chunks {
            return Err("not every document chunk was embedded".into());
        }
        self.record("document_embeddings_ms", start);
        let rebuilt = DocumentSearchSource::try_open(
            &self.args.root.join("documents-rebuilt.sqlite"),
            artifacts,
            config,
            None,
        )?;
        let start = Instant::now();
        let mut chunks = 0;
        for input in inputs {
            chunks += rebuilt.index_document(input).await?.chunks;
        }
        if chunks != self.args.chunks {
            return Err("document rebuild count differs".into());
        }
        self.record("document_rebuild_ms", start);
        Ok(())
    }

    fn hybrid_engine(
        &self,
        memory: Arc<MemorySearchSource>,
        documents: Arc<DocumentSearchSource>,
    ) -> Result<SearchEngine, Error> {
        let legs = ["memory", "documents"]
            .into_iter()
            .flat_map(|source| {
                [SearchStrategy::Lexical(LexicalKind::Bm25), self.semantic()]
                    .into_iter()
                    .map(move |strategy| HybridLeg {
                        source: source.into(),
                        strategy,
                        weight_micros: 1_000_000,
                    })
            })
            .collect();
        Ok(SearchEngine::try_new(
            SearchConfig {
                scope: self.scope.clone(),
                default_plan: HybridPlan {
                    legs,
                    fusion: Fusion::default(),
                },
                limits: SearchLimits::default(),
                max_concurrency: 4,
                source_timeout_ms: 60_000,
                graph_expansion: None,
            },
            vec![memory, documents],
        )?)
    }

    async fn graph_corpus(&mut self, graph: &GraphSearchSource) -> Result<(), Error> {
        let start = Instant::now();
        let mut cursor = None;
        let mut indexed = 0;
        loop {
            let page = graph.rebuild_step(cursor, 256).await?;
            if page.unavailable != 0 {
                return Err("graph build lost evidence".into());
            }
            indexed += page.indexed;
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        if indexed != self.args.memories {
            return Err("graph source count differs".into());
        }
        self.record("graph_build_ms", start);
        Ok(())
    }

    async fn documents_journal_and_graph(
        &mut self,
        memory: Arc<MemorySearchSource>,
    ) -> Result<(), Error> {
        let artifacts = Arc::new(LocalArtifactStore::try_new(
            self.args.root.join("artifacts"),
        )?);
        let config = DocumentIndexConfig::new("documents", self.scope.clone());
        let documents = Arc::new(DocumentSearchSource::try_open(
            &self.args.root.join("documents.sqlite"),
            artifacts.clone(),
            config.clone(),
            Some(self.embedder.clone()),
        )?);
        if !self.args.reuse {
            self.document_corpus(artifacts, documents.as_ref(), config)
                .await?;
        }
        self.query(
            "document_lexical",
            documents.as_ref(),
            SearchStrategy::Lexical(LexicalKind::Bm25),
        )
        .await?;
        self.query("document_semantic", documents.as_ref(), self.semantic())
            .await?;
        let engine = self.hybrid_engine(memory.clone(), documents)?;
        let hybrid = measure::hybrid(&engine, self.args.samples).await?;
        self.queries.insert("hybrid", hybrid);
        let (journal, journal_times) = Box::pin(corpus::journal(
            &self.args.root,
            &self.scope,
            self.args.journal_entries,
            self.args.reuse,
        ))
        .await?;
        self.timings.extend(journal_times);
        self.query(
            "journal_lexical",
            &journal,
            SearchStrategy::Lexical(LexicalKind::Bm25),
        )
        .await?;
        let mut config = GraphIndexConfig::new("graph", self.scope.clone(), corpus::vocabulary());
        config.max_entities = 20_000;
        let graph = GraphSearchSource::try_open(
            &self.args.root.join("graph.sqlite"),
            config,
            vec![memory],
        )?;
        if !self.args.reuse {
            self.graph_corpus(&graph).await?;
        }
        self.query(
            "graph_neighborhood",
            &graph,
            SearchStrategy::Graph(GraphQuery::Neighborhood {
                depth: 2,
                max_nodes: 100,
                max_edges: 1000,
            }),
        )
        .await
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Error> {
    let args = Args::parse();
    args.validate()?;
    let manifest = args.root.join("corpus.json");
    args.prepare_root().await?;
    let scope = SearchScope::try_new("search-benchmark")?;
    let embedder: Arc<dyn TextEmbedder> = Arc::new(HashEmbedder::try_new(64)?);
    let store = Arc::new(SqliteMemoryStore::try_open(
        &args.root.join("memory.sqlite"),
    )?);
    let memory = Arc::new(MemorySearchSource::try_new(
        "memory",
        store.clone(),
        scope.clone(),
        ScopeMapping::Exact,
        SearchLimits::default(),
        Some(embedder.clone()),
    )?);
    let identity = json!({"version":1,"chunks":args.chunks,"memories":args.memories,"journal_entries":args.journal_entries,"embedding_dimensions":64,"chunker_target":4096,"chunker_overlap":512});
    let mut bench = Bench {
        args,
        scope,
        embedder,
        timings: BTreeMap::new(),
        queries: BTreeMap::new(),
    };
    bench
        .memory_corpus(&store, &memory, &manifest, &identity)
        .await?;
    bench
        .query(
            "memory_lexical",
            memory.as_ref(),
            SearchStrategy::Lexical(LexicalKind::Bm25),
        )
        .await?;
    bench
        .query("memory_semantic", memory.as_ref(), bench.semantic())
        .await?;
    if !bench.args.memory_only {
        Box::pin(bench.documents_journal_and_graph(memory)).await?;
    }
    if !bench.args.reuse {
        tokio::fs::write(&manifest, serde_json::to_vec_pretty(&identity)?).await?;
    }
    let Bench {
        args,
        timings,
        queries,
        ..
    } = bench;
    println!(
        "{}",
        serde_json::to_string(
            &json!({"corpus":identity,"reuse":args.reuse,"memory_only":args.memory_only,"timings":timings,"queries":queries,"bytes":measure::sizes(&args.root)?})
        )?
    );
    Ok(())
}
