//! Toolset identity, specs, and construction.

use std::sync::Arc;

use finstack_ai_kernel::{Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactStore, PortFuture, SideEffectClass,
    ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream, ToolReconcileResult,
    ToolSpec, ToolsetDescriptor, Toolset, PendingToolEffect,
};
use thiserror::Error;

use crate::parser::DocumentLimits;

pub(crate) const TOOL_ID: &str = "finstack.tools.document";
pub(crate) const PARSE_NAME: &str = "document_parse";
pub(crate) const CLASSIFY_NAME: &str = "pdf_classify";

const PARSE_INPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference JSON for the document bytes.","type":"object"},"path":{"description":"Absolute filesystem path (native hosts only).","type":"string"},"media_type_hint":{"type":"string"},"page_range":{"description":"1-based inclusive page range, PDFs only.","items":{"minimum":1,"type":"integer"},"maxItems":2,"minItems":2,"type":"array"},"max_output_bytes":{"description":"Per-call output ceiling, clamped to the configured limit.","minimum":1,"type":"integer"}},"type":"object"}"#;
const PARSE_OUTPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"markdown":{"type":"string"},"format":{"type":"string"},"page_count":{"type":["integer","null"]},"classification":{"type":["string","null"]},"requires_ocr":{"type":"boolean"},"truncated":{"type":"boolean"},"spilled_artifact":{"type":["object","null"]}},"required":["markdown","format","requires_ocr","truncated"],"type":"object"}"#;
const CLASSIFY_INPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference JSON for the PDF bytes.","type":"object"},"path":{"description":"Absolute filesystem path (native hosts only).","type":"string"}},"type":"object"}"#;
const CLASSIFY_OUTPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"classification":{"enum":["text","scanned","mixed","image"],"type":"string"},"page_count":{"type":"integer"},"page_stats":{"description":"Optional per-page text-coverage stats when the classifier exposes them.","items":{"type":"object"},"type":["array","null"]}},"required":["classification","page_count"],"type":"object"}"#;

/// Document toolset construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentError {
    /// A checked-in identity or schema constant is invalid.
    #[error("document_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Document toolset with two immutable cached tool specifications.
#[derive(Clone)]
pub struct DocumentToolset {
    pub(crate) descriptor: ToolsetDescriptor,
    pub(crate) tools: Arc<[ToolSpec]>,
    pub(crate) tool_id: ToolId,
    pub(crate) limits: DocumentLimits,
    pub(crate) artifact_store: Option<Arc<dyn ArtifactStore>>,
}

impl std::fmt::Debug for DocumentToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentToolset")
            .field("tool_id", &self.tool_id)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DocumentToolset {
    /// Construct the toolset with default limits and no artifact store.
    ///
    /// # Errors
    ///
    /// Returns a configuration error only if a checked-in identity or schema
    /// constant is invalid.
    pub fn try_new() -> Result<Self, DocumentError> {
        Self::try_with_limits(DocumentLimits::default())
    }

    /// Construct with explicit limits.
    ///
    /// # Errors
    ///
    /// Returns a configuration error only if a checked-in identity or schema
    /// constant is invalid.
    pub fn try_with_limits(limits: DocumentLimits) -> Result<Self, DocumentError> {
        let tool_id = ToolId::parse(TOOL_ID).map_err(|_| DocumentError::Configuration {
            reason: "invalid_tool_id",
        })?;
        let parse_spec = spec(
            &tool_id,
            PARSE_NAME,
            "Parse Document",
            "Convert an attached document (pdf, docx, xlsx, pptx, odf, rtf, epub, csv) to GitHub-Flavored Markdown. Scanned PDFs succeed with requires_ocr=true.",
            PARSE_INPUT_SCHEMA,
            PARSE_OUTPUT_SCHEMA,
            1_048_576,
        )?;
        let classify_spec = spec(
            &tool_id,
            CLASSIFY_NAME,
            "Classify PDF",
            "Fast PDF classification (text, scanned, mixed, image) with page count; no text extraction.",
            CLASSIFY_INPUT_SCHEMA,
            CLASSIFY_OUTPUT_SCHEMA,
            4_096,
        )?;
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-document"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([parse_spec, classify_spec]),
            tool_id,
            limits,
            artifact_store: None,
        })
    }

    /// Attach the artifact store used for `artifact` sources and output spill.
    #[must_use]
    pub fn with_artifact_store(mut self, store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }
}

impl Toolset for DocumentToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        _ctx: ToolCallContext,
        _call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        // Call dispatch implementation lands in Task 4
        Box::pin(async {
            Err(ToolError::try_new(
                "document_call_not_implemented",
                finstack_ai_kernel::ErrorCategory::Internal,
                false,
                "document_call_not_implemented",
                Metadata::empty(),
            ).unwrap_or_else(Into::into))
        })
    }

    fn reconcile(
        &self,
        _ctx: finstack_ai_runtime::ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        Box::pin(async { Ok(ToolReconcileResult::Unknown) })
    }
}

fn spec(
    tool_id: &ToolId,
    name: &str,
    title: &str,
    description: &str,
    input_schema: &[u8],
    output_schema: &[u8],
    max_result_bytes: u64,
) -> Result<ToolSpec, DocumentError> {
    let spec = ToolSpec {
        id: tool_id.clone(),
        model_name: Arc::from(name),
        title: Arc::from(title),
        description: Arc::from(description),
        input_schema: RawJson::parse(input_schema).map_err(|_| DocumentError::Configuration {
            reason: "invalid_input_schema",
        })?,
        output_schema: Some(RawJson::parse(output_schema).map_err(|_| {
            DocumentError::Configuration {
                reason: "invalid_output_schema",
            }
        })?),
        execution: ToolExecutionMode::Parallel,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate().map_err(|_| DocumentError::Configuration {
        reason: "invalid_tool_spec",
    })?;
    Ok(spec)
}
