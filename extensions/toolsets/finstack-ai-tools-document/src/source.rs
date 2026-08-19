//! Tool-call source union: staged artifact or native filesystem path.

use serde::Deserialize;

/// Where document bytes come from. Exactly one field must be set.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSource {
    /// Staged artifact reference (serialized `ArtifactRef`).
    #[serde(default)]
    pub artifact: Option<serde_json::Value>,
    /// Absolute filesystem path (native hosts only).
    #[serde(default)]
    pub path: Option<String>,
}
