//! Artifact-backed document source resolution.

use std::sync::Arc;

use crate::parser::DocumentLimits;
use finstack_ai_kernel::{ArtifactRef, Sensitivity};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{ArtifactScope, ArtifactStore};
use finstack_ai_runtime::ports::tool::ToolCallContext;

/// Document bytes resolved from a staged artifact.
pub(crate) struct ResolvedSource {
    /// Zero-copy handle straight off the artifact store's `Bytes`.
    pub(crate) bytes: Bytes,
    /// The staged blob's declared media type.
    pub(crate) media_type_hint: String,
}

/// Resolve a staged artifact to bytes, enforcing the size ceiling.
pub(crate) async fn resolve(
    artifact_json: &serde_json::Value,
    ctx: &ToolCallContext,
    store: &dyn ArtifactStore,
    limits: &DocumentLimits,
) -> Result<ResolvedSource, SourceError> {
    let artifact: ArtifactRef = serde_json::from_value(artifact_json.clone())
        .map_err(|_| SourceError::InvalidArguments("artifact_reference_invalid"))?;
    let bytes = store
        .get(call_scope(ctx), artifact.clone())
        .await
        .map_err(|_| SourceError::Unavailable("artifact_get_failed"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_input_bytes {
        return Err(SourceError::TooLarge);
    }
    Ok(ResolvedSource {
        bytes,
        media_type_hint: artifact.blob().media_type().to_owned(),
    })
}

/// Source resolution failure.
pub(crate) enum SourceError {
    InvalidArguments(&'static str),
    Unavailable(&'static str),
    TooLarge,
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
