//! Toolset identity, specs, and construction.

use std::sync::Arc;

use finstack_ai_kernel::{
    ArtifactRef, ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId,
    ValidatedToolCall,
};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::{
    ArtifactError, ArtifactMetadata, ArtifactStore, stage_required_artifact,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    PendingToolEffect, ToolCallContext, ToolError, ToolEventStream, ToolReconcileResult,
    ToolResult, ToolStreamItem, Toolset, ToolsetDescriptor, verify_authority,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::parser::{DocumentClassification, DocumentFormat, DocumentLimits, ParsedDocument};

/// Distinct namespaced tool ids. [`ToolCatalog`](finstack_ai_runtime) requires
/// every registered `ToolSpec::id` to be globally unique, so each tool gets
/// its own id even though both live in one toolset (mirrors
/// `finstack-ai-tools-filesystem`'s per-tool ids).
pub(crate) const PARSE_TOOL_ID: &str = "finstack.tools.document.parse";
pub(crate) const CLASSIFY_TOOL_ID: &str = "finstack.tools.document.classify";
pub(crate) const PARSE_NAME: &str = "document_parse";
pub(crate) const CLASSIFY_NAME: &str = "pdf_classify";

/// Input schemas list every property in `required` and make optional
/// properties nullable. Providers that send tools with `strict: true` (the
/// OpenAI/OpenRouter Responses endpoints) reject any object schema whose
/// `required` array omits a declared property.
const PARSE_INPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference JSON for the document bytes.","type":"object"},"media_type_hint":{"type":["string","null"]},"page_range":{"description":"1-based inclusive page range, PDFs only.","items":{"minimum":1,"type":"integer"},"maxItems":2,"minItems":2,"type":["array","null"]},"max_output_bytes":{"description":"Per-call output ceiling, clamped to the configured limit.","minimum":1,"type":["integer","null"]}},"required":["artifact","media_type_hint","page_range","max_output_bytes"],"type":"object"}"#;
const PARSE_OUTPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"markdown":{"type":"string"},"format":{"type":"string"},"page_count":{"type":["integer","null"]},"classification":{"type":["string","null"]},"requires_ocr":{"type":"boolean"},"truncated":{"type":"boolean"},"spilled_artifact":{"type":["object","null"]}},"required":["markdown","format","requires_ocr","truncated"],"type":"object"}"#;
const CLASSIFY_INPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference JSON for the PDF bytes.","type":"object"}},"required":["artifact"],"type":"object"}"#;
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
    pub(crate) limits: DocumentLimits,
    pub(crate) artifact_store: Arc<dyn ArtifactStore>,
}

impl std::fmt::Debug for DocumentToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentToolset")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DocumentToolset {
    /// Construct the toolset with default limits over the required artifact store.
    ///
    /// # Errors
    ///
    /// Returns a configuration error only if a checked-in identity or schema
    /// constant is invalid.
    pub fn try_new(artifact_store: Arc<dyn ArtifactStore>) -> Result<Self, DocumentError> {
        Self::try_new_with_limits(artifact_store, DocumentLimits::default())
    }

    /// Construct with explicit limits.
    ///
    /// A `try_new` variant, not a builder step: it takes no `self`. The
    /// `try_with_limits` name is reserved for the consuming-builder form
    /// used by the filesystem and shell toolsets.
    ///
    /// # Errors
    ///
    /// Returns a configuration error only if a checked-in identity or schema
    /// constant is invalid.
    pub fn try_new_with_limits(
        artifact_store: Arc<dyn ArtifactStore>,
        limits: DocumentLimits,
    ) -> Result<Self, DocumentError> {
        let parse_id = ToolId::parse(PARSE_TOOL_ID).map_err(|_| DocumentError::Configuration {
            reason: "invalid_tool_id",
        })?;
        let classify_id =
            ToolId::parse(CLASSIFY_TOOL_ID).map_err(|_| DocumentError::Configuration {
                reason: "invalid_tool_id",
            })?;
        let parse_spec = spec(
            &parse_id,
            PARSE_NAME,
            "Parse Document",
            "Convert an attached document (pdf, docx, xlsx, pptx, odf, rtf, epub, csv) to GitHub-Flavored Markdown. Scanned PDFs succeed with requires_ocr=true.",
            PARSE_INPUT_SCHEMA,
            PARSE_OUTPUT_SCHEMA,
            1_048_576,
        )?;
        let classify_spec = spec(
            &classify_id,
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
            limits,
            artifact_store,
        })
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
            let name = call.call.tool_name();
            let spec = toolset
                .tools
                .iter()
                .find(|spec| spec.model_name.as_ref() == name)
                .ok_or_else(|| invalid_arguments("unknown document tool name"))?;
            validate_call_context(&ctx, &call, &spec.id)?;
            let args = parse_call_arguments(name, call.call.arguments().as_bytes())?;
            if let Some((start, end)) = args.page_range
                && (start == 0 || end < start)
            {
                return Err(invalid_arguments("page_range is reversed or zero-based"));
            }
            let resolved = crate::source::resolve(
                &args.artifact,
                &ctx,
                toolset.artifact_store.as_ref(),
                &toolset.limits,
            )
            .await
            .map_err(|error| source_error(&error))?;
            // `Bytes` is a shared immutable buffer straight off the store;
            // it is moved (refcount bump, no copy) into the parser worker.
            let bytes = resolved.bytes;
            let media_type_hint = args.media_type_hint.unwrap_or(resolved.media_type_hint);

            let output = match name {
                PARSE_NAME => {
                    build_parse_result(
                        &toolset,
                        &ctx,
                        bytes,
                        &media_type_hint,
                        args.page_range,
                        args.max_output_bytes,
                        spec.max_result_bytes,
                    )
                    .await?
                }
                CLASSIFY_NAME => DocumentOutput {
                    bytes: build_classify_result(bytes).await?,
                    artifact: None,
                },
                _ => return Err(invalid_arguments("unknown document tool name")),
            };
            completed_stream(output)
        })
    }

    fn reconcile(
        &self,
        _ctx: finstack_ai_runtime::ports::model::ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        Box::pin(async { Ok(ToolReconcileResult::Unknown) })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParseArguments {
    artifact: serde_json::Value,
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
    artifact: serde_json::Value,
}

fn validate_call_context(
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
    expected_id: &ToolId,
) -> Result<(), ToolError> {
    if call.tool_id != *expected_id {
        return Err(invalid_arguments("document call identity is invalid"));
    }
    verify_authority(ctx)
}

/// One finished tool result: the output JSON serialized exactly once, plus
/// the artifact the output spilled to when it exceeded `max_result_bytes`.
struct DocumentOutput {
    bytes: Vec<u8>,
    artifact: Option<ArtifactRef>,
}

/// Borrowing `document_parse` output shape. Serialized straight from the
/// parsed document so the (possibly megabyte-scale) Markdown is never
/// copied into an intermediate `serde_json::Value`.
#[derive(Serialize)]
struct ParseOutput<'a> {
    markdown: &'a str,
    format: DocumentFormat,
    page_count: Option<u32>,
    classification: Option<DocumentClassification>,
    requires_ocr: bool,
    truncated: bool,
    spilled_artifact: Option<&'a ArtifactRef>,
}

/// `pdf_classify` output shape.
#[derive(Serialize)]
struct ClassifyOutput {
    classification: DocumentClassification,
    page_count: u32,
    /// Reserved: the classifier does not expose per-page stats yet.
    page_stats: Option<Vec<serde_json::Value>>,
}

/// Deserialize the per-tool argument shape; `pdf_classify` takes only the
/// artifact, so its parse-only fields come back unset.
fn parse_call_arguments(name: &str, arguments: &[u8]) -> Result<ParseArguments, ToolError> {
    match name {
        PARSE_NAME => serde_json::from_slice(arguments)
            .map_err(|_| invalid_arguments("document_parse arguments are invalid")),
        CLASSIFY_NAME => {
            let arguments: ClassifyArguments = serde_json::from_slice(arguments)
                .map_err(|_| invalid_arguments("pdf_classify arguments are invalid"))?;
            Ok(ParseArguments {
                artifact: arguments.artifact,
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
    bytes: Bytes,
    media_type_hint: &str,
    page_range: Option<(u32, u32)>,
    max_output_bytes: Option<u64>,
    max_result_bytes: u64,
) -> Result<DocumentOutput, ToolError> {
    let mut limits = toolset.limits.clone();
    if let Some(requested) = max_output_bytes {
        limits.max_output_bytes = limits.max_output_bytes.min(requested);
    }
    if page_range.is_some() && !bytes.starts_with(b"%PDF-") {
        return Err(invalid_arguments(
            "page_range is only valid for PDF documents",
        ));
    }
    let parsed = parse_document(bytes, media_type_hint, page_range, limits).await?;
    build_parse_output(parsed, ctx, &toolset.artifact_store, max_result_bytes).await
}

async fn build_classify_result(bytes: Bytes) -> Result<Vec<u8>, ToolError> {
    #[cfg(not(target_arch = "wasm32"))]
    let (classification, page_count) =
        tokio::task::spawn_blocking(move || crate::parser::classify_pdf(&bytes))
            .await
            .map_err(|_| parse_worker_error())?
            .map_err(|error| parse_error(&error))?;
    #[cfg(target_arch = "wasm32")]
    let (classification, page_count) =
        crate::parser::classify_pdf(&bytes).map_err(|error| parse_error(&error))?;
    serialize_result(&ClassifyOutput {
        classification,
        page_count,
        page_stats: None,
    })
}

fn invalid_arguments(message: &'static str) -> ToolError {
    tool_error(
        crate::DOCUMENT_INVALID_ARGUMENTS,
        ErrorCategory::Validation,
        message,
    )
}

fn source_error(error: &crate::source::SourceError) -> ToolError {
    use crate::source::SourceError;
    match *error {
        SourceError::InvalidArguments(message) => tool_error(
            crate::DOCUMENT_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            message,
        ),
        SourceError::Unavailable(message) => tool_error(
            crate::DOCUMENT_SOURCE_UNAVAILABLE,
            ErrorCategory::Tool,
            message,
        ),
        SourceError::TooLarge => tool_error(
            crate::DOCUMENT_TOO_LARGE,
            ErrorCategory::Validation,
            "document input exceeds the byte ceiling",
        ),
    }
}

/// Run the parser off the async thread on native hosts (the shared `Bytes`
/// handle moves into the worker without copying); inline on `wasm32`.
async fn parse_document(
    bytes: Bytes,
    media_type_hint: &str,
    page_range: Option<(u32, u32)>,
    limits: DocumentLimits,
) -> Result<ParsedDocument, ToolError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let media_type_hint = media_type_hint.to_owned();
        tokio::task::spawn_blocking(move || match page_range {
            Some(range) => crate::parser::parse_pages(&bytes, range, &limits),
            None => crate::parser::parse(&bytes, Some(&media_type_hint), &limits),
        })
        .await
        .map_err(|_| parse_worker_error())?
        .map_err(|error| parse_error(&error))
    }
    #[cfg(target_arch = "wasm32")]
    match page_range {
        Some(range) => crate::parser::parse_pages(&bytes, range, &limits),
        None => crate::parser::parse(&bytes, Some(media_type_hint), &limits),
    }
    .map_err(|error| parse_error(&error))
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_worker_error() -> ToolError {
    tool_error(
        crate::DOCUMENT_PARSE_FAILED,
        ErrorCategory::Internal,
        "document parser worker failed",
    )
}

fn parse_error(error: &crate::parser::DocumentParseError) -> ToolError {
    use crate::parser::DocumentParseError;
    let (code, message) = match error {
        DocumentParseError::TooLarge { .. } => (
            crate::DOCUMENT_TOO_LARGE,
            "document exceeds size or page ceiling",
        ),
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

/// Map an output-spill staging failure. An artifact-store size rejection is
/// a limit the caller can act on (shrink `max_output_bytes`); every other
/// store failure means the artifact store is unavailable for this call.
fn staging_error(error: &ArtifactError) -> ToolError {
    match error {
        ArtifactError::TooLarge { .. } => tool_error(
            crate::DOCUMENT_TOO_LARGE,
            ErrorCategory::Limit,
            "document output exceeds the artifact store ceiling",
        ),
        _ => tool_error(
            crate::DOCUMENT_SOURCE_UNAVAILABLE,
            ErrorCategory::Tool,
            "document artifact staging failed",
        ),
    }
}

/// Serialize one output shape exactly once.
fn serialize_result<T: Serialize>(output: &T) -> Result<Vec<u8>, ToolError> {
    serde_json::to_vec(output).map_err(|_| {
        tool_error(
            crate::DOCUMENT_PARSE_FAILED,
            ErrorCategory::Internal,
            "document result serialization failed",
        )
    })
}

fn byte_len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// Build the `document_parse` output, spilling to an artifact when the
/// serialized result exceeds `max_result_bytes`. The inline result is
/// serialized once here and handed to the stream as-is.
async fn build_parse_output(
    parsed: ParsedDocument,
    ctx: &ToolCallContext,
    store: &Arc<dyn ArtifactStore>,
    max_result_bytes: u64,
) -> Result<DocumentOutput, ToolError> {
    let bytes = serialize_result(&ParseOutput {
        markdown: &parsed.markdown,
        format: parsed.format,
        page_count: parsed.page_count,
        classification: parsed.classification,
        requires_ocr: parsed.requires_ocr,
        truncated: parsed.truncated,
        spilled_artifact: None,
    })?;
    if byte_len(&bytes) <= max_result_bytes {
        return Ok(DocumentOutput {
            bytes,
            artifact: None,
        });
    }
    let scope = crate::source::call_scope(ctx);
    let artifact = stage_required_artifact(
        store.as_ref(),
        scope,
        Bytes::copy_from_slice(parsed.markdown.as_bytes()),
        ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from("text/markdown"),
            name: Some(Arc::from("document.md")),
            attributes: Metadata::empty(),
        },
    )
    .await
    .map_err(|error| staging_error(&error))?;
    let bytes = spilled_parse_output(&parsed, &artifact, max_result_bytes)?;
    Ok(DocumentOutput {
        bytes,
        artifact: Some(artifact),
    })
}

/// Build the final spilled `document_parse` JSON with inline `markdown`
/// truncated so the *whole* serialized result (including the real,
/// already-staged `spilled_artifact`) fits `max_result_bytes`.
///
/// The naive approach — compute a truncation budget from `max_result_bytes`
/// minus the pre-spill skeleton's size (`spilled_artifact: null`) — under
/// counts the overhead once the real `ArtifactRef` JSON (hundreds of bytes)
/// replaces the `null`, so the truncated result can still exceed
/// `max_result_bytes`. Instead, measure overhead from a skeleton that
/// already carries the real `spilled_artifact`, then shrink the markdown
/// budget and re-measure until the actual serialized size fits (JSON string
/// escaping can also inflate a byte-accurate `str` truncation, so a single
/// pass is not guaranteed to converge on the first try). Each attempt
/// borrows a prefix of the Markdown; nothing is cloned per attempt.
const SPILL_TRUNCATION_ATTEMPTS: u8 = 16;

/// # Errors
///
/// Returns an internal error only when the output shape cannot be
/// serialized.
pub(crate) fn spilled_parse_output(
    parsed: &ParsedDocument,
    spilled_artifact: &ArtifactRef,
    max_result_bytes: u64,
) -> Result<Vec<u8>, ToolError> {
    let build = |markdown: &str| {
        serialize_result(&ParseOutput {
            markdown,
            format: parsed.format,
            page_count: parsed.page_count,
            classification: parsed.classification,
            requires_ocr: parsed.requires_ocr,
            truncated: true,
            spilled_artifact: Some(spilled_artifact),
        })
    };
    let overhead = byte_len(&build("")?);
    let mut budget = max_result_bytes.saturating_sub(overhead);
    for _ in 0..SPILL_TRUNCATION_ATTEMPTS {
        let (candidate_markdown, _) = crate::parser::truncate_str(&parsed.markdown, budget);
        let candidate = build(candidate_markdown)?;
        let candidate_size = byte_len(&candidate);
        if candidate_size <= max_result_bytes || budget == 0 {
            return Ok(candidate);
        }
        // JSON escaping (or the char-boundary rounding in truncate_str)
        // left the candidate over budget; shrink by the exact overshoot
        // (or at least one byte) and re-measure.
        let overshoot = candidate_size - max_result_bytes;
        budget = budget.saturating_sub(overshoot.max(1));
    }
    build("")
}

fn completed_stream(output: DocumentOutput) -> Result<ToolEventStream, ToolError> {
    let result = ToolResult {
        output: RawJson::parse(output.bytes).map_err(|_| {
            tool_error(
                crate::DOCUMENT_PARSE_FAILED,
                ErrorCategory::Internal,
                "document result normalization failed",
            )
        })?,
        is_error: false,
    };
    let mut items = Vec::with_capacity(2);
    if let Some(artifact) = output.artifact {
        items.push(Ok(ToolStreamItem::Artifact(artifact)));
    }
    items.push(Ok(ToolStreamItem::Completed(result)));
    Ok(Box::pin(futures_util::stream::iter(items)) as ToolEventStream)
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
