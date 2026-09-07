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
    SearchScope, SearchStrategy,
};
use finstack_ai_store_artifact::LocalArtifactStore;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Instant};
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

#[tokio::main(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // One measured build/query lifecycle; fixture construction lives in corpus.
async fn main() -> Result<(), Error> {
    let args = Args::parse();
    if args.chunks == 0
        || args.chunks > 100_000
        || !args.chunks.is_multiple_of(100)
        || args.memories < 100
        || args.memories > 10_000
        || args.journal_entries < 100
        || args.journal_entries > 10_000
        || !(5..=200).contains(&args.samples)
    {
        return Err("invalid benchmark bounds".into());
    }
    let manifest = args.root.join("corpus.json");
    if !args.reuse && args.root.exists() && args.root.read_dir()?.next().is_some() {
        return Err("benchmark requires an empty directory unless --reuse is set".into());
    }
    std::fs::create_dir_all(&args.root)?;
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
    let mut timings = BTreeMap::new();
    let identity = json!({"version":1,"chunks":args.chunks,"memories":args.memories,"journal_entries":args.journal_entries,"embedding_dimensions":64,"chunker_target":4096,"chunker_overlap":512});
    if args.reuse {
        let saved: Value = serde_json::from_slice(&std::fs::read(&manifest)?)?;
        if saved != identity {
            return Err("benchmark corpus parameters differ".into());
        }
    } else {
        let start = Instant::now();
        corpus::memory(&store, args.memories).await?;
        timings.insert("memory_build_ms", start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        for offset in (0..args.memories).step_by(256) {
            memory.reconcile_embeddings(offset, 256).await?;
        }
        timings.insert(
            "memory_embeddings_ms",
            start.elapsed().as_secs_f64() * 1000.0,
        );
    }
    let mut queries = BTreeMap::new();
    for (label, strategy) in [
        ("memory_lexical", SearchStrategy::Lexical(LexicalKind::Bm25)),
        (
            "memory_semantic",
            SearchStrategy::Semantic {
                space: embedder.descriptor().embedder_id.clone(),
            },
        ),
    ] {
        queries.insert(
            label,
            measure::source(
                memory.as_ref(),
                &scope,
                strategy,
                args.samples,
                args.memories,
            )
            .await?,
        );
    }
    if !args.memory_only {
        let artifacts = Arc::new(LocalArtifactStore::try_new(args.root.join("artifacts"))?);
        let config = DocumentIndexConfig::new("documents", scope.clone());
        let documents = Arc::new(DocumentSearchSource::try_open(
            &args.root.join("documents.sqlite"),
            artifacts.clone(),
            config.clone(),
            Some(embedder.clone()),
        )?);
        if !args.reuse {
            let start = Instant::now();
            let inputs =
                corpus::documents(artifacts.as_ref(), documents.as_ref(), args.chunks).await?;
            timings.insert("document_build_ms", start.elapsed().as_secs_f64() * 1000.0);
            std::fs::write(
                args.root.join("document-inputs.json"),
                serde_json::to_vec(&inputs)?,
            )?;
            let start = Instant::now();
            let mut vector_count = 0;
            loop {
                let count = documents.reconcile_embeddings(64).await?;
                vector_count += count;
                if count == 0 {
                    break;
                }
            }
            if vector_count != args.chunks {
                return Err("not every document chunk was embedded".into());
            }
            timings.insert(
                "document_embeddings_ms",
                start.elapsed().as_secs_f64() * 1000.0,
            );
            let rebuilt = DocumentSearchSource::try_open(
                &args.root.join("documents-rebuilt.sqlite"),
                artifacts,
                config,
                None,
            )?;
            let start = Instant::now();
            let mut chunks = 0;
            for input in inputs {
                chunks += rebuilt.index_document(input).await?.chunks;
            }
            if chunks != args.chunks {
                return Err("document rebuild count differs".into());
            }
            timings.insert(
                "document_rebuild_ms",
                start.elapsed().as_secs_f64() * 1000.0,
            );
        }
        for (label, strategy) in [
            (
                "document_lexical",
                SearchStrategy::Lexical(LexicalKind::Bm25),
            ),
            (
                "document_semantic",
                SearchStrategy::Semantic {
                    space: embedder.descriptor().embedder_id.clone(),
                },
            ),
        ] {
            queries.insert(
                label,
                measure::source(
                    documents.as_ref(),
                    &scope,
                    strategy,
                    args.samples,
                    args.memories,
                )
                .await?,
            );
        }
        let legs = ["memory", "documents"]
            .into_iter()
            .flat_map(|source| {
                [
                    SearchStrategy::Lexical(LexicalKind::Bm25),
                    SearchStrategy::Semantic {
                        space: embedder.descriptor().embedder_id.clone(),
                    },
                ]
                .into_iter()
                .map(move |strategy| HybridLeg {
                    source: source.into(),
                    strategy,
                    weight_micros: 1_000_000,
                })
            })
            .collect();
        let engine = SearchEngine::try_new(
            SearchConfig {
                scope: scope.clone(),
                default_plan: HybridPlan {
                    legs,
                    fusion: Fusion::default(),
                },
                limits: SearchLimits::default(),
                max_concurrency: 4,
                source_timeout_ms: 60_000,
                graph_expansion: None,
            },
            vec![memory.clone(), documents],
        )?;
        queries.insert("hybrid", measure::hybrid(&engine, args.samples).await?);
        let (journal, journal_times) = Box::pin(corpus::journal(
            &args.root,
            &scope,
            args.journal_entries,
            args.reuse,
        ))
        .await?;
        timings.extend(journal_times);
        queries.insert(
            "journal_lexical",
            measure::source(
                &journal,
                &scope,
                SearchStrategy::Lexical(LexicalKind::Bm25),
                args.samples,
                args.memories,
            )
            .await?,
        );
        let mut config = GraphIndexConfig::new("graph", scope.clone(), corpus::vocabulary());
        config.max_entities = 20_000;
        let graph =
            GraphSearchSource::try_open(&args.root.join("graph.sqlite"), config, vec![memory])?;
        if !args.reuse {
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
            if indexed != args.memories {
                return Err("graph source count differs".into());
            }
            timings.insert("graph_build_ms", start.elapsed().as_secs_f64() * 1000.0);
        }
        queries.insert(
            "graph_neighborhood",
            measure::source(
                &graph,
                &scope,
                SearchStrategy::Graph(GraphQuery::Neighborhood {
                    depth: 2,
                    max_nodes: 100,
                    max_edges: 1000,
                }),
                args.samples,
                args.memories,
            )
            .await?,
        );
    }
    if !args.reuse {
        std::fs::write(manifest, serde_json::to_vec_pretty(&identity)?)?;
    }
    println!(
        "{}",
        serde_json::to_string(
            &json!({"corpus":identity,"reuse":args.reuse,"memory_only":args.memory_only,"timings":timings,"queries":queries,"bytes":measure::sizes(&args.root)?})
        )?
    );
    Ok(())
}
