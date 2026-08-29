//! Batch text embedder contract and the deterministic hash reference
//! implementation.

use std::sync::Arc;

use thiserror::Error;

use finstack_ai_runtime::ports::{PortFuture, PortObject};

use crate::vector::{EMBEDDING_MAX_DIMENSIONS, EmbeddingVector};

/// Maximum input bytes accepted by [`HashEmbedder`] per text.
///
/// Deliberately generous: hashing is cheap and local, so the reference
/// embedder never forces callers to truncate realistic memory-sized inputs.
const HASH_EMBEDDER_MAX_INPUT_BYTES: usize = 65_536;

/// Errors raised by embedding-vector construction and by [`TextEmbedder`]
/// implementations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EmbedError {
    /// The input text or vector components failed validation.
    #[error("embed_input_invalid: {reason}")]
    InvalidInput {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// The embedder is unavailable (e.g. a backend outage).
    #[error("embed_unavailable: {message}")]
    Unavailable {
        /// Stable non-secret reason.
        message: Arc<str>,
    },
}

/// Stable, non-secret identity and limits of a [`TextEmbedder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEmbedderDescriptor {
    /// Stable embedder identity: model, revision, and dimensionality (e.g.
    /// `"embed.ollama.nomic-embed-text.768"`).
    ///
    /// The identity names an embedding *space*: vectors are only comparable
    /// within one `embedder_id`. A changed model revision is a new
    /// `embedder_id` — that is, a new space — never a mutation of an
    /// existing one.
    pub embedder_id: Arc<str>,
    /// Number of components in every produced vector.
    pub dimensions: usize,
    /// Maximum accepted input length per text, in bytes. Callers truncate
    /// (at a character boundary) before embedding.
    pub max_input_bytes: usize,
}

/// Batch text embedding: turn texts into same-space [`EmbeddingVector`]s.
///
/// The batch shape serves backfill: one call embeds many pending texts.
/// Output order equals input order, and one invalid text fails the whole
/// batch.
pub trait TextEmbedder: PortObject {
    /// Stable identity and limits of this embedder.
    fn descriptor(&self) -> TextEmbedderDescriptor;

    /// Embed `texts` in order.
    ///
    /// On success the result holds exactly one vector per input text, in
    /// input order, each with [`TextEmbedderDescriptor::dimensions`]
    /// components.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::InvalidInput`] when any text fails validation
    /// and [`EmbedError::Unavailable`] when the embedding backend cannot be
    /// reached; either failure applies to the batch as a whole.
    fn embed(&self, texts: Vec<Arc<str>>) -> PortFuture<Result<Vec<EmbeddingVector>, EmbedError>>;
}

/// Deterministic token-hash bag-of-words embedder at fixed dimensions.
///
/// The reference implementation for tests, examples, and offline golden
/// questions — never a production default: it captures token overlap, not
/// meaning. Texts are lowercased and split into alphanumeric tokens; each
/// token FNV-1a-hashes to a bucket, buckets accumulate in input order, and
/// the result is unit-normalized. Tokenless input (e.g. punctuation only)
/// hashes the trimmed text whole; empty or whitespace-only input is
/// [`EmbedError::InvalidInput`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
///
/// use finstack_ai_embeddings::embedder::{HashEmbedder, TextEmbedder};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let embedder = HashEmbedder::try_new(64).expect("valid dimensions");
/// assert_eq!(embedder.descriptor().embedder_id.as_ref(), "embed.hash-v1.64");
/// let vectors = embedder
///     .embed(vec![Arc::from("hello world")])
///     .await
///     .expect("hash embedding is local and deterministic");
/// assert_eq!(vectors.len(), 1);
/// assert_eq!(vectors[0].dimensions(), 64);
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dimensions: usize,
    embedder_id: Arc<str>,
}

impl HashEmbedder {
    /// Construct a hash embedder producing `dimensions`-component vectors.
    ///
    /// The embedder identity is `"embed.hash-v1.<dimensions>"`: the hashing
    /// scheme is versioned, and each dimensionality is its own embedding
    /// space.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::InvalidInput`] (reason
    /// `embedder_dimensions_invalid`) when `dimensions` is zero or exceeds
    /// [`EMBEDDING_MAX_DIMENSIONS`].
    pub fn try_new(dimensions: usize) -> Result<Self, EmbedError> {
        if dimensions == 0 || dimensions > EMBEDDING_MAX_DIMENSIONS {
            return Err(EmbedError::InvalidInput {
                reason: "embedder_dimensions_invalid",
            });
        }
        Ok(Self {
            dimensions,
            embedder_id: Arc::from(format!("embed.hash-v1.{dimensions}")),
        })
    }

    fn embed_text(&self, text: &str) -> Result<EmbeddingVector, EmbedError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(EmbedError::InvalidInput {
                reason: "embed_input_empty",
            });
        }
        if trimmed.len() > HASH_EMBEDDER_MAX_INPUT_BYTES {
            return Err(EmbedError::InvalidInput {
                reason: "embed_input_too_long",
            });
        }
        let mut components = vec![0.0_f32; self.dimensions];
        let mut hashed_any_token = false;
        for token in tokenize(trimmed) {
            accumulate(&mut components, &token);
            hashed_any_token = true;
        }
        if !hashed_any_token {
            // Tokenless input (punctuation, symbols): hash the trimmed text
            // whole so it still lands on a deterministic nonzero vector.
            accumulate(&mut components, trimmed);
        }
        // At least one bucket accumulated a positive weight, so the vector
        // is nonzero and construction cannot fail on the zero-vector rule.
        EmbeddingVector::try_new(components).map(|vector| vector.unit_normalized())
    }
}

impl TextEmbedder for HashEmbedder {
    fn descriptor(&self) -> TextEmbedderDescriptor {
        TextEmbedderDescriptor {
            embedder_id: Arc::clone(&self.embedder_id),
            dimensions: self.dimensions,
            max_input_bytes: HASH_EMBEDDER_MAX_INPUT_BYTES,
        }
    }

    fn embed(&self, texts: Vec<Arc<str>>) -> PortFuture<Result<Vec<EmbeddingVector>, EmbedError>> {
        let result = texts
            .iter()
            .map(|text| self.embed_text(text))
            .collect::<Result<Vec<_>, _>>();
        Box::pin(async move { result })
    }
}

/// Lowercased alphanumeric tokens of `text`, in input order.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Add one occurrence of `token` to its hash bucket.
fn accumulate(components: &mut [f32], token: &str) {
    let bucket_count = u64::try_from(components.len()).unwrap_or(u64::MAX);
    let bucket = usize::try_from(fnv1a_64(token.as_bytes()) % bucket_count).unwrap_or(0);
    if let Some(slot) = components.get_mut(bucket) {
        *slot += 1.0;
    }
}

/// FNV-1a 64-bit: a fixed, dependency-free hash so vectors are identical
/// across platforms and releases (the scheme is part of `embed.hash-v1`).
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}
