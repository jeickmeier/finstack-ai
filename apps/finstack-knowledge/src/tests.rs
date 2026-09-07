use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    EmbedderChoice, KnowledgeConfig, KnowledgeError, ProviderChoice, SELF_DOCS, build_agent,
    default_data_dir, materialize_self_docs, model_name, security,
};

pub(crate) mod loopback;

fn ollama() -> ProviderChoice {
    ProviderChoice::Ollama {
        base_url: "http://127.0.0.1:11434".to_owned(),
        model: "gemma4:26b".to_owned(),
    }
}

#[test]
fn data_dir_prefers_explicit_then_know_home_then_home() {
    let explicit = default_data_dir(
        Some(PathBuf::from("/tmp/explicit")),
        Some(PathBuf::from("/tmp/know-home")),
        Some(PathBuf::from("/tmp/home")),
    )
    .expect("explicit wins");
    assert_eq!(explicit, PathBuf::from("/tmp/explicit"));

    let know_home = default_data_dir(
        None,
        Some(PathBuf::from("/tmp/know-home")),
        Some(PathBuf::from("/tmp/home")),
    )
    .expect("know home wins over home");
    assert_eq!(know_home, PathBuf::from("/tmp/know-home"));

    let home =
        default_data_dir(None, None, Some(PathBuf::from("/tmp/home"))).expect("home fallback");
    assert_eq!(home, PathBuf::from("/tmp/home/.finstack-know"));

    assert!(matches!(
        default_data_dir(None, None, None),
        Err(KnowledgeError::Config { .. })
    ));
}

#[test]
fn fetch_allowlist_defaults_empty() {
    let config = KnowledgeConfig::new(PathBuf::from("/tmp/data"), ollama());
    assert!(config.fetch_allowlist.is_empty());
}

#[test]
fn security_rejects_empty_user() {
    assert!(matches!(security(""), Err(KnowledgeError::Config { .. })));
    assert!(matches!(
        security("   "),
        Err(KnowledgeError::Config { .. })
    ));
}

#[test]
fn security_builds_local_context_for_os_user() {
    let context = security("jeickmeier").expect("valid user");
    let debug = format!("{context:?}");
    assert!(debug.contains("local"), "tenant label present: {debug}");
    assert!(debug.contains("jeickmeier"), "principal present: {debug}");
}

fn loopback_config(dir: &std::path::Path, base_url: String) -> KnowledgeConfig {
    KnowledgeConfig::new(
        dir.to_path_buf(),
        ProviderChoice::Ollama {
            base_url,
            model: "preview-model".to_owned(),
        },
    )
}

#[tokio::test]
async fn agent_builds_and_answers_offline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) =
        loopback::serve_ndjson(vec![loopback::text_response("knowledge answer")])
            .await
            .expect("loopback");
    let config = loopback_config(dir.path(), base_url);
    let agent = build_agent(&config).await.expect("agent builds").agent;
    let request = finstack_ai::AgentRunRequest::try_new(
        model_name(&config).expect("model name"),
        "Say hello.",
        security("tester").expect("security"),
    )
    .expect("request");
    let output = agent.run(request).await.expect("run succeeds");
    server.await.expect("server task").expect("server ok");
    assert!(output.text().contains("knowledge answer"));
    // The journal landed in the data dir.
    assert!(dir.path().join("journal.sqlite3").exists());
    // Self-docs were materialized for the repository provider.
    assert!(dir.path().join("self-docs/architecture.md").exists());
}

#[tokio::test]
async fn self_docs_reach_the_model_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson_capture(vec![loopback::text_response("ok")])
        .await
        .expect("loopback");
    let config = loopback_config(dir.path(), base_url);
    let agent = build_agent(&config).await.expect("agent builds").agent;
    let request = finstack_ai::AgentRunRequest::try_new(
        model_name(&config).expect("model name"),
        "What is the architecture?",
        security("tester").expect("security"),
    )
    .expect("request");
    agent.run(request).await.expect("run succeeds");
    let captured = server.await.expect("server task").expect("server ok");
    // The self-docs repository provider needs an explicit `<topic>.md`
    // allowlist; with the default one it silently contributes nothing.
    assert!(
        captured
            .iter()
            .any(|body| body.contains("deterministic agent microkernel")),
        "self-docs text missing from the model request"
    );
}

#[tokio::test]
async fn capability_catalog_lists_citation_skill() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(Vec::new()).await.expect("loopback");
    let config = loopback_config(dir.path(), base_url);
    let agent = build_agent(&config).await.expect("agent builds").agent;
    drop(server);
    let catalog = agent.compact_capability_catalog();
    assert!(
        catalog.contains("finstack.know.skill.citations"),
        "catalog: {catalog}"
    );
}

#[tokio::test]
async fn fetch_allowlist_gates_the_fetch_toolset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(Vec::new()).await.expect("loopback");
    // Invalid pattern is rejected at build time.
    let bad = loopback_config(dir.path(), base_url.clone())
        .with_fetch_allowlist(vec!["not a host pattern!!".to_owned()]);
    assert!(matches!(
        build_agent(&bad).await,
        Err(KnowledgeError::Config { .. } | KnowledgeError::Compose { .. })
    ));
    // Valid allowlist builds.
    let good = loopback_config(dir.path(), base_url)
        .with_fetch_allowlist(vec!["docs.example.com".to_owned()]);
    build_agent(&good)
        .await
        .expect("fetch-enabled agent builds");
    drop(server);
}

/// Dimensionality shared by the test embedder config and the scripted
/// loopback `/api/embed` server.
const EMBED_DIMENSIONS: usize = 32;

fn ollama_embedder(base_url: String) -> EmbedderChoice {
    EmbedderChoice::Ollama {
        base_url,
        model: "hash-embed".to_owned(),
        dimensions: EMBED_DIMENSIONS,
    }
}

/// Does the global search tool advertise a semantic strategy?
fn search_advertises_semantic(agent: &finstack_ai::Agent) -> bool {
    agent
        .resolved()
        .run_plan()
        .toolsets()
        .iter()
        .any(|toolset| {
            toolset.handle().tools().iter().any(|spec| {
                spec.model_name.as_ref() == "search"
                    && spec.input_schema.as_str().contains("\"semantic\"")
            })
        })
}

#[test]
fn default_config_has_no_embedder() {
    let config = KnowledgeConfig::new(PathBuf::from("/tmp/data"), ollama());
    assert!(config.embedder.is_none());
}

#[tokio::test]
async fn embedder_advertises_semantic_search() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(Vec::new()).await.expect("loopback");

    // Lexical-only composition omits semantic strategies.
    let lexical = loopback_config(dir.path(), base_url.clone());
    let agent = build_agent(&lexical)
        .await
        .expect("lexical agent builds")
        .agent;
    assert!(!search_advertises_semantic(&agent));
    drop(agent);

    // The explicit embedder enables semantic search in the global tool.
    let semantic =
        loopback_config(dir.path(), base_url.clone()).with_embedder(ollama_embedder(base_url));
    let agent = build_agent(&semantic)
        .await
        .expect("semantic agent builds")
        .agent;
    assert!(search_advertises_semantic(&agent));
    drop(server);
}

#[tokio::test]
async fn down_embedder_never_blocks_startup() {
    use finstack_ai_kernel::{Sensitivity, UNIX_EPOCH};
    use finstack_ai_memory::record::{
        ExtractionMethod, MemoryBody, MemoryId, MemoryProvenance, MemoryRecord, MemoryScope,
        RetentionPolicy,
    };
    use finstack_ai_memory::store::{MemoryStore, SqliteMemoryStore};

    let dir = tempfile::tempdir().expect("tempdir");
    // Seed one live record so the startup backfill genuinely attempts the
    // embedder instead of short-circuiting on an empty pending set.
    {
        let store = SqliteMemoryStore::try_open(&dir.path().join("memory.sqlite3"))
            .expect("memory store opens");
        let record = MemoryRecord {
            id: MemoryId::parse("seed-record").expect("id"),
            scope: MemoryScope::try_new("local").expect("scope"),
            keywords: Arc::from([Arc::<str>::from("seed")]),
            body: MemoryBody::Inline(Arc::from("seed body")),
            preview: Arc::from("seed body"),
            sensitivity: Sensitivity::Internal,
            provenance: MemoryProvenance {
                source_session: None,
                source_run: None,
                source_ref: None,
                extraction: ExtractionMethod::Explicit,
                confidence: 80,
            },
            created_at: UNIX_EPOCH,
            last_confirmed_at: UNIX_EPOCH,
            supersedes: None,
            superseded_by: None,
            retention: RetentionPolicy::KeepUntilDeleted,
            tombstoned: false,
        };
        store
            .put(Arc::from("seed-key"), record)
            .await
            .expect("seed record stored");
    }

    // Bind then drop a loopback listener: the embedder endpoint is down.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let dead_url = format!("http://{}", listener.local_addr().expect("address"));
    drop(listener);

    let (base_url, server) = loopback::serve_ndjson(Vec::new()).await.expect("loopback");
    let config = loopback_config(dir.path(), base_url).with_embedder(ollama_embedder(dead_url));
    build_agent(&config)
        .await
        .expect("a down embedder never blocks startup");
    drop(server);
}

#[test]
fn golden_fixture_loads_and_validates() {
    let entries = crate::golden_entries().expect("fixture loads");
    assert!(entries.len() >= 10, "at least ten entries");
    let mut ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), entries.len(), "ids are unique");
    for entry in &entries {
        assert!(!entry.question.trim().is_empty(), "{}: question", entry.id);
        assert!(
            !entry.scripted_response.trim().is_empty(),
            "{}: scripted_response",
            entry.id
        );
        assert!(!entry.must_contain.is_empty(), "{}: must_contain", entry.id);
        assert!(
            !entry.event_kinds_expected.is_empty(),
            "{}: event_kinds_expected",
            entry.id
        );
    }
}

#[tokio::test]
async fn golden_entries_hold_offline() {
    let entries = crate::golden_entries().expect("fixture loads");
    for entry in entries {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base_url, server) =
            loopback::serve_ndjson(vec![loopback::text_response(&entry.scripted_response)])
                .await
                .expect("loopback");
        let config = loopback_config(dir.path(), base_url);
        let agent = build_agent(&config).await.expect("agent builds").agent;
        let request = finstack_ai::AgentRunRequest::try_new(
            model_name(&config).expect("model name"),
            entry.question.as_str(),
            security("golden").expect("security"),
        )
        .expect("request");
        let run = agent.start(request).expect("run starts");
        let mut observed_kinds: Vec<String> = Vec::new();
        while let Some(batch) = run.next_event_batch().await.expect("batch") {
            for event in batch.events() {
                let kind = serde_json::to_value(event.kind()).expect("kind serializes");
                if let Some(kind) = kind.as_str() {
                    observed_kinds.push(kind.to_owned());
                }
            }
        }
        let output = run.result().await.expect("run result");
        server.await.expect("server task").expect("server ok");
        for needle in &entry.must_contain {
            assert!(
                output.text().contains(needle),
                "{}: answer must contain {needle:?}: {}",
                entry.id,
                output.text()
            );
        }
        for expected in &entry.event_kinds_expected {
            assert!(
                observed_kinds.iter().any(|kind| kind == expected),
                "{}: expected event kind {expected}; observed {observed_kinds:?}",
                entry.id
            );
        }
    }
}

/// The one golden entry carrying a semantic-recall seed.
fn semantic_entry() -> crate::GoldenEntry {
    let entries = crate::golden_entries().expect("fixture loads");
    let mut seeded = entries
        .into_iter()
        .filter(|entry| entry.memory_seed.is_some());
    let entry = seeded.next().expect("a semantic entry exists");
    assert!(seeded.next().is_none(), "exactly one semantic entry");
    entry
}

/// One scripted NDJSON `remember` tool call storing `seed`.
fn remember_response(seed: &crate::GoldenMemorySeed) -> String {
    let call = serde_json::json!({
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "function": {
                    "name": "remember",
                    "arguments": { "keywords": &seed.keywords, "body": &seed.body }
                }
            }]
        },
        "done": false
    });
    format!(
        "{call}\n{{\"message\":{{\"role\":\"assistant\",\"content\":\"\"}},\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}}\n"
    )
}

/// Run the semantic entry's scripted flow — a `remember` seeding run, then
/// the paraphrased ask in a fresh session — and return the entry plus the
/// ask run's model request, answer text, and observed event kinds.
async fn run_semantic_entry(
    with_embedder: bool,
) -> (crate::GoldenEntry, String, String, Vec<String>) {
    let entry = semantic_entry();
    let seed = entry
        .memory_seed
        .clone()
        .expect("semantic entry has a seed");
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ollama_scripted(
        vec![
            remember_response(&seed),
            loopback::text_response("Noted."),
            loopback::text_response(&entry.scripted_response),
        ],
        EMBED_DIMENSIONS,
    )
    .await
    .expect("loopback");
    let mut config = loopback_config(dir.path(), base_url.clone());
    if with_embedder {
        config = config.with_embedder(ollama_embedder(base_url));
    }
    let agent = build_agent(&config).await.expect("agent builds").agent;

    // Seeding run: the scripted `remember` call stores the record.
    let request = finstack_ai::AgentRunRequest::try_new(
        model_name(&config).expect("model name"),
        "Please save this note.",
        security("golden").expect("security"),
    )
    .expect("request");
    agent.run(request).await.expect("seeding run succeeds");

    // Ask run: the paraphrased question in a fresh session.
    let request = finstack_ai::AgentRunRequest::try_new(
        model_name(&config).expect("model name"),
        entry.question.as_str(),
        security("golden").expect("security"),
    )
    .expect("request");
    let run = agent.start(request).expect("run starts");
    let mut observed_kinds: Vec<String> = Vec::new();
    while let Some(batch) = run.next_event_batch().await.expect("batch") {
        for event in batch.events() {
            let kind = serde_json::to_value(event.kind()).expect("kind serializes");
            if let Some(kind) = kind.as_str() {
                observed_kinds.push(kind.to_owned());
            }
        }
    }
    let output = run.result().await.expect("run result");
    let captured = server.await.expect("server task").expect("server ok");
    assert_eq!(captured.len(), 3, "three chat requests");
    let ask_request = captured.into_iter().nth(2).expect("ask request captured");
    (entry, ask_request, output.text().clone(), observed_kinds)
}

/// Lowercased alphanumeric tokens, the unit both lexical legs match on.
fn lexical_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[test]
fn semantic_golden_question_shares_no_lexical_token_with_its_seed() {
    let entry = semantic_entry();
    let seed = entry.memory_seed.expect("semantic entry has a seed");
    let question_tokens = lexical_tokens(&entry.question);
    let mut seed_tokens = lexical_tokens(&seed.body);
    for keyword in &seed.keywords {
        seed_tokens.extend(lexical_tokens(keyword));
    }
    assert!(!question_tokens.is_empty() && !seed_tokens.is_empty());
    for question_token in &question_tokens {
        for seed_token in &seed_tokens {
            // Full-text recall prefix-matches every question token, so a
            // shared prefix in either direction would let a lexical leg
            // surface the record and the entry would stop guarding the
            // semantic leg.
            assert!(
                !seed_token.starts_with(question_token.as_str())
                    && !question_token.starts_with(seed_token.as_str()),
                "question token {question_token:?} lexically overlaps seed token {seed_token:?}"
            );
        }
    }
}

#[tokio::test]
async fn semantic_golden_misses_under_lexical_recall() {
    let (entry, ask_request, _answer, _kinds) = run_semantic_entry(false).await;
    let seed = entry.memory_seed.expect("semantic entry has a seed");
    // The guard: lexical-only recall must not surface the seeded record —
    // otherwise the fixture would not genuinely require the semantic leg.
    assert!(
        !ask_request.contains(seed.body.as_str()),
        "lexical recall unexpectedly surfaced the seed: {ask_request}"
    );
}

#[tokio::test]
async fn semantic_golden_surfaces_with_embedder() {
    let (entry, ask_request, answer, observed_kinds) = run_semantic_entry(true).await;
    let seed = entry
        .memory_seed
        .clone()
        .expect("semantic entry has a seed");
    assert!(
        ask_request.contains(seed.body.as_str()),
        "semantic recall did not surface the seed in the model request: {ask_request}"
    );
    for needle in &entry.must_contain {
        assert!(
            answer.contains(needle),
            "{}: answer must contain {needle:?}: {answer}",
            entry.id
        );
    }
    for expected in &entry.event_kinds_expected {
        assert!(
            observed_kinds.iter().any(|kind| kind == expected),
            "{}: expected event kind {expected}; observed {observed_kinds:?}",
            entry.id
        );
    }
}

#[test]
fn self_docs_cover_the_promised_topics() {
    let topics: Vec<&str> = SELF_DOCS.iter().map(|(topic, _)| *topic).collect();
    for expected in ["architecture", "sessions", "memory", "ingestion", "cli"] {
        assert!(topics.contains(&expected), "missing topic {expected}");
    }
    for (topic, body) in SELF_DOCS {
        assert!(!body.trim().is_empty(), "empty body for {topic}");
    }
}

#[test]
fn materialize_writes_once_and_repairs_drift() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = materialize_self_docs(dir.path()).expect("materialize");
    assert_eq!(root, dir.path().join("self-docs"));
    for (topic, body) in SELF_DOCS {
        let path = root.join(format!("{topic}.md"));
        assert_eq!(&fs::read_to_string(&path).expect("readable"), body);
    }

    // Second call is a no-op: mtimes unchanged.
    let stamp = |topic: &str| {
        fs::metadata(root.join(format!("{topic}.md")))
            .and_then(|meta| meta.modified())
            .expect("mtime")
    };
    let before = stamp("architecture");
    let root_again = materialize_self_docs(dir.path()).expect("second materialize");
    assert_eq!(root_again, root);
    assert_eq!(stamp("architecture"), before);

    // Drifted on-disk content is repaired to the embedded body.
    let drifted = root.join("memory.md");
    fs::write(&drifted, "stale local edit").expect("write drift");
    materialize_self_docs(dir.path()).expect("repair");
    let repaired = fs::read_to_string(&drifted).expect("read");
    assert_ne!(repaired, "stale local edit");
}

#[test]
fn provider_debug_redacts_api_keys() {
    let choices = [
        ProviderChoice::Anthropic {
            api_key: "sk-secret-anthropic".to_owned(),
            model: "claude-fable-5".to_owned(),
        },
        ProviderChoice::OpenAi {
            api_key: "sk-secret-openai".to_owned(),
            model: "gpt".to_owned(),
        },
        ProviderChoice::OpenRouter {
            api_key: "sk-secret-openrouter".to_owned(),
            model: "meta/llama".to_owned(),
        },
    ];
    for choice in choices {
        let debug = format!("{choice:?}");
        assert!(!debug.contains("sk-secret"), "redacted: {debug}");
        let config = KnowledgeConfig::new(PathBuf::from("/tmp/data"), choice);
        let debug = format!("{config:?}");
        assert!(
            !debug.contains("sk-secret"),
            "redacted through config: {debug}"
        );
    }
}
