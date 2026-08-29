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
pub mod embedder {}

/// Validated embedding vector type and vector math (populated by a later
/// task).
pub mod vector {}
