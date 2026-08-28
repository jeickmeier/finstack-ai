//! `finstack-know` argument grammar (clap derive).

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// The knowledge agent: ingest documents, remember facts, answer with
/// citations.
#[derive(Debug, Parser)]
#[command(name = "finstack-know", version, about)]
pub struct Cli {
    /// Data directory (default: `$FINSTACK_KNOW_HOME`, else `$HOME/.finstack-know`).
    #[arg(long, global = true)]
    pub data_dir: Option<PathBuf>,

    /// Emit NDJSON run events on stdout instead of human text.
    #[arg(long, global = true)]
    pub json: bool,

    /// Model provider.
    #[arg(long, global = true, value_enum, default_value_t = ProviderKind::Ollama)]
    pub provider: ProviderKind,

    /// Model name (provider-specific default when omitted).
    #[arg(long, global = true)]
    pub model: Option<String>,

    /// Base URL for the ollama provider.
    #[arg(long, global = true)]
    pub base_url: Option<String>,

    /// Environment variable to read the provider API key from. The library
    /// never reads the environment; the binary reads this one variable and
    /// passes the value in.
    #[arg(long, global = true)]
    pub api_key_env: Option<String>,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// Provider selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderKind {
    /// Local or remote Ollama endpoint (default).
    Ollama,
    /// Anthropic API (requires `--api-key-env`).
    Anthropic,
    /// `OpenAI` API (requires `--api-key-env`).
    #[value(name = "openai")]
    OpenAi,
    /// `OpenRouter` API (requires `--api-key-env`).
    #[value(name = "openrouter")]
    OpenRouter,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Ask one question (creates a session unless `--session` is given).
    Ask(AskArgs),
    /// Ingest a document into a session.
    Ingest(IngestArgs),
    /// Inspect and label sessions in the shared journal.
    Sessions {
        /// Session operation.
        #[command(subcommand)]
        command: SessionsCommand,
    },
    /// Interactive loop on one session.
    Repl {
        /// Session id to continue (default: create a new session).
        #[arg(long)]
        session: Option<String>,
    },
    /// Print bundled self-doc topics (no provider or data dir needed).
    Docs {
        /// Topic name; omit to list topics.
        topic: Option<String>,
    },
}

/// Arguments for `ask`.
#[derive(Debug, clap::Args)]
pub struct AskArgs {
    /// The question.
    pub question: String,
    /// Session id to continue (default: create a new session and print its id).
    #[arg(long)]
    pub session: Option<String>,
}

/// Arguments for `ingest`.
#[derive(Debug, clap::Args)]
pub struct IngestArgs {
    /// Path of the document to ingest.
    pub path: PathBuf,
    /// Session id to ingest into (default: create a new session).
    #[arg(long)]
    pub session: Option<String>,
}

/// `sessions` subcommands.
#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    /// List sessions in the journal.
    List,
    /// Render one session's lanes and history.
    Show {
        /// Session id.
        id: String,
    },
    /// Name a session (compare-and-swap on the journal metadata).
    Name {
        /// Session id.
        id: String,
        /// Human-readable name.
        name: String,
    },
}
