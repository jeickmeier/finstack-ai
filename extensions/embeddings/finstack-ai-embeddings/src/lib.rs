//! Embedding primitives for finstack-ai: validated vectors, the
//! `TextEmbedder` contract, and a deterministic hash reference embedder.
#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

/// Text embedder contract and the deterministic hash reference
/// implementation (populated by a later task).
pub mod embedder {
    use std::sync::Arc;

    use thiserror::Error;

    /// Errors raised by embedding-vector construction and by
    /// [`TextEmbedder`](crate) implementations.
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
}

pub mod vector;

#[cfg(test)]
mod tests;
