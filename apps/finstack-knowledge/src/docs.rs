//! Embedded self-docs and their on-disk materialization.
//!
//! The agent answers questions about finstack-ai itself, offline, by
//! pointing a repository-instructions context provider at a directory of
//! markdown. The sources are embedded here so the binary is
//! self-contained; [`materialize_self_docs`] writes them under the data
//! directory (write-if-changed) for that provider to read.

use std::fs;
use std::path::{Path, PathBuf};

use crate::KnowledgeError;

/// Bundled self-doc topics as `(topic, markdown body)` pairs.
pub const SELF_DOCS: &[(&str, &str)] = &[
    ("architecture", include_str!("../docs/architecture.md")),
    ("sessions", include_str!("../docs/sessions.md")),
    ("memory", include_str!("../docs/memory.md")),
    ("ingestion", include_str!("../docs/ingestion.md")),
    ("cli", include_str!("../docs/cli.md")),
];

/// Materialize the embedded self-docs under `<data_dir>/self-docs/`.
///
/// Each topic becomes `<topic>.md`. Files whose content already matches
/// the embedded body are left untouched (mtimes preserved); missing or
/// drifted files are (re)written. Returns the self-docs root.
///
/// # Errors
///
/// Returns [`KnowledgeError::Config`] when the directory or a doc file
/// cannot be created, read, or written.
pub fn materialize_self_docs(data_dir: &Path) -> Result<PathBuf, KnowledgeError> {
    let root = data_dir.join("self-docs");
    fs::create_dir_all(&root).map_err(|_| KnowledgeError::Config {
        reason: "self_docs_dir_unwritable",
    })?;
    for (topic, body) in SELF_DOCS {
        let path = root.join(format!("{topic}.md"));
        let current = fs::read_to_string(&path).ok();
        if current.as_deref() != Some(*body) {
            fs::write(&path, body).map_err(|_| KnowledgeError::Config {
                reason: "self_docs_write_failed",
            })?;
        }
    }
    Ok(root)
}
