//! Bounded source catalogs for explicit host graph construction and rebuilds.
use crate::{SearchError, SourceRef};
use serde::{Deserialize, Serialize};

/// Metadata-only source catalog page. References are candidates, not verified
/// evidence: consumers must call `read_evidence` before deriving facts. Restart
/// from the first page after a source rebuild or concurrent catalog mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceReferencePage {
    /// At most 256 exact references in source-owned stable order.
    pub references: Vec<SourceRef>,
    /// Opaque next-page position, absent at the current end of the catalog.
    pub next_cursor: Option<String>,
}
/// Validate a metadata catalog request before reading any storage.
///
/// # Errors
/// Rejects zero/excessive pages and malformed/oversized opaque cursors.
pub fn validate_catalog_request(cursor: Option<&str>, limit: usize) -> Result<(), SearchError> {
    if limit == 0
        || limit > 256
        || cursor.is_some_and(|c| c.is_empty() || c.len() > 256 || c.contains('\0'))
    {
        return Err(SearchError::invalid("source_catalog_request"));
    }
    Ok(())
}
