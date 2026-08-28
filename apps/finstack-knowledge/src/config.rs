//! Knowledge-agent configuration, data-dir resolution, and the local
//! security projection.
//!
//! The core stays environment-free: data-dir resolution takes the
//! environment *values* as arguments, and provider API keys arrive as
//! explicit values (a binary reads the variable the user names; the
//! library never touches the environment).

use std::fmt;
use std::path::PathBuf;

use finstack_ai::{PrincipalRef, RunSecurityContext};
use thiserror::Error;

/// Knowledge-agent failure with stable, non-secret reasons.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KnowledgeError {
    /// Configuration or usage is invalid.
    #[error("knowledge_config_invalid: {reason}")]
    Config {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// A frozen identity or security label failed kernel validation.
    #[error("knowledge_identity_invalid: {reason}")]
    Identity {
        /// Bounded non-secret diagnostic.
        reason: String,
    },
    /// A released component rejected its composition inputs.
    #[error("knowledge_compose_failed: {reason}")]
    Compose {
        /// Bounded non-secret diagnostic from the failing component.
        reason: String,
    },
}

/// Which model provider backs the agent.
///
/// `Debug` redacts API keys; only the model name and endpoint survive.
#[derive(Clone)]
#[non_exhaustive]
pub enum ProviderChoice {
    /// Local or remote Ollama endpoint (the default provider).
    Ollama {
        /// Base URL of the Ollama server.
        base_url: String,
        /// Model name to request.
        model: String,
    },
    /// Anthropic API.
    Anthropic {
        /// API key value (never read from the environment by this library).
        api_key: String,
        /// Model name to request.
        model: String,
    },
    /// The `OpenAI` API.
    OpenAi {
        /// API key value (never read from the environment by this library).
        api_key: String,
        /// Model name to request.
        model: String,
    },
    /// The `OpenRouter` API.
    OpenRouter {
        /// API key value (never read from the environment by this library).
        api_key: String,
        /// Model name to request.
        model: String,
    },
}

impl fmt::Debug for ProviderChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ollama { base_url, model } => f
                .debug_struct("Ollama")
                .field("base_url", base_url)
                .field("model", model)
                .finish(),
            Self::Anthropic { model, .. } => f
                .debug_struct("Anthropic")
                .field("api_key", &"<redacted>")
                .field("model", model)
                .finish(),
            Self::OpenAi { model, .. } => f
                .debug_struct("OpenAi")
                .field("api_key", &"<redacted>")
                .field("model", model)
                .finish(),
            Self::OpenRouter { model, .. } => f
                .debug_struct("OpenRouter")
                .field("api_key", &"<redacted>")
                .field("model", model)
                .finish(),
        }
    }
}

/// Everything the knowledge agent needs to build.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct KnowledgeConfig {
    /// Root directory for the journal, self-docs, and memory store.
    pub data_dir: PathBuf,
    /// Model provider selection.
    pub provider: ProviderChoice,
    /// Optional repository root offered as a second instruction source.
    pub project_root: Option<PathBuf>,
    /// Host patterns the fetch toolset may reach; empty disables fetch
    /// entirely. Patterns are validated against the fetch toolset's
    /// grammar when the agent is built.
    pub fetch_allowlist: Vec<String>,
}

impl KnowledgeConfig {
    /// Construct a config with fetch disabled (empty allowlist).
    #[must_use]
    pub fn new(data_dir: PathBuf, provider: ProviderChoice) -> Self {
        Self {
            data_dir,
            provider,
            project_root: None,
            fetch_allowlist: Vec::new(),
        }
    }

    /// Replace the fetch allowlist.
    #[must_use]
    pub fn with_fetch_allowlist(mut self, allowlist: Vec<String>) -> Self {
        self.fetch_allowlist = allowlist;
        self
    }

    /// Offer a repository root as a second instruction source.
    #[must_use]
    pub fn with_project_root(mut self, root: PathBuf) -> Self {
        self.project_root = Some(root);
        self
    }
}

/// Resolve the data directory from an explicit path and environment values.
///
/// Precedence: `explicit`, then `$FINSTACK_KNOW_HOME` (used verbatim), then
/// `$HOME/.finstack-know`. The caller reads the environment and passes the
/// values in; this function performs no reads itself.
///
/// # Errors
///
/// Returns [`KnowledgeError::Config`] when no candidate is available.
pub fn default_data_dir(
    explicit: Option<PathBuf>,
    finstack_know_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<PathBuf, KnowledgeError> {
    if let Some(dir) = explicit {
        return Ok(dir);
    }
    if let Some(dir) = finstack_know_home {
        return Ok(dir);
    }
    if let Some(home) = home {
        return Ok(home.join(".finstack-know"));
    }
    Err(KnowledgeError::Config {
        reason: "data_dir_unresolved",
    })
}

/// Construct the explicit local single-user security projection.
///
/// Tenant `local`, principal from the OS user, auth method `local`,
/// explicit policy/decision labels (the `examples/rust-minimal` pattern).
///
/// # Errors
///
/// Returns [`KnowledgeError::Config`] for a blank user and
/// [`KnowledgeError::Identity`] when kernel validation rejects a label.
pub fn security(os_user: &str) -> Result<RunSecurityContext, KnowledgeError> {
    let user = os_user.trim();
    if user.is_empty() {
        return Err(KnowledgeError::Config {
            reason: "os_user_empty",
        });
    }
    let principal = PrincipalRef::try_new(user, "operator", Some("local"))
        .map_err(|error| KnowledgeError::Identity {
            reason: error.to_string(),
        })?;
    RunSecurityContext::try_new(
        "local",
        principal,
        "local",
        "knowledge-local",
        "know-policy-v1",
        "know-decision-v1",
        None,
    )
    .map_err(|error| KnowledgeError::Identity {
        reason: error.to_string(),
    })
}
