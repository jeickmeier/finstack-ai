//! Artifact-backed document source resolution.

use std::sync::Arc;

use crate::parser::DocumentLimits;
use finstack_ai_kernel::{ArtifactRef, Sensitivity};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{ArtifactScope, ArtifactStore};
use finstack_ai_runtime::ports::tool::ToolCallContext;

/// Document bytes resolved from a staged artifact.
pub(crate) enum ResolvedSource {
    Bytes {
        /// Zero-copy handle: sliced straight off the artifact store's
        /// `Bytes` returned by the artifact store.
        bytes: Bytes,
        media_type_hint: Option<String>,
        #[allow(dead_code, reason = "carried for future diagnostics/spill naming")]
        name: Option<String>,
    },
}

/// Resolve a staged artifact to bytes, enforcing the size ceiling.
pub(crate) async fn resolve(
    artifact_json: &serde_json::Value,
    ctx: &ToolCallContext,
    store: &Arc<dyn ArtifactStore>,
    limits: &DocumentLimits,
) -> Result<ResolvedSource, SourceError> {
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

/// Source resolution failure.
pub(crate) enum SourceError {
    InvalidArguments(&'static str),
    Unavailable(&'static str),
    #[allow(
        dead_code,
        reason = "the oversized length is diagnostic-only; the mapped tool_error message is a fixed string"
    )]
    TooLarge(usize),
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
