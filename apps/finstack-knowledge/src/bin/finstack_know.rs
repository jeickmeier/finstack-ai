//! `finstack-know` binary entry point.
//!
//! Exit codes: 0 success, 1 run failure, 2 configuration/usage error.
//! The binary owns environment reads (data dir, `--api-key-env`) and
//! stdout/stderr; the library never touches either.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser as _;
use finstack_ai_knowledge::cli::args::{Cli, Command, ProviderKind};
use finstack_ai_knowledge::cli::render::{EventSink, JsonRenderer, TextRenderer};
use finstack_ai_knowledge::cli::{ask, docs_command};
use finstack_ai_knowledge::{KnowledgeConfig, KnowledgeError, ProviderChoice, default_data_dir};

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
const DEFAULT_OLLAMA_MODEL: &str = "gemma4:26b";

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(&cli) {
        Ok(code) => code,
        Err(error @ KnowledgeError::Run { .. }) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(cli: &Cli) -> Result<ExitCode, KnowledgeError> {
    match &cli.command {
        Command::Docs { topic } => {
            let rendered = docs_command(topic.as_deref())?;
            println!("{rendered}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Ask(args) => {
            let config = config_from(cli)?;
            let question = args.question.clone();
            let session = args.session.clone();
            runtime()?.block_on(async move {
                if cli.json {
                    let mut sink = JsonRenderer::new();
                    let outcome =
                        ask::run_ask(&config, &question, session.as_deref(), &os_user(), &mut sink)
                            .await?;
                    print!("{}", sink.into_markup());
                    report_session(&outcome);
                    Ok(ExitCode::SUCCESS)
                } else {
                    let mut sink = TextRenderer::new();
                    let outcome =
                        ask::run_ask(&config, &question, session.as_deref(), &os_user(), &mut sink)
                            .await?;
                    print_markup(&sink.into_markup());
                    report_session(&outcome);
                    Ok(ExitCode::SUCCESS)
                }
            })
        }
        Command::Ingest(_) | Command::Sessions { .. } | Command::Repl { .. } => {
            Err(KnowledgeError::Config {
                reason: "command_not_implemented_yet",
            })
        }
    }
}

/// Render accumulated markup to the real terminal (ANSI when attached).
fn print_markup(markup: &str) {
    rich_rust::console::Console::builder()
        .markup(true)
        .build()
        .print(markup);
}

fn report_session(outcome: &ask::AskOutcome) {
    if outcome.created {
        eprintln!("session: {}", outcome.session_id);
    }
}

fn os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "operator".to_owned())
}

fn config_from(cli: &Cli) -> Result<KnowledgeConfig, KnowledgeError> {
    let data_dir = default_data_dir(
        cli.data_dir.clone(),
        std::env::var_os("FINSTACK_KNOW_HOME").map(std::path::PathBuf::from),
        std::env::var_os("HOME").map(std::path::PathBuf::from),
    )?;
    let api_key = || -> Result<String, KnowledgeError> {
        let variable = cli.api_key_env.as_deref().ok_or(KnowledgeError::Config {
            reason: "api_key_env_required",
        })?;
        std::env::var(variable).map_err(|_| KnowledgeError::Config {
            reason: "api_key_env_unset",
        })
    };
    let provider = match cli.provider {
        ProviderKind::Ollama => ProviderChoice::Ollama {
            base_url: cli
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_owned()),
            model: cli
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_owned()),
        },
        ProviderKind::Anthropic => ProviderChoice::Anthropic {
            api_key: api_key()?,
            model: cli.model.clone().ok_or(KnowledgeError::Config {
                reason: "model_required",
            })?,
        },
        ProviderKind::OpenAi => ProviderChoice::OpenAi {
            api_key: api_key()?,
            model: cli.model.clone().ok_or(KnowledgeError::Config {
                reason: "model_required",
            })?,
        },
        ProviderKind::OpenRouter => ProviderChoice::OpenRouter {
            api_key: api_key()?,
            model: cli.model.clone().ok_or(KnowledgeError::Config {
                reason: "model_required",
            })?,
        },
    };
    Ok(KnowledgeConfig::new(data_dir, provider))
}

fn runtime() -> Result<tokio::runtime::Runtime, KnowledgeError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| KnowledgeError::Config {
            reason: "tokio_runtime_unavailable",
        })
}
