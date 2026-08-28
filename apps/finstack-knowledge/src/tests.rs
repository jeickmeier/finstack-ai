use std::fs;
use std::path::PathBuf;

use crate::{
    KnowledgeConfig, KnowledgeError, ProviderChoice, SELF_DOCS, build_agent, default_data_dir,
    materialize_self_docs, model_name, security,
};

mod loopback;

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

    let home = default_data_dir(None, None, Some(PathBuf::from("/tmp/home")))
        .expect("home fallback");
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
    assert!(matches!(
        security(""),
        Err(KnowledgeError::Config { .. })
    ));
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
    let agent = build_agent(&config).await.expect("agent builds");
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
async fn capability_catalog_lists_citation_skill() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(Vec::new()).await.expect("loopback");
    let config = loopback_config(dir.path(), base_url);
    let agent = build_agent(&config).await.expect("agent builds");
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
    build_agent(&good).await.expect("fetch-enabled agent builds");
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
        let agent = build_agent(&config).await.expect("agent builds");
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
        assert!(!debug.contains("sk-secret"), "redacted through config: {debug}");
    }
}
