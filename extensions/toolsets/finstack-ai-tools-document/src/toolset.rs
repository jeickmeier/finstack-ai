//! Toolset identity, specs, and construction.

use std::sync::Arc;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactMetadata, ArtifactStore, Bytes, PortFuture,
    SideEffectClass, ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream,
    ToolReconcileResult, ToolResult, ToolSpec, ToolStreamItem, ToolsetDescriptor, Toolset,
    PendingToolEffect, stage_required_artifact,
};
use serde::Deserialize;
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
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let toolset = self.clone();
        Box::pin(async move {
            validate_call_context(&ctx, &call, &toolset.tool_id)?;
            let name = call.call.tool_name();
            let args = parse_call_arguments(name, call.call.arguments().as_bytes())?;
            let resolved = crate::source::resolve(
                &args.source,
                &ctx,
                toolset.artifact_store.as_ref(),
                &toolset.limits,
            )
            .await
            .map_err(|error| source_error(&error))?;
            let crate::source::ResolvedSource::Bytes {
                bytes,
                media_type_hint,
                ..
            } = resolved;
            let media_type_hint = args.media_type_hint.or(media_type_hint);

            let output = match name {
                PARSE_NAME => {
                    build_parse_result(
                        &toolset,
                        &ctx,
                        &bytes,
                        media_type_hint.as_deref(),
                        args.page_range,
                        args.max_output_bytes,
                    )
                    .await?
                }
                CLASSIFY_NAME => build_classify_result(&bytes)?,
                _ => return Err(invalid_arguments("unknown document tool name")),
            };
            completed_stream(&output)
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParseArguments {
    #[serde(default)]
    artifact: Option<serde_json::Value>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    media_type_hint: Option<String>,
    #[serde(default)]
    page_range: Option<(u32, u32)>,
    #[serde(default)]
    max_output_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassifyArguments {
    #[serde(default)]
    artifact: Option<serde_json::Value>,
    #[serde(default)]
    path: Option<String>,
}

fn validate_call_context(
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
    expected_id: &ToolId,
) -> Result<(), ToolError> {
    if call.tool_id != *expected_id {
        return Err(invalid_arguments("document call identity is invalid"));
    }
    let locator_scope = ctx.run.locator.tenant_scope.as_ref();
    if ctx
        .run
        .authorization
        .principal
        .tenant_scope()
        .is_some_and(|scope| scope != locator_scope)
    {
        return Err(invalid_arguments(
            "document principal scope does not match the committed effect",
        ));
    }
    Ok(())
}

/// Arguments common to both tools after per-tool JSON deserialization.
struct CallArguments {
    source: crate::DocumentSource,
    media_type_hint: Option<String>,
    page_range: Option<(u32, u32)>,
    max_output_bytes: Option<u64>,
}

fn parse_call_arguments(name: &str, arguments: &[u8]) -> Result<CallArguments, ToolError> {
    match name {
        PARSE_NAME => {
            let arguments: ParseArguments = serde_json::from_slice(arguments)
                .map_err(|_| invalid_arguments("document_parse arguments are invalid"))?;
            Ok(CallArguments {
                source: crate::DocumentSource {
                    artifact: arguments.artifact,
                    path: arguments.path,
                },
                media_type_hint: arguments.media_type_hint,
                page_range: arguments.page_range,
                max_output_bytes: arguments.max_output_bytes,
            })
        }
        CLASSIFY_NAME => {
            let arguments: ClassifyArguments = serde_json::from_slice(arguments)
                .map_err(|_| invalid_arguments("pdf_classify arguments are invalid"))?;
            Ok(CallArguments {
                source: crate::DocumentSource {
                    artifact: arguments.artifact,
                    path: arguments.path,
                },
                media_type_hint: None,
                page_range: None,
                max_output_bytes: None,
            })
        }
        _ => Err(invalid_arguments("unknown document tool name")),
    }
}

/// Run `document_parse`: optional page-range extraction, per-call output
/// clamp, and output-spill staging.
async fn build_parse_result(
    toolset: &DocumentToolset,
    ctx: &ToolCallContext,
    bytes: &[u8],
    media_type_hint: Option<&str>,
    page_range: Option<(u32, u32)>,
    max_output_bytes: Option<u64>,
) -> Result<serde_json::Value, ToolError> {
    let mut limits = toolset.limits.clone();
    if let Some(requested) = max_output_bytes {
        limits.max_output_bytes = limits.max_output_bytes.min(requested);
    }
    if let Some((start, end)) = page_range {
        if start == 0 || end < start {
            return Err(invalid_arguments("page_range is reversed or zero-based"));
        }
        if !bytes.starts_with(b"%PDF-") {
            return Err(invalid_arguments(
                "page_range is only valid for PDF documents",
            ));
        }
    }
    let parsed = match page_range {
        Some(range) => crate::parser::parse_pages(bytes, range, &limits)
            .map_err(|error| parse_error(&error))?,
        None => crate::parser::parse(bytes, media_type_hint, &limits)
            .map_err(|error| parse_error(&error))?,
    };
    let max_result_bytes = toolset
        .tools
        .iter()
        .find(|spec| spec.model_name.as_ref() == PARSE_NAME)
        .map_or(1_048_576, |spec| spec.max_result_bytes);
    build_parse_output(parsed, ctx, toolset.artifact_store.as_ref(), max_result_bytes).await
}

fn build_classify_result(bytes: &[u8]) -> Result<serde_json::Value, ToolError> {
    let (classification, page_count) =
        crate::parser::classify_pdf(bytes).map_err(|error| parse_error(&error))?;
    Ok(serde_json::json!({
        "classification": classification,
        "page_count": page_count,
        "page_stats": serde_json::Value::Null,
    }))
}

fn invalid_arguments(message: &'static str) -> ToolError {
    tool_error(crate::DOCUMENT_INVALID_ARGUMENTS, ErrorCategory::Validation, message)
}

fn source_error(error: &crate::source::SourceError) -> ToolError {
    use crate::source::SourceError;
    match *error {
        SourceError::InvalidArguments(message) => {
            tool_error(crate::DOCUMENT_INVALID_ARGUMENTS, ErrorCategory::Validation, message)
        }
        SourceError::Unavailable(message) => {
            tool_error(crate::DOCUMENT_SOURCE_UNAVAILABLE, ErrorCategory::Tool, message)
        }
        SourceError::TooLarge(_) => tool_error(
            crate::DOCUMENT_TOO_LARGE,
            ErrorCategory::Validation,
            "document input exceeds the byte ceiling",
        ),
        SourceError::PathUnsupported => tool_error(
            crate::DOCUMENT_PATH_UNSUPPORTED,
            ErrorCategory::Tool,
            "path sources are unsupported on this target",
        ),
    }
}

fn parse_error(error: &crate::parser::DocumentParseError) -> ToolError {
    use crate::parser::DocumentParseError;
    let (code, message) = match error {
        DocumentParseError::TooLarge { .. } => {
            (crate::DOCUMENT_TOO_LARGE, "document exceeds size or page ceiling")
        }
        DocumentParseError::UnsupportedFormat => (
            crate::DOCUMENT_UNSUPPORTED_FORMAT,
            "no supported document format detected",
        ),
        DocumentParseError::ParseFailed { .. } => {
            (crate::DOCUMENT_PARSE_FAILED, "document parsing failed")
        }
    };
    tool_error(code, ErrorCategory::Tool, message)
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

/// Build the `document_parse` output, spilling to an artifact when the
/// serialized result exceeds `max_result_bytes`.
async fn build_parse_output(
    parsed: crate::parser::ParsedDocument,
    ctx: &ToolCallContext,
    store: Option<&Arc<dyn ArtifactStore>>,
    max_result_bytes: u64,
) -> Result<serde_json::Value, ToolError> {
    let base = serde_json::json!({
        "markdown": parsed.markdown,
        "format": parsed.format,
        "page_count": parsed.page_count,
        "classification": parsed.classification,
        "requires_ocr": parsed.requires_ocr,
        "truncated": parsed.truncated,
        "spilled_artifact": serde_json::Value::Null,
    });
    let size = serde_json::to_vec(&base).map_or(u64::MAX, |bytes| bytes.len() as u64);
    if size <= max_result_bytes {
        return Ok(base);
    }
    let store = store.ok_or_else(|| {
        tool_error(
            crate::DOCUMENT_TOO_LARGE,
            ErrorCategory::Limit,
            "document result requires an artifact store for spill",
        )
    })?;
    let scope = crate::source::call_scope(ctx);
    let markdown_len = u64::try_from(parsed.markdown.len()).unwrap_or(u64::MAX);
    let artifact = stage_required_artifact(
        store.as_ref(),
        scope,
        Bytes::from(parsed.markdown.clone().into_bytes()),
        ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from("text/markdown"),
            name: None,
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(|_| {
        tool_error(
            crate::DOCUMENT_TOO_LARGE,
            ErrorCategory::Tool,
            "document artifact staging failed",
        )
    })?;
    let overhead = size.saturating_sub(markdown_len);
    let budget = max_result_bytes.saturating_sub(overhead);
    let (truncated_markdown, _) = crate::parser::truncate_utf8(parsed.markdown, budget);
    let spilled_artifact = serde_json::to_value(&artifact).map_err(|_| {
        tool_error(
            crate::DOCUMENT_PARSE_FAILED,
            ErrorCategory::Internal,
            "spilled artifact reference serialization failed",
        )
    })?;
    Ok(serde_json::json!({
        "markdown": truncated_markdown,
        "format": parsed.format,
        "page_count": parsed.page_count,
        "classification": parsed.classification,
        "requires_ocr": parsed.requires_ocr,
        "truncated": true,
        "spilled_artifact": spilled_artifact,
    }))
}

fn completed_stream(output: &serde_json::Value) -> Result<ToolEventStream, ToolError> {
    let bytes = serde_json::to_vec(output).map_err(|_| {
        tool_error(
            crate::DOCUMENT_PARSE_FAILED,
            ErrorCategory::Internal,
            "document result serialization failed",
        )
    })?;
    let result = ToolResult {
        output: RawJson::parse(bytes).map_err(|_| {
            tool_error(
                crate::DOCUMENT_PARSE_FAILED,
                ErrorCategory::Internal,
                "document result normalization failed",
            )
        })?,
        is_error: false,
    };
    Ok(Box::pin(futures_util::stream::once(async move {
        Ok(ToolStreamItem::Completed(result))
    })) as ToolEventStream)
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
