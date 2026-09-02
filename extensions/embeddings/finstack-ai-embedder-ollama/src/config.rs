//! Strict Ollama `/api/embed` transport and identity configuration.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_embeddings::embedder::EmbedError;
use finstack_ai_embeddings::vector::EMBEDDING_MAX_DIMENSIONS;
use reqwest::Url;

/// Path of the Ollama embedding endpoint under the configured base URL.
const DEFAULT_EMBED_PATH: &str = "/api/embed";

/// Whole-request timeout, from connecting through the body read.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

/// Maximum accepted input length per text, in bytes.
pub(crate) const MAX_INPUT_BYTES: usize = 8_192;

/// Maximum accepted model-name length, in bytes.
const MODEL_MAX_BYTES: usize = 256;

/// Strict Ollama `/api/embed` embedder configuration.
///
/// The base URL is an operator-configured fixed endpoint, validated once at
/// construction rather than vetted per request: plaintext HTTP is allowed
/// only toward a loopback IP, and credential, query, and fragment
/// components are rejected outright.
#[derive(Debug, Clone)]
pub struct OllamaEmbedderConfig {
    base_url: Arc<str>,
    pub(crate) model: Arc<str>,
    pub(crate) dimensions: usize,
}

impl OllamaEmbedderConfig {
    /// Construct configuration for one `/api/embed` endpoint and model.
    ///
    /// The `dimensions` value is a contract, not a hint: every response
    /// vector must carry exactly this many components, and it becomes part
    /// of the embedder identity (`embed.ollama.<model>.<dimensions>`).
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::InvalidInput`] with a stable reason when the
    /// base URL is unparsable, has a non-HTTP scheme, carries credentials,
    /// query, or fragment components, or uses plaintext HTTP toward a
    /// non-loopback host (`embedder_base_url_invalid`); when the model name
    /// is empty, oversized, or contains whitespace or control characters
    /// (`embedder_model_invalid`); or when `dimensions` is zero or exceeds
    /// [`EMBEDDING_MAX_DIMENSIONS`] (`embedder_dimensions_invalid`).
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_embedder_ollama::config::OllamaEmbedderConfig;
    ///
    /// let config =
    ///     OllamaEmbedderConfig::try_new("http://127.0.0.1:11434", "nomic-embed-text", 768)
    ///         .expect("config");
    /// assert!(format!("{config:?}").contains("127.0.0.1"));
    /// ```
    pub fn try_new(base_url: &str, model: &str, dimensions: usize) -> Result<Self, EmbedError> {
        validate_base_url(base_url)?;
        if model.is_empty()
            || model.len() > MODEL_MAX_BYTES
            || model
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(EmbedError::InvalidInput {
                reason: "embedder_model_invalid",
            });
        }
        if dimensions == 0 || dimensions > EMBEDDING_MAX_DIMENSIONS {
            return Err(EmbedError::InvalidInput {
                reason: "embedder_dimensions_invalid",
            });
        }
        Ok(Self {
            base_url: Arc::from(base_url),
            model: Arc::from(model),
            dimensions,
        })
    }

    pub(crate) fn endpoint_url(&self) -> Result<Url, EmbedError> {
        let mut url = Url::parse(&self.base_url).map_err(|_| EmbedError::InvalidInput {
            reason: "embedder_base_url_invalid",
        })?;
        url.set_path(DEFAULT_EMBED_PATH);
        Ok(url)
    }
}

/// Reject a base URL whose shape could leak credentials or leave loopback.
fn validate_base_url(value: &str) -> Result<(), EmbedError> {
    let invalid = EmbedError::InvalidInput {
        reason: "embedder_base_url_invalid",
    };
    let Ok(url) = Url::parse(value) else {
        return Err(invalid);
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid);
    }
    if url.scheme() == "http"
        && !url
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|address| address.is_loopback())
    {
        return Err(invalid);
    }
    Ok(())
}
