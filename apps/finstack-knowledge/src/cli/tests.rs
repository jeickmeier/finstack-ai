use clap::Parser as _;

use super::args::{Cli, Command, SessionsCommand};

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
