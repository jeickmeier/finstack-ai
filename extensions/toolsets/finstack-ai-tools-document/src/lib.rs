//! Document-to-Markdown ingestion toolset built on anydoc and pdf-inspector.

#![warn(missing_docs)]

pub mod parser;
mod source;
mod toolset;

pub use source::DocumentSource;
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
/// Stable path-source-unsupported-on-target code.
pub const DOCUMENT_PATH_UNSUPPORTED: &str = "document_path_unsupported";

#[cfg(test)]
mod tests;
