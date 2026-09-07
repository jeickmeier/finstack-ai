//! Bounded property-graph retrieval backed by scoped live source evidence.
//! Extraction is deterministic and rule-based; model text never defines types
//! or authority. Maintenance reads exact source references before deriving facts.
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

mod database;
mod indexing;
mod rebuild;
mod retrieval;
mod source;
mod vocabulary;
pub use indexing::{GraphIndexReport, GraphReconcileReport};
pub use rebuild::{GraphBuildCursor, GraphBuildReport};
pub use source::{GraphIndexConfig, GraphSearchSource};
pub use vocabulary::{EdgeKind, EdgeRule, EntityRule, GraphVocabulary};
