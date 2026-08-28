//! The knowledge agent: ingest documents, remember facts across sessions,
//! and answer questions with retrieval and citations.
//!
//! This crate owns the shared agent *definition* — configuration,
//! composition of released components, embedded self-docs, and the
//! golden-questions fixture — consumed by the `finstack-know` CLI, the
//! Python `k`-track notebooks, and the TypeScript browser example. It
//! implements no runtime ports; it only composes existing extensions.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
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

pub mod cli;
mod compose;
mod config;
mod docs;
mod golden;

pub use compose::{build_agent, model_name};
pub use golden::{GoldenEntry, golden_entries};
pub use config::{
    KnowledgeConfig, KnowledgeError, ProviderChoice, default_data_dir, security,
};
pub use docs::{SELF_DOCS, materialize_self_docs};

#[cfg(test)]
mod tests;
