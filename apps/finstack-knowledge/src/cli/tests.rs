use clap::Parser as _;
use finstack_ai_kernel::RunEvent;
use finstack_ai_memory::store::{MemoryPage, MemoryStore as _};

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
    let first = super::ask::run_ask(&config, "what is a lane?", None, "ask-test", &mut sink)
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
    let error = super::ask::run_ask(
        &config,
        "q",
        Some("not-a-session-id"),
        "ask-test",
        &mut sink,
    )
    .await
    .expect_err("bad id rejected");
    drop(server);
    assert!(matches!(
        error,
        crate::KnowledgeError::Config { .. } | crate::KnowledgeError::Compose { .. }
    ));
}

/// Ollama NDJSON body carrying one `remember` tool call, then a final turn.
fn remember_tool_response() -> String {
    concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[{\"function\":{\"name\":\"remember\",\"arguments\":{\"keywords\":[\"acme\",\"revenue\"],\"body\":\"Acme Corp revenue rose 12 percent in Q1 per ingested report.\"}}}]},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}\n",
    )
    .to_owned()
}

#[tokio::test]
async fn ingest_attaches_summarizes_and_captures_memory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let doc_path = dir.path().join("report.csv");
    std::fs::write(
        &doc_path,
        "company,metric,change\nAcme Corp,revenue,rose 12 percent in Q1\n",
    )
    .expect("fixture doc");

    let (base_url, server) = loopback::serve_ndjson(vec![
        remember_tool_response(),
        loopback::text_response("Summary: Acme revenue rose 12 percent."),
        loopback::text_response("It rose 12 percent (see report.md)."),
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

    // Ingest: attaches the doc, model remembers a fact and summarizes.
    let mut sink = TextRenderer::new();
    let outcome = super::ingest::run_ingest(&config, &doc_path, None, "ingest-test", &mut sink)
        .await
        .expect("ingest");
    let plain = render_markup_plain(&sink.into_markup());
    assert!(plain.contains("Summary"), "summary rendered: {plain}");

    // Follow-up ask in the same session answers from context.
    let mut sink = TextRenderer::new();
    super::ask::run_ask(
        &config,
        "How much did Acme revenue rise?",
        Some(&outcome.session_id),
        "ingest-test",
        &mut sink,
    )
    .await
    .expect("follow-up ask");
    let plain = render_markup_plain(&sink.into_markup());
    assert!(plain.contains("12 percent"), "follow-up answered: {plain}");
    server.await.expect("join").expect("served");

    // The memory store holds at least one captured record for the tenant.
    let memory =
        finstack_ai_memory::store::SqliteMemoryStore::try_open(&dir.path().join("memory.sqlite3"))
            .expect("memory store opens");
    let scope = finstack_ai_memory::record::MemoryScope::try_new("local").expect("scope");
    let records = memory
        .list(
            scope,
            MemoryPage {
                offset: 0,
                limit: 16,
            },
        )
        .await
        .expect("list");
    assert!(!records.records.is_empty(), "captured memory records");
}

#[tokio::test]
async fn ingest_missing_file_is_a_config_error() {
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
    let error = super::ingest::run_ingest(
        &config,
        std::path::Path::new("/definitely/missing.md"),
        None,
        "ingest-test",
        &mut sink,
    )
    .await
    .expect_err("missing file");
    drop(server);
    assert!(matches!(error, crate::KnowledgeError::Config { .. }));
}

/// Ollama NDJSON body carrying one `ask_user` elicitation call.
fn ask_user_response(prompt: &str) -> String {
    format!(
        "{{\"message\":{{\"role\":\"assistant\",\"content\":\"\",\"tool_calls\":[{{\"function\":{{\"name\":\"ask_user\",\"arguments\":{{\"prompt\":\"{prompt}\",\"kind\":null,\"options\":null,\"response_schema\":null}}}}}}]}},\"done\":false}}\n{{\"message\":{{\"role\":\"assistant\",\"content\":\"\"}},\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}}\n"
    )
}

#[tokio::test]
async fn repl_resolves_elicitation_and_quits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(vec![
        ask_user_response("Which position limit applies?"),
        loopback::text_response("The limit is 42 contracts."),
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
    // One question; the elicitation answer; quit.
    let mut input = std::io::Cursor::new(b"what limit?\n42 contracts\n:q\n".to_vec());
    let mut output = Vec::new();
    super::repl::run_repl(
        &config,
        None,
        "repl-test",
        &mut input,
        &mut output,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .await
    .expect("repl completes");
    server.await.expect("join").expect("served");
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("[interaction] Which position limit applies?"),
        "interaction rendered: {output}"
    );
    assert!(
        output.contains("The limit is 42 contracts."),
        "final answer rendered: {output}"
    );
}

#[tokio::test]
async fn repl_cancel_settles_run_and_loop_survives() {
    let dir = tempfile::tempdir().expect("tempdir");
    // No scripted responses: the cancelled run must never need one.
    let (base_url, server) = loopback::serve_ndjson(Vec::new()).await.expect("loopback");
    let config = crate::KnowledgeConfig::new(
        dir.path().to_path_buf(),
        crate::ProviderChoice::Ollama {
            base_url,
            model: "preview-model".to_owned(),
        },
    );
    let interrupt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mut input = std::io::Cursor::new(b"doomed question\n:session\n:q\n".to_vec());
    let mut output = Vec::new();
    super::repl::run_repl(
        &config,
        None,
        "repl-test",
        &mut input,
        &mut output,
        std::sync::Arc::clone(&interrupt),
    )
    .await
    .expect("loop survives cancellation");
    drop(server);
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("run cancelled") || output.contains("cancelled"),
        "cancellation reported: {output}"
    );
    // The loop kept going: `:session` printed the id again after the cancel.
    let id = output
        .lines()
        .find_map(|line| line.strip_prefix("session: "))
        .expect("session banner");
    assert!(output.matches(id).count() >= 2, "loop survived: {output}");
}

#[tokio::test]
async fn sessions_list_show_and_name_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (base_url, server) = loopback::serve_ndjson(vec![
        loopback::text_response("answer one"),
        loopback::text_response("answer two"),
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
    // Two sessions via ask.
    let mut sink = TextRenderer::new();
    let first = super::ask::run_ask(&config, "one?", None, "sess-test", &mut sink)
        .await
        .expect("first");
    let mut sink = TextRenderer::new();
    let second = super::ask::run_ask(&config, "two?", None, "sess-test", &mut sink)
        .await
        .expect("second");
    server.await.expect("join").expect("served");

    // list shows both.
    let listing = super::sessions::run_list(&config).await.expect("list");
    assert!(
        listing.contains(&first.session_id),
        "list has first: {listing}"
    );
    assert!(
        listing.contains(&second.session_id),
        "list has second: {listing}"
    );

    // name round-trips into the listing.
    super::sessions::run_name(&config, &first.session_id, "quarterly-review")
        .await
        .expect("name");
    let listing = super::sessions::run_list(&config)
        .await
        .expect("list again");
    assert!(listing.contains("quarterly-review"), "named: {listing}");

    // show renders the main lane history with the question text.
    let shown = super::sessions::run_show(&config, &second.session_id)
        .await
        .expect("show");
    assert!(shown.contains("main"), "lane name shown: {shown}");
    assert!(shown.contains("two?"), "user turn shown: {shown}");
    assert!(
        shown.contains("answer two"),
        "assistant turn shown: {shown}"
    );

    // Unknown id is a config/compose error (exit 2 at the bin).
    assert!(
        super::sessions::run_show(&config, "definitely-not-an-id")
            .await
            .is_err()
    );

    // Renaming again replaces the name (CAS against the fresh head).
    super::sessions::run_name(&config, &first.session_id, "renamed-again")
        .await
        .expect("rename");
    let listing = super::sessions::run_list(&config)
        .await
        .expect("list third");
    assert!(listing.contains("renamed-again"), "renamed: {listing}");
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
    assert!(
        kinds.iter().any(|kind| kind == "run_completed"),
        "kinds: {kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| kind == "model_text_delta"),
        "kinds: {kinds:?}"
    );
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
        Command::Sessions {
            command: SessionsCommand::List
        }
    ));
    let show =
        Cli::try_parse_from(["finstack-know", "sessions", "show", "id-1"]).expect("show parses");
    match show.command {
        Command::Sessions {
            command: SessionsCommand::Show { id },
        } => assert_eq!(id, "id-1"),
        other => panic!("expected show, got {other:?}"),
    }
    let name = Cli::try_parse_from(["finstack-know", "sessions", "name", "id-1", "quarterly"])
        .expect("name parses");
    match name.command {
        Command::Sessions {
            command: SessionsCommand::Name { id, name },
        } => {
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
    assert_eq!(
        cli.data_dir.as_deref(),
        Some(std::path::Path::new("/tmp/kd"))
    );
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
