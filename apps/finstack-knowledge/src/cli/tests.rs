use clap::Parser as _;
use finstack_ai_kernel::RunEvent;

use super::args::{Cli, Command, SessionsCommand};
use super::render::{EventSink as _, JsonRenderer, TextRenderer, render_markup_plain};
use crate::tests::loopback;
use crate::{build_agent, model_name, security};

/// Run one scripted question and capture the real event stream + answer.
async fn capture_run(answer: &str) -> (Vec<RunEvent>, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(vec![loopback::text_response(answer)])
        .await
        .expect("loopback");
    let config = crate::KnowledgeConfig::new(
        dir.path().to_path_buf(),
        crate::ProviderChoice::Ollama {
            base_url,
            model: "preview-model".to_owned(),
        },
    );
    let agent = build_agent(&config).await.expect("agent");
    let request = finstack_ai::AgentRunRequest::try_new(
        model_name(&config).expect("model"),
        "Say the answer.",
        security("render-test").expect("security"),
    )
    .expect("request");
    let run = agent.start(request).expect("start");
    let mut events = Vec::new();
    while let Some(batch) = run.next_event_batch().await.expect("batch") {
        events.extend(batch.events().iter().cloned());
    }
    let output = run.result().await.expect("result");
    server.await.expect("join").expect("served");
    (events, output.text())
}

#[tokio::test]
async fn ask_creates_a_session_then_resumes_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(vec![
        loopback::text_response("first answer"),
        loopback::text_response("second answer"),
    ])
    .await
    .expect("loopback");
    let config = crate::KnowledgeConfig::new(
        dir.path().to_path_buf(),
        crate::ProviderChoice::Ollama {
            base_url,
            model: "preview-model".to_owned(),
        },
    );

    // Fresh ask: creates a session and answers.
    let mut sink = TextRenderer::new();
    let first = super::ask::run_ask(
        &config,
        "what is a lane?",
        None,
        "ask-test",
        &mut sink,
    )
    .await
    .expect("first ask");
    assert!(first.created);
    assert!(!first.session_id.is_empty());
    let plain = render_markup_plain(&sink.into_markup());
    assert!(plain.contains("first answer"), "answer rendered: {plain}");

    // Second ask with the printed id continues the same session.
    let mut sink = TextRenderer::new();
    let second = super::ask::run_ask(
        &config,
        "and a session?",
        Some(&first.session_id),
        "ask-test",
        &mut sink,
    )
    .await
    .expect("second ask");
    assert!(!second.created);
    assert_eq!(second.session_id, first.session_id);
    server.await.expect("join").expect("served");

    // History grew: both turns live on the same main lane.
    let journal = crate::open_journal(&config).expect("journal");
    let session = finstack_ai::Session::open(
        journal,
        finstack_ai_kernel::SessionId::parse(&first.session_id).expect("id parses"),
        "local",
    )
    .await
    .expect("session opens");
    let lane = session.lane("main").await.expect("main lane");
    let inspect = lane.inspect().await.expect("inspect");
    assert!(
        inspect.history.len() >= 4,
        "two user turns and two answers, got {}",
        inspect.history.len()
    );
}

#[tokio::test]
async fn ask_json_mode_emits_expected_event_kinds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) =
        loopback::serve_ndjson(vec![loopback::text_response("json mode answer")])
            .await
            .expect("loopback");
    let config = crate::KnowledgeConfig::new(
        dir.path().to_path_buf(),
        crate::ProviderChoice::Ollama {
            base_url,
            model: "preview-model".to_owned(),
        },
    );
    let mut sink = JsonRenderer::new();
    super::ask::run_ask(&config, "kinds?", None, "ask-test", &mut sink)
        .await
        .expect("ask");
    server.await.expect("join").expect("served");
    let output = sink.into_markup();
    for expected in ["message_finalized", "run_completed"] {
        assert!(
            output.contains(&format!("\"kind\":\"{expected}\"")),
            "ndjson contains {expected}: {output}"
        );
    }
}

#[tokio::test]
async fn ask_unknown_session_id_is_a_config_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(Vec::new()).await.expect("loopback");
    let config = crate::KnowledgeConfig::new(
        dir.path().to_path_buf(),
        crate::ProviderChoice::Ollama {
            base_url,
            model: "preview-model".to_owned(),
        },
    );
    let mut sink = TextRenderer::new();
    let error = super::ask::run_ask(&config, "q", Some("not-a-session-id"), "ask-test", &mut sink)
        .await
        .expect_err("bad id rejected");
    drop(server);
    assert!(matches!(
        error,
        crate::KnowledgeError::Config { .. } | crate::KnowledgeError::Compose { .. }
    ));
}

#[tokio::test]
async fn text_renderer_streams_deltas_and_result() {
    let (events, answer) = capture_run("rendered answer text").await;
    let mut renderer = TextRenderer::new();
    renderer.on_events(&events);
    renderer.finish(&answer);
    let plain = render_markup_plain(&renderer.into_markup());
    assert!(
        plain.contains("rendered answer text"),
        "delta text inline: {plain}"
    );
    assert!(plain.contains("run completed"), "completion line: {plain}");
}

#[tokio::test]
async fn json_renderer_emits_one_verbatim_ndjson_object_per_event() {
    let (events, answer) = capture_run("json answer").await;
    let mut renderer = JsonRenderer::new();
    renderer.on_events(&events);
    renderer.finish(&answer);
    let output = renderer.into_markup();
    let lines: Vec<&str> = output.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), events.len(), "one line per event");
    let mut kinds = Vec::new();
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line).expect("valid json");
        let kind = value["kind"].as_str().expect("kind field").to_owned();
        kinds.push(kind);
    }
    assert!(kinds.iter().any(|kind| kind == "run_completed"), "kinds: {kinds:?}");
    assert!(kinds.iter().any(|kind| kind == "model_text_delta"), "kinds: {kinds:?}");
}

#[tokio::test]
async fn both_renderers_consume_identical_event_counts() {
    let (events, answer) = capture_run("count parity").await;
    let mut text = TextRenderer::new();
    let mut json = JsonRenderer::new();
    text.on_events(&events);
    json.on_events(&events);
    text.finish(&answer);
    json.finish(&answer);
    assert_eq!(text.events_seen(), json.events_seen());
    assert_eq!(text.events_seen(), events.len() as u64);
}

#[test]
fn parses_ask_with_session_and_json() {
    let cli = Cli::try_parse_from([
        "finstack-know",
        "--json",
        "ask",
        "what is a lane?",
        "--session",
        "abc123",
    ])
    .expect("parses");
    assert!(cli.json);
    match cli.command {
        Command::Ask(args) => {
            assert_eq!(args.question, "what is a lane?");
            assert_eq!(args.session.as_deref(), Some("abc123"));
        }
        other => panic!("expected ask, got {other:?}"),
    }
}

#[test]
fn parses_sessions_subcommands() {
    let list = Cli::try_parse_from(["finstack-know", "sessions", "list"]).expect("list parses");
    assert!(matches!(
        list.command,
        Command::Sessions { command: SessionsCommand::List }
    ));
    let show =
        Cli::try_parse_from(["finstack-know", "sessions", "show", "id-1"]).expect("show parses");
    match show.command {
        Command::Sessions { command: SessionsCommand::Show { id } } => assert_eq!(id, "id-1"),
        other => panic!("expected show, got {other:?}"),
    }
    let name = Cli::try_parse_from(["finstack-know", "sessions", "name", "id-1", "quarterly"])
        .expect("name parses");
    match name.command {
        Command::Sessions { command: SessionsCommand::Name { id, name } } => {
            assert_eq!(id, "id-1");
            assert_eq!(name, "quarterly");
        }
        other => panic!("expected name, got {other:?}"),
    }
}

#[test]
fn parses_global_provider_flags() {
    let cli = Cli::try_parse_from([
        "finstack-know",
        "--data-dir",
        "/tmp/kd",
        "--provider",
        "openai",
        "--model",
        "gpt-5",
        "--api-key-env",
        "MY_KEY",
        "docs",
    ])
    .expect("parses");
    assert_eq!(cli.data_dir.as_deref(), Some(std::path::Path::new("/tmp/kd")));
    assert_eq!(cli.provider, super::args::ProviderKind::OpenAi);
    assert_eq!(cli.model.as_deref(), Some("gpt-5"));
    assert_eq!(cli.api_key_env.as_deref(), Some("MY_KEY"));
}

#[test]
fn rejects_unknown_subcommand() {
    assert!(Cli::try_parse_from(["finstack-know", "frobnicate"]).is_err());
}

#[test]
fn docs_lists_topics_and_renders_bodies() {
    let listing = super::docs_command(None).expect("listing renders");
    for topic in ["architecture", "sessions", "memory", "ingestion", "cli"] {
        assert!(listing.contains(topic), "listing names {topic}: {listing}");
    }
    let body = super::docs_command(Some("sessions")).expect("topic renders");
    assert!(body.contains("lane"), "sessions doc mentions lanes: {body}");
}

#[test]
fn docs_unknown_topic_is_a_config_error() {
    assert!(matches!(
        super::docs_command(Some("no-such-topic")),
        Err(crate::KnowledgeError::Config { .. })
    ));
}
