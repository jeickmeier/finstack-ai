use std::fs;
use std::path::PathBuf;

use crate::{
    KnowledgeConfig, KnowledgeError, ProviderChoice, SELF_DOCS, default_data_dir,
    materialize_self_docs, security,
};

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
