//! Content-type routing and artifact staging for one fetched body (spec
//! §4.1's mode semantics).
//!
//! [`deliver`] decides, per `FetchMode`, whether the body is inlined as
//! text or staged to the [`ArtifactStore`]. The pipeline (`pipeline.rs`)
//! merges the result into the response envelope as an exclusive `content`
//! or `artifact` field.

use std::sync::Arc;

use finstack_ai_kernel::{ErrorCategory, Metadata, Sensitivity};
use finstack_ai_runtime::{
    ArtifactMetadata, ArtifactScope, ArtifactStore, Bytes, ToolCallContext, ToolError,
    stage_required_artifact,
};

use crate::toolset::FetchMode;
use crate::{FETCH_LIMIT_EXCEEDED, FETCH_TRANSPORT_FAILED};

/// Display name given to every staged fetch-body artifact.
const ARTIFACT_NAME: &str = "http-fetch-body";

/// Result of [`deliver`]: exactly one of an inline string or a staged
/// artifact reference, never both.
pub(crate) enum DeliveredContent {
    /// Body decoded and inlined as text (merged into the `content` field).
    Inline(String),
    /// Body staged to the artifact store (merged into the `artifact` field).
    Artifact(serde_json::Value),
}

fn tool_error(code: &'static str, category: ErrorCategory, message: impl Into<String>) -> ToolError {
    ToolError::try_new(code, category, false, message.into(), Metadata::empty()).unwrap_or_else(Into::into)
}

fn binary_without_store_error() -> ToolError {
    tool_error(
        FETCH_LIMIT_EXCEEDED,
        ErrorCategory::Limit,
        "fetch content is binary and requires an artifact store",
    )
}

/// Is `essence` (already lowercased, parameters already stripped) in the
/// inline-text set: `text/*`, exactly `application/json`/`application/xml`,
/// or ending `+json`/`+xml`?
fn is_inline_text_essence(essence: &str) -> bool {
    essence.starts_with("text/")
        || essence == "application/json"
        || essence == "application/xml"
        || essence.ends_with("+json")
        || essence.ends_with("+xml")
}

/// Decide the delivery shape for one fetched body per spec §4.1.
///
/// `media_type` must already be the lowercased Content-Type essence with any
/// `;` parameters stripped (the pipeline's `media_type_of` does this).
/// `max_result_budget` bounds every inline result (`min(config.max_response_bytes,
/// args.max_bytes.unwrap_or(usize::MAX))`, the same effective cap already
/// enforced on the raw read).
///
/// # Errors
///
/// Returns [`ToolError`] with `FETCH_LIMIT_EXCEEDED` when an inline result
/// would exceed `max_result_budget`, when the body is binary (or invalid
/// UTF-8) and no artifact store is attached, or when `mode` is `artifact`
/// and no store is attached. Returns `FETCH_TRANSPORT_FAILED` when artifact
/// staging itself fails.
pub(crate) async fn deliver(
    body: Vec<u8>,
    media_type: &str,
    mode: FetchMode,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    max_result_budget: usize,
) -> Result<DeliveredContent, ToolError> {
    match mode {
        FetchMode::Text => {
            let text = String::from_utf8_lossy(&body).into_owned();
            inline_within_budget(text, max_result_budget)
        }
        FetchMode::Artifact => stage_or_error(body, media_type, store, ctx).await,
        FetchMode::Auto | FetchMode::Markdown => {
            deliver_auto_or_markdown(body, media_type, store, ctx, max_result_budget).await
        }
    }
}

/// `Auto`/`Markdown` routing: inline-text essences (including HTML, for
/// now) decode as UTF-8 text; anything else, or a decode failure, falls
/// through to artifact staging.
async fn deliver_auto_or_markdown(
    body: Vec<u8>,
    media_type: &str,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
    max_result_budget: usize,
) -> Result<DeliveredContent, ToolError> {
    let is_html = media_type == "text/html" || media_type == "application/xhtml+xml";
    if is_html {
        // Task 10: convert HTML to Markdown here instead of falling through
        // to plain inline text below (both `Auto` and `Markdown` route HTML
        // through the converter; `Markdown` additionally forces conversion
        // for `application/xhtml+xml`). Until Task 10 lands, HTML is
        // treated as inline text, same as any other text essence.
    }

    if is_inline_text_essence(media_type) {
        return match String::from_utf8(body) {
            Ok(text) => inline_within_budget(text, max_result_budget),
            Err(err) => stage_or_error(err.into_bytes(), media_type, store, ctx).await,
        };
    }

    stage_or_error(body, media_type, store, ctx).await
}

/// Stage `body` as an artifact, or fail with the shared binary-without-store
/// error when no store is attached (used both for `mode: "artifact"` and
/// for binary/invalid-UTF-8 bodies under `Auto`/`Markdown`).
async fn stage_or_error(
    body: Vec<u8>,
    media_type: &str,
    store: Option<&Arc<dyn ArtifactStore>>,
    ctx: &ToolCallContext,
) -> Result<DeliveredContent, ToolError> {
    let Some(store) = store else {
        return Err(binary_without_store_error());
    };
    stage_artifact(body, media_type, store, ctx)
        .await
        .map(DeliveredContent::Artifact)
}

/// Port of the `stage_required_artifact` call shape from
/// `finstack-ai-tools-openrouter-media/src/http.rs:110-133`.
async fn stage_artifact(
    body: Vec<u8>,
    media_type: &str,
    store: &Arc<dyn ArtifactStore>,
    ctx: &ToolCallContext,
) -> Result<serde_json::Value, ToolError> {
    let artifact = stage_required_artifact(
        store.as_ref(),
        ArtifactScope {
            tenant_scope: Arc::clone(&ctx.run.locator.tenant_scope),
            session_id: ctx.run.locator.session_id,
            run_id: Some(ctx.run.locator.run_id),
            sensitivity: Sensitivity::Internal,
        },
        Bytes::from(body),
        ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from(media_type),
            name: Some(Arc::from(ARTIFACT_NAME)),
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(|_| {
        tool_error(
            FETCH_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "fetch artifact staging failed",
        )
    })?;
    serde_json::to_value(&artifact).map_err(|_| {
        tool_error(
            FETCH_TRANSPORT_FAILED,
            ErrorCategory::Internal,
            "fetch artifact serialization failed",
        )
    })
}

/// Inline `text` unless it exceeds `max_result_budget`.
fn inline_within_budget(text: String, max_result_budget: usize) -> Result<DeliveredContent, ToolError> {
    if text.len() > max_result_budget {
        return Err(tool_error(
            FETCH_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            "fetch content exceeds the configured byte limit",
        ));
    }
    Ok(DeliveredContent::Inline(text))
}
