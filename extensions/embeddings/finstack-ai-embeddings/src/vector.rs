//! Validated embedding vector type and deterministic vector math.
//!
//! Every arithmetic helper here accumulates in input order with no
//! reassociation, so identical inputs produce bit-identical outputs on every
//! target. Float comparisons go through [`f32::to_bits`]; construction bans
//! non-finite components so bit-pattern equality is sound.

use std::sync::Arc;

use crate::embedder::EmbedError;

/// Maximum number of components accepted by [`EmbeddingVector::try_new`].
pub const EMBEDDING_MAX_DIMENSIONS: usize = 4096;

/// A validated, immutable embedding vector.
///
/// Construction rejects empty, oversized (more than
/// [`EMBEDDING_MAX_DIMENSIONS`] components), non-finite, and all-zero
/// component sets, so every value has a well-defined nonzero Euclidean norm
/// and unit normalization is always defined.
///
/// Equality and hashing are defined over the exact component bit patterns
/// ([`f32::to_bits`]) — sound once NaN and infinity are banned — which lets
/// containing types keep a derived `Eq`. Note that `+0.0` and `-0.0` are
/// therefore *distinct*.
///
/// # Examples
///
/// ```
/// use finstack_ai_embeddings::vector::EmbeddingVector;
///
/// let vector = EmbeddingVector::try_new(vec![3.0, 4.0]).expect("valid");
/// let unit = vector.unit_normalized();
/// assert_eq!(unit.as_slice(), &[0.6, 0.8]);
/// ```
#[derive(Debug, Clone)]
pub struct EmbeddingVector(Arc<[f32]>);

impl EmbeddingVector {
    /// Validate `components` into an embedding vector.
    ///
    /// # Errors
    ///
    /// Returns [`EmbedError::InvalidInput`] with a stable reason when
    /// `components` is empty (`embedding_components_empty`), longer than
    /// [`EMBEDDING_MAX_DIMENSIONS`] (`embedding_dimensions_exceeded`),
    /// contains a NaN or infinite component
    /// (`embedding_component_not_finite`), or is entirely zero
    /// (`embedding_zero_vector` — a zero norm makes unit normalization
    /// undefined).
    pub fn try_new(components: Vec<f32>) -> Result<Self, EmbedError> {
        if components.is_empty() {
            return Err(EmbedError::InvalidInput {
                reason: "embedding_components_empty",
            });
        }
        if components.len() > EMBEDDING_MAX_DIMENSIONS {
            return Err(EmbedError::InvalidInput {
                reason: "embedding_dimensions_exceeded",
            });
        }
        if components.iter().any(|component| !component.is_finite()) {
            return Err(EmbedError::InvalidInput {
                reason: "embedding_component_not_finite",
            });
        }
        // `abs()` folds -0.0 into +0.0, so this is a sign-insensitive zero
        // test without a float comparison.
        if components
            .iter()
            .all(|component| component.abs().to_bits() == 0)
        {
            return Err(EmbedError::InvalidInput {
                reason: "embedding_zero_vector",
            });
        }
        Ok(Self(Arc::from(components)))
    }

    /// Number of components.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.0.len()
    }

    /// Borrow the components.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    /// This vector scaled to Euclidean norm 1.
    ///
    /// The squared norm accumulates in `f64` in input order (so huge `f32`
    /// components cannot overflow the intermediate sum), and each component
    /// is divided in `f64` before narrowing back. An input whose norm is
    /// exactly 1 is returned unchanged, and renormalizing an already
    /// normalized vector reproduces it, so stored unit vectors do not drift.
    #[must_use]
    pub fn unit_normalized(&self) -> Self {
        let mut sum_of_squares = 0.0_f64;
        for component in self.0.iter() {
            let value = f64::from(*component);
            sum_of_squares += value * value;
        }
        let norm = sum_of_squares.sqrt();
        if norm.to_bits() == 1.0_f64.to_bits() {
            return self.clone();
        }
        let components: Vec<f32> = self
            .0
            .iter()
            .map(|component| {
                // Each |component| <= norm, so the quotient is bounded by 1
                // in magnitude; narrowing to the component type cannot
                // overflow.
                #[allow(clippy::cast_possible_truncation)]
                let normalized = (f64::from(*component) / norm) as f32;
                normalized
            })
            .collect();
        // The largest-magnitude component maps to at least
        // 1/sqrt(dimensions), so the result is finite and nonzero: the
        // construction invariants hold without revalidation.
        Self(Arc::from(components))
    }

    /// Dot product with `other`, or `None` when the dimensions differ.
    ///
    /// Accumulates in `f32` in input order for cross-target determinism.
    /// Finite components can still overflow the accumulation; callers that
    /// need a bounded result should dot unit-normalized vectors, whose
    /// product lies in `[-1, 1]` up to rounding.
    #[must_use]
    pub fn dot(&self, other: &Self) -> Option<f32> {
        if self.0.len() != other.0.len() {
            return None;
        }
        let mut accumulated = 0.0_f32;
        for (left, right) in self.0.iter().zip(other.0.iter()) {
            accumulated += left * right;
        }
        Some(accumulated)
    }
}

impl PartialEq for EmbeddingVector {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .zip(other.0.iter())
                .all(|(left, right)| left.to_bits() == right.to_bits())
    }
}

impl Eq for EmbeddingVector {}

impl core::hash::Hash for EmbeddingVector {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        state.write_usize(self.0.len());
        for component in self.0.iter() {
            state.write_u32(component.to_bits());
        }
    }
}

/// Truncate `text` to at most `max_bytes` bytes at a character boundary.
///
/// A `text` already within the limit is returned unchanged; otherwise the
/// cut backs up to the nearest character boundary, so the result is always
/// valid UTF-8 and never splits a character.
#[must_use]
pub fn truncate_to_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
