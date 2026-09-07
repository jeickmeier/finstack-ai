//! Bounded host rebuild over this graph's exact configured source catalogs.
use crate::GraphSearchSource;
use finstack_ai_search_core::{SearchError, validate_catalog_request};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Position in the configured ordered source set; restart with None after source
/// catalog mutation or rebuilding its underlying index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphBuildCursor {
    /// Configured evidence-source namespace.
    pub source: Arc<str>,
    /// Source-owned reference catalog cursor.
    pub reference: Option<String>,
}
/// One bounded graph rebuild step; absent next cursor means the current catalog
/// pass ended, not that future source mutations cannot require another pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphBuildReport {
    /// References whose current facts were indexed/replaced.
    pub indexed: usize,
    /// References removed because their authoritative evidence disappeared.
    pub removed: usize,
    /// Catalogs/references that could not be verified in this pass.
    pub unavailable: usize,
    /// Position for the next bounded step.
    pub next_cursor: Option<GraphBuildCursor>,
}
impl GraphSearchSource {
    /// Rebuild at most `limit` source references from the configured catalogs.
    /// Every candidate is re-read through exact source evidence before extraction.
    /// Unsupported/unavailable catalogs are counted, never treated as complete
    /// source coverage. Reconciliation removes facts for deleted catalog entries.
    ///
    /// # Errors
    /// Rejects unconfigured source cursors, invalid limits or authorization failure.
    pub async fn rebuild_step(
        &self,
        cursor: Option<GraphBuildCursor>,
        limit: usize,
    ) -> Result<GraphBuildReport, SearchError> {
        validate_catalog_request(cursor.as_ref().and_then(|c| c.reference.as_deref()), limit)?;
        if limit > self.config.max_evidence_reads
            || cursor
                .as_ref()
                .is_some_and(|c| !self.sources.contains_key(&c.source))
        {
            return Err(SearchError::invalid("graph_build_cursor"));
        }
        let mut report = GraphBuildReport {
            indexed: 0,
            removed: 0,
            unavailable: 0,
            next_cursor: None,
        };
        let mut remaining = limit;
        let mut active = cursor.is_none();
        for (id, source) in &self.sources {
            if !active && cursor.as_ref().is_some_and(|c| &c.source == id) {
                active = true;
            }
            if !active {
                continue;
            }
            let mut reference = cursor
                .as_ref()
                .filter(|c| &c.source == id)
                .and_then(|c| c.reference.clone());
            loop {
                if remaining == 0 {
                    report.next_cursor = Some(GraphBuildCursor {
                        source: id.clone(),
                        reference,
                    });
                    return Ok(report);
                }
                let page = match tokio::time::timeout(
                    std::time::Duration::from_millis(self.config.source_timeout_ms),
                    source.references(self.config.scope.clone(), reference, remaining),
                )
                .await
                {
                    Ok(Ok(page)) => page,
                    Ok(Err(SearchError::SearchScopeDenied)) => {
                        return Err(SearchError::SearchScopeDenied);
                    }
                    _ => {
                        report.unavailable += 1;
                        break;
                    }
                };
                validate_catalog_request(page.next_cursor.as_deref(), remaining)?;
                if page.references.len() > remaining
                    || page.references.is_empty() && page.next_cursor.is_some()
                {
                    return Err(SearchError::invalid("graph_catalog_result"));
                }
                for reference in page.references {
                    remaining -= 1;
                    match self.index_reference(id, reference).await {
                        Ok(value) => {
                            report.indexed += usize::from(!value.removed);
                            report.removed += usize::from(value.removed);
                        }
                        Err(SearchError::SearchScopeDenied) => {
                            return Err(SearchError::SearchScopeDenied);
                        }
                        Err(_) => report.unavailable += 1,
                    }
                }
                reference = page.next_cursor;
                if reference.is_none() {
                    break;
                }
            }
        }
        Ok(report)
    }
}
