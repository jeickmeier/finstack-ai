//! Rebuildable document retrieval over scoped artifacts. Lexical indexing is an
//! explicit idempotent tool effect; middleware never writes this index. Exact
//! semantic retrieval requires an explicitly configured embedder. Native only.
#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![doc(test(attr(allow(clippy::expect_used))))]

mod catalog;
mod chunker;
mod database;
mod indexing;
mod retrieval;
mod source;
mod toolset;

pub use chunker::{ChunkerConfig, DocumentChunk, chunk_document};
pub use indexing::{DocumentIndexReport, DocumentInput, DocumentStatus, ReconcileReport};
pub use source::{DocumentIndexConfig, DocumentSearchSource};
pub use toolset::{DocumentIndexToolset, INDEX_DOCUMENT_TOOL_ID};
