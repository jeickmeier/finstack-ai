use std::sync::Arc;

use finstack_ai_kernel::{
    ArtifactRef, BlobRef, ErrorCategory, Metadata, RawJson, RetrySafety, Sensitivity,
    ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::artifact::ArtifactScope;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, ReconcileContext, SideEffectClass, ToolDeferralSupport,
    ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    PendingToolEffect, ToolCallContext, ToolError, ToolEventStream, ToolReconcileResult,
    ToolResult, ToolStreamItem, Toolset, ToolsetDescriptor,
};
use finstack_ai_search_core::SearchError;
use serde::Deserialize;

use crate::{DocumentInput, DocumentSearchSource};

/// Stable namespaced indexing tool identity. Model-facing name: `index_document`.
pub const INDEX_DOCUMENT_TOOL_ID: &str = "finstack.search.index_document";

/// Explicit committed idempotent indexing effect. This tool parses artifacts and
/// atomically writes derived rows/receipt; it never embeds, pins, or changes the
/// artifact system of record. Replaying identical input is safe.
#[derive(Clone)]
pub struct DocumentIndexToolset {
    source: Arc<DocumentSearchSource>,
    tools: Arc<[ToolSpec]>,
}

impl DocumentIndexToolset {
    /// Construct the indexing tool over a host-bound document source.
    ///
    /// # Errors
    /// Rejects invalid checked-in tool identity/schema.
    pub fn try_new(source: Arc<DocumentSearchSource>) -> Result<Self, SearchError> {
        let spec = ToolSpec {
            id: ToolId::parse(INDEX_DOCUMENT_TOOL_ID).map_err(|_| SearchError::invalid("index_document_id"))?,
            model_name: "index_document".into(), title: "Index document".into(),
            description: "Idempotently index an attached artifact for scoped lexical retrieval. Preserves OCR-required and truncated extraction status. Does not perform OCR, change the source, or request embeddings.".into(),
            input_schema: RawJson::parse(br#"{"type":"object","additionalProperties":false,"properties":{"artifact":{"type":"object","description":"Exact artifact reference in the current run scope."},"attachment":{"type":"object","description":"Exact BlobRef of a tenant upload, resolved in the reserved upload scope."}},"oneOf":[{"required":["artifact"]},{"required":["attachment"]}]}"#).map_err(|_| SearchError::invalid("index_document_schema"))?,
            output_schema: None, execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::IdempotentWrite, retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata { requirement: ApprovalRequirement::NotRequired, reason: None, attributes: Metadata::empty() },
            max_result_bytes: 16_384, metadata: Metadata::empty(), deferral: ToolDeferralSupport::Never,
        };
        spec.validate()
            .map_err(|_| SearchError::invalid("index_document_spec"))?;
        Ok(Self {
            source,
            tools: vec![spec].into(),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    artifact: Option<ArtifactRef>,
    attachment: Option<BlobRef>,
}

impl Toolset for DocumentIndexToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        ToolsetDescriptor {
            name: "finstack-document-index".into(),
            metadata: Metadata::empty(),
        }
    }
    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let source = Arc::clone(&self.source);
        Box::pin(async move {
            if call.tool_id.as_str() != INDEX_DOCUMENT_TOOL_ID
                || call.call.tool_name() != "index_document"
                || ctx.run.locator.tenant_scope != source.config.scope.tenant
                || ctx
                    .run
                    .authorization
                    .principal
                    .tenant_scope()
                    .is_some_and(|tenant| tenant != source.config.scope.tenant.as_ref())
            {
                return Err(tool_error(SearchError::SearchScopeDenied));
            }
            if call.call.arguments().as_bytes().len() > 8192 {
                return Err(tool_error(SearchError::invalid("index_document_arguments")));
            }
            let arguments: Arguments = serde_json::from_slice(call.call.arguments().as_bytes())
                .map_err(|_| tool_error(SearchError::invalid("index_document_arguments")))?;
            let run_scope = ArtifactScope {
                tenant_scope: ctx.run.locator.tenant_scope.clone(),
                session_id: ctx.run.locator.session_id,
                run_id: Some(ctx.run.locator.run_id),
                sensitivity: Sensitivity::Internal,
            };
            let input = match (arguments.artifact, arguments.attachment) {
                (Some(artifact), None) => DocumentInput {
                    artifact,
                    artifact_scope: run_scope,
                },
                (None, Some(blob)) => {
                    // Same tenant-bound reserved upload scope used by the SDK's
                    // bindings and DocumentIngestMiddleware. No caller-supplied
                    // tenant/session/sensitivity can broaden this read.
                    let artifact_scope = ArtifactScope {
                        tenant_scope: ctx.run.locator.tenant_scope.clone(),
                        session_id: finstack_ai_kernel::SessionId::from_bytes([0; 16]),
                        run_id: None,
                        sensitivity: Sensitivity::Internal,
                    };
                    let read = tokio::select! {
                        biased;
                        () = ctx.run.cancellation.cancelled() => return Err(tool_error(SearchError::SearchUnavailable)),
                        read = source.artifacts.get_by_blob(artifact_scope.clone(), blob.clone()) => read.map_err(|_| tool_error(SearchError::SearchUnavailable))?,
                    };
                    if read.reference.blob() != &blob {
                        return Err(tool_error(SearchError::SearchScopeDenied));
                    }
                    DocumentInput {
                        artifact: read.reference,
                        artifact_scope,
                    }
                }
                _ => return Err(tool_error(SearchError::invalid("index_document_source"))),
            };
            let report = tokio::select! {
                biased;
                () = ctx.run.cancellation.cancelled() => return Err(ToolError::try_new("document_index_cancelled", ErrorCategory::Cancellation, false, "document indexing cancelled", Metadata::empty()).unwrap_or_else(Into::into)),
                result = source.index_document(input) => result.map_err(tool_error)?,
            };
            let bytes = serde_json::to_vec(&report)
                .map_err(|_| tool_error(SearchError::invalid("document_index_output")))?;
            let output = RawJson::parse(bytes)
                .map_err(|_| tool_error(SearchError::invalid("document_index_output")))?;
            Ok(Box::pin(futures_util::stream::once(async move {
                Ok(ToolStreamItem::Completed(ToolResult {
                    output,
                    is_error: false,
                }))
            })) as ToolEventStream)
        })
    }

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        // The whole operation is a transaction over rebuildable derived rows;
        // repeated source/chunker input returns its already committed receipt.
        Box::pin(async { Ok(ToolReconcileResult::RetrySafe) })
    }
}

#[allow(clippy::needless_pass_by_value)] // Result::map_err consumes source errors.
fn tool_error(error: SearchError) -> ToolError {
    let category = if matches!(error, SearchError::SearchCapacityExceeded { .. }) {
        ErrorCategory::Limit
    } else {
        ErrorCategory::Validation
    };
    ToolError::try_new(
        error.code(),
        category,
        false,
        error.code(),
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}
