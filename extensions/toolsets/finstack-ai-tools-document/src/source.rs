//! Tool-call source union: staged artifact or native filesystem path.

use std::sync::Arc;

use finstack_ai_kernel::{ArtifactRef, Sensitivity};
use finstack_ai_runtime::{ArtifactScope, ArtifactStore, Bytes, ToolCallContext};
use serde::Deserialize;

use crate::parser::DocumentLimits;

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

/// Document bytes resolved from a [`DocumentSource`].
pub(crate) enum ResolvedSource {
    Bytes {
        /// Zero-copy handle: sliced straight off the artifact store's
        /// `Bytes` for artifact sources, wrapped without copying for path
        /// sources.
        bytes: Bytes,
        media_type_hint: Option<String>,
        #[allow(dead_code, reason = "carried for future diagnostics/spill naming")]
        name: Option<String>,
    },
}

/// Resolve a [`DocumentSource`] to bytes, enforcing the size ceiling.
pub(crate) async fn resolve(
    source: &DocumentSource,
    ctx: &ToolCallContext,
    store: Option<&Arc<dyn ArtifactStore>>,
    limits: &DocumentLimits,
) -> Result<ResolvedSource, SourceError> {
    match (&source.artifact, &source.path) {
        (Some(artifact_json), None) => {
            let store = store.ok_or(SourceError::Unavailable("artifact_store_not_configured"))?;
            let artifact: ArtifactRef = serde_json::from_value(artifact_json.clone())
                .map_err(|_| SourceError::InvalidArguments("artifact_reference_invalid"))?;
            let scope = call_scope(ctx);
            let bytes = store
                .get(scope, artifact.clone())
                .await
                .map_err(|_| SourceError::Unavailable("artifact_get_failed"))?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_input_bytes {
                return Err(SourceError::TooLarge(bytes.len()));
            }
            Ok(ResolvedSource::Bytes {
                bytes,
                media_type_hint: Some(artifact.blob().media_type().to_owned()),
                name: artifact.blob().name().map(str::to_owned),
            })
        }
        (None, Some(path)) => resolve_path(path, limits).await,
        _ => Err(SourceError::InvalidArguments("exactly_one_source_required")),
    }
}

#[cfg(unix)]
#[allow(
    clippy::unused_async,
    reason = "matches the resolve() call site's .await across both cfg(unix) and non-unix stubs"
)]
async fn resolve_path(path: &str, limits: &DocumentLimits) -> Result<ResolvedSource, SourceError> {
    let path = path.to_owned();
    let max = limits.max_input_bytes;
    let bytes = std::fs::metadata(&path)
        .ok()
        .filter(|meta| meta.is_file() && meta.len() <= max)
        .and_then(|_| std::fs::read(&path).ok())
        .ok_or(SourceError::Unavailable("path_unreadable_or_oversized"))?;
    Ok(ResolvedSource::Bytes {
        bytes: Bytes::from(bytes),
        media_type_hint: media_type_hint_from_extension(&path),
        name: std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    })
}

/// Best-effort media-type hint from a path's extension, for formats (csv,
/// rtf) that `anydoc::Format::from_bytes` cannot content-sniff.
#[cfg(unix)]
fn media_type_hint_from_extension(path: &str) -> Option<String> {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)?
        .to_ascii_lowercase();
    let media_type = match extension.as_str() {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ppt" => "application/vnd.ms-powerpoint",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "rtf" => "application/rtf",
        "epub" => "application/epub+zip",
        "csv" => "text/csv",
        _ => return None,
    };
    Some(media_type.to_owned())
}

#[cfg(not(unix))]
async fn resolve_path(
    _path: &str,
    _limits: &DocumentLimits,
) -> Result<ResolvedSource, SourceError> {
    Err(SourceError::PathUnsupported)
}

/// Source resolution failure.
pub(crate) enum SourceError {
    InvalidArguments(&'static str),
    Unavailable(&'static str),
    #[allow(
        dead_code,
        reason = "the oversized length is diagnostic-only; the mapped tool_error message is a fixed string"
    )]
    TooLarge(usize),
    #[cfg_attr(
        unix,
        allow(
            dead_code,
            reason = "only constructed by the non-unix resolve_path stub"
        )
    )]
    PathUnsupported,
}

/// Build the exact scope used for artifact `get`/`stage_put` calls: the
/// committed run's tenant scope, session, and run, with an internal
/// sensitivity classification. Mirrors
/// `finstack-ai-tools-filesystem`'s spill scope construction.
pub(crate) fn call_scope(ctx: &ToolCallContext) -> ArtifactScope {
    ArtifactScope {
        tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
        session_id: ctx.run.locator.session_id,
        run_id: Some(ctx.run.locator.run_id),
        sensitivity: Sensitivity::Internal,
    }
}
