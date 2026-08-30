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

pub use compose::{
    build_agent, build_agent_with_journal, build_agent_with_stores, model_name,
    open_artifact_store, open_journal, open_journal_sqlite,
};
pub use config::{
    EmbedderChoice, KnowledgeConfig, KnowledgeError, ProviderChoice, default_data_dir, security,
};
pub use docs::{SELF_DOCS, materialize_self_docs};
pub use golden::{GoldenEntry, GoldenMemorySeed, golden_entries};

#[cfg(test)]
mod tests;
