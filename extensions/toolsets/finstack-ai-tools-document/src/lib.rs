//! Document-to-Markdown ingestion toolset built on anydoc and pdf-inspector.

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

pub mod parser;
mod source;
mod toolset;

pub use toolset::{DocumentError, DocumentToolset};

/// Stable invalid-argument code.
pub const DOCUMENT_INVALID_ARGUMENTS: &str = "document_invalid_arguments";
/// Stable parse-failure code.
pub const DOCUMENT_PARSE_FAILED: &str = "document_parse_failed";
/// Stable oversized-input code.
pub const DOCUMENT_TOO_LARGE: &str = "document_too_large";
/// Stable unresolvable-source code.
pub const DOCUMENT_SOURCE_UNAVAILABLE: &str = "document_source_unavailable";
/// Stable unsupported-format code.
pub const DOCUMENT_UNSUPPORTED_FORMAT: &str = "document_unsupported_format";
#[cfg(test)]
mod tests;
