//! Ollama `/api/embed` implementation of the batch `TextEmbedder` contract.

use core::fmt;
use std::sync::Arc;

use finstack_ai_embeddings::embedder::{EmbedError, TextEmbedder, TextEmbedderDescriptor};
use finstack_ai_embeddings::vector::EmbeddingVector;
use finstack_ai_runtime::ports::PortFuture;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};

use crate::config::{MAX_INPUT_BYTES, OllamaEmbedderConfig, REQUEST_TIMEOUT};

/// Upper bound on the serialized request payload and on the raw response
/// body, in bytes. A worst-case legitimate backfill batch (64 texts at
/// 4096 dimensions) serializes to roughly 4 MiB, so 16 MiB is generous
/// while still bounding memory before commitment.
const MAX_TRANSFER_BYTES: usize = 16 * 1_048_576;

/// JSON request body of `POST /api/embed`.
#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [Arc<str>],
}

/// JSON response body of `POST /api/embed`; unknown fields are ignored.
#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

/// Reusable Ollama `/api/embed` text embedder.
///
/// One non-streaming JSON POST embeds a whole batch; redirect following is
/// disabled on the pooled HTTP client. Backend failures — transport errors,
/// non-2xx statuses, and malformed or contract-violating response bodies —
/// surface as [`EmbedError::Unavailable`] with a stable, non-secret
/// message, never as partial results.
///
/// # Examples
///
/// ```
/// use finstack_ai_embedder_ollama::config::OllamaEmbedderConfig;
/// use finstack_ai_embedder_ollama::embedder::OllamaEmbedder;
/// use finstack_ai_embeddings::embedder::TextEmbedder;
///
/// let config =
///     OllamaEmbedderConfig::try_new("http://127.0.0.1:11434", "nomic-embed-text", 768)
///         .expect("config");
/// let embedder = OllamaEmbedder::try_new(config).expect("embedder");
/// assert_eq!(
///     embedder.descriptor().embedder_id.as_ref(),
///     "embed.ollama.nomic-embed-text.768"
/// );
/// ```
pub struct OllamaEmbedder {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    descriptor: TextEmbedderDescriptor,
    config: OllamaEmbedderConfig,
}

impl fmt::Debug for OllamaEmbedder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OllamaEmbedder")
            .field("config", &self.config)
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

impl OllamaEmbedder {
    /// Construct one embedder and its reusable pooled HTTP client.
    ///
    /// The embedder identity is `embed.ollama.<model>.<dimensions>`; a
    /// changed model revision is a new identity — a new embedding space.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::Unavailable`] when the HTTP client cannot be
    /// built and [`EmbedError::InvalidInput`] when the configured base URL
    /// no longer parses.
    pub fn try_new(config: OllamaEmbedderConfig) -> Result<Self, EmbedError> {
        let endpoint = config.endpoint_url()?;
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|_| unavailable("ollama embed HTTP client could not be built"))?;
        let descriptor = TextEmbedderDescriptor {
            embedder_id: Arc::from(format!(
                "embed.ollama.{}.{}",
                config.model, config.dimensions
            )),
            dimensions: config.dimensions,
            max_input_bytes: MAX_INPUT_BYTES,
        };
        Ok(Self {
            client,
            endpoint,
            descriptor,
            config,
        })
    }

    /// Validate every text and serialize the batch request body.
    fn prepare_payload(&self, texts: &[Arc<str>]) -> Result<Vec<u8>, EmbedError> {
        for text in texts {
            if text.trim().is_empty() {
                return Err(EmbedError::InvalidInput {
                    reason: "embed_input_empty",
                });
            }
            if text.len() > MAX_INPUT_BYTES {
                return Err(EmbedError::InvalidInput {
                    reason: "embed_input_too_long",
                });
            }
        }
        let payload = serde_json::to_vec(&EmbedRequest {
            model: &self.config.model,
            input: texts,
        })
        .map_err(|_| unavailable("ollama embed request could not be serialized"))?;
        if payload.len() > MAX_TRANSFER_BYTES {
            return Err(EmbedError::InvalidInput {
                reason: "embed_batch_too_large",
            });
        }
        Ok(payload)
    }
}

impl TextEmbedder for OllamaEmbedder {
    fn descriptor(&self) -> TextEmbedderDescriptor {
        self.descriptor.clone()
    }

    fn embed(&self, texts: Vec<Arc<str>>) -> PortFuture<Result<Vec<EmbeddingVector>, EmbedError>> {
        if texts.is_empty() {
            return Box::pin(async { Ok(Vec::new()) });
        }
        let prepared = self.prepare_payload(&texts);
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let expected_count = texts.len();
        let expected_dimensions = self.descriptor.dimensions;
        Box::pin(async move {
            let payload = prepared?;
            let response = client
                .post(endpoint)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(REQUEST_TIMEOUT)
                .body(payload)
                .send()
                .await
                .map_err(|_| unavailable("ollama embed request could not be sent"))?;
            if !response.status().is_success() {
                return Err(unavailable(
                    "ollama embed endpoint returned an unsuccessful status",
                ));
            }
            let body = read_bounded_body(response).await?;
            let parsed: EmbedResponse = serde_json::from_slice(&body)
                .map_err(|_| unavailable("ollama embed response body is not valid JSON"))?;
            if parsed.embeddings.len() != expected_count {
                return Err(unavailable(
                    "ollama embed response vector count does not match the input count",
                ));
            }
            let mut vectors = Vec::with_capacity(parsed.embeddings.len());
            for components in parsed.embeddings {
                if components.len() != expected_dimensions {
                    return Err(unavailable(
                        "ollama embed response vector has the wrong dimensions",
                    ));
                }
                let vector = EmbeddingVector::try_new(components)
                    .map_err(|_| unavailable("ollama embed response vector is invalid"))?;
                vectors.push(vector);
            }
            Ok(vectors)
        })
    }
}

/// Read the whole response body, refusing more than [`MAX_TRANSFER_BYTES`].
async fn read_bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, EmbedError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| unavailable("ollama embed response could not be read"))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_TRANSFER_BYTES {
            return Err(unavailable("ollama embed response exceeded the size limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Stable, non-secret [`EmbedError::Unavailable`] with `message`.
fn unavailable(message: &str) -> EmbedError {
    EmbedError::Unavailable {
        message: Arc::from(message),
    }
}
