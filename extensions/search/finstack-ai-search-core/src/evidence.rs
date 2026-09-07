//! Exact source reads used to extract and revalidate derived graph evidence.
use crate::{SearchError, SearchHit, SearchLimits};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Maximum retained text in one exact evidence read (64 KiB).
pub const MAX_EVIDENCE_BYTES: usize = 65_536;

/// Authoritative bounded source text plus its ordinary citation. A graph reads
/// this through `SearchSource` before extraction and again before serving derived
/// claims. The source verifies liveness and returns its current content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchEvidence {
    /// Exact reference, complete scope, current digest, and sensitivity.
    pub hit: SearchHit,
    /// Source text, never instructions or authority.
    pub text: Arc<str>,
    /// False when only a bounded portion or preview of the source is available.
    pub complete: bool,
}
impl SearchEvidence {
    /// Validate bounds before retaining extraction input.
    ///
    /// # Errors
    /// Rejects excessive text or an invalid citation.
    pub fn validate(&self, limits: &SearchLimits) -> Result<(), SearchError> {
        self.hit.validate(limits)?;
        if self.text.len() > MAX_EVIDENCE_BYTES {
            return Err(SearchError::invalid("evidence_text_bytes"));
        }
        Ok(())
    }
}
