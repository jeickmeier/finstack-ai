//! `Toolset` port implementation: the cached `http_fetch` `ToolSpec`,
//! argument parsing, and call dispatch.
//!
//! `call()` parses arguments and hands them to
//! [`crate::pipeline::execute_fetch`] for the bounded request flow
//! (allowlist/redirect/destination checks, the actual HTTP GET, and the
//! text/markdown/artifact response shaping).

use std::fmt;
use std::sync::Arc;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactStore, PortFuture, SideEffectClass,
    ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream, ToolSpec, ToolStreamItem,
    Toolset, ToolsetDescriptor,
};
use futures_util::stream;
use serde::Deserialize;

use crate::config;
use crate::pipeline::{FetchState, execute_fetch};
use crate::{FETCH_INVALID_ARGUMENTS, FETCH_LIMIT_EXCEEDED, HttpFetchConfig, HttpFetchError};

#[cfg(test)]
use finstack_ai_net_guard::HostResolver;

// `finstack.tools.http_fetch`: verified against
// `finstack_ai_kernel::primitives::ids::validate_key`, which accepts
// `[a-z][a-z0-9._-]*` for the namespaced form, so `_` is fine alongside `.`.
const TOOL_ID: &str = "finstack.tools.http_fetch";
const TOOL_NAME: &str = "http_fetch";
/// Bytes of headroom added on top of `HttpFetchConfig::max_response_bytes`
/// for the `max_result_bytes` ceiling reported on the `ToolSpec` (envelope
/// for the JSON wrapper around the fetched body).
const RESULT_ENVELOPE_BYTES: u64 = 4_096;

/// The kernel's `RawJson::parse` hard-caps every parsed document at this
/// many bytes (`finstack_ai_kernel::RAW_JSON_MAX_BYTES`, currently 1 MiB),
/// independent of whatever this toolset's own `max_response_bytes` is
/// configured to. A local copy is kept (rather than importing the kernel
/// constant directly into the arithmetic below) only so the `min` call
/// reads as a self-contained ceiling computation; the value itself must
/// stay equal to the kernel constant.
const KERNEL_RAW_JSON_MAX_BYTES: u64 = finstack_ai_kernel::RAW_JSON_MAX_BYTES as u64;

/// Compute the serialized-result ceiling from `max_response_bytes`: the same
/// value reported as `ToolSpec::max_result_bytes` and enforced in `call()`
/// against the actual serialized (JSON-escaped) output, so the two can never
/// drift apart.
///
/// Clamped to [`KERNEL_RAW_JSON_MAX_BYTES`]: [`RawJson::parse`] (used below
/// to normalize `call()`'s output) hard-caps at that many bytes regardless
/// of this toolset's own configured limit, so advertising or enforcing a
/// larger ceiling here would let a result pass this toolset's own
/// over-ceiling check only to be rejected by `RawJson::parse` afterwards
/// with a different, less specific error. Clamping means the over-ceiling
/// check in `call()` fires first, with the toolset's own stable
/// `FETCH_LIMIT_EXCEEDED` code, for any serialized output past 1 MiB.
fn result_ceiling(max_response_bytes: usize) -> u64 {
    u64::try_from(max_response_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(RESULT_ENVELOPE_BYTES)
        .min(KERNEL_RAW_JSON_MAX_BYTES)
}

/// Requested output shape for one `http_fetch` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FetchMode {
    /// Let the toolset choose text, markdown, or an artifact.
    #[default]
    Auto,
    /// Return extracted plain text.
    Text,
    /// Return a Markdown rendering.
    Markdown,
    /// Always stage the response as an artifact.
    Artifact,
}

/// Parsed `http_fetch` tool call arguments.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FetchArguments {
    /// URL to fetch; the host must match the toolset's allowlist.
    pub(crate) url: String,
    /// Requested output shape; defaults to [`FetchMode::Auto`].
    #[serde(default)]
    pub(crate) mode: FetchMode,
    /// Optional caller-supplied cap on response bytes, bounded by the
    /// toolset's own `max_response_bytes` ceiling.
    pub(crate) max_bytes: Option<usize>,
}

/// Validated bounded HTTP fetch toolset: parsed allowlist patterns,
/// validated per-host headers, and one cached `ToolSpec`. Construction is
/// fail-closed.
pub struct HttpFetchToolset {
    state: Arc<FetchState>,
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    tool_id: ToolId,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
}

impl HttpFetchToolset {
    /// Validate `config` and construct a toolset with its cached `ToolSpec`.
    ///
    /// # Errors
    ///
    /// Returns [`HttpFetchError::Configuration`] when the allowlist is
    /// empty, any allowlist or per-host-header-key entry fails
    /// [`HostPattern::parse`], a per-host-header key is a wildcard pattern
    /// rather than an exact host, any numeric limit is zero or exceeds its
    /// hard ceiling, any per-host header name/value is not a valid HTTP
    /// header, or a checked-in identity/schema constant is invalid.
    pub fn try_new(config: HttpFetchConfig) -> Result<Self, HttpFetchError> {
        let (config, patterns) = config::validate(config)?;

        let tool_id = ToolId::parse(TOOL_ID).map_err(|_| HttpFetchError::Configuration {
            reason: "invalid_tool_id",
        })?;
        let input_schema = RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"url":{"type":"string"},"mode":{"enum":["auto","text","markdown","artifact"],"type":"string"},"max_bytes":{"minimum":1,"type":"integer"}},"required":["url"],"type":"object"}"#,
        )
        .map_err(|_| HttpFetchError::Configuration {
            reason: "invalid_input_schema",
        })?;

        let max_result_bytes = result_ceiling(config.max_response_bytes);

        let spec = ToolSpec {
            id: tool_id.clone(),
            model_name: Arc::from(TOOL_NAME),
            title: Arc::from("HTTP fetch"),
            description: Arc::from(
                "Fetch one allowlisted HTTPS page and return its content as text, Markdown, or a staged artifact.",
            ),
            input_schema,
            output_schema: None,
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
        spec.validate().map_err(|_| HttpFetchError::Configuration {
            reason: "invalid_tool_spec",
        })?;

        Ok(Self {
            state: Arc::new(FetchState::new(config, patterns)),
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-fetch"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([spec]),
            tool_id,
            artifact_store: None,
        })
    }

    /// Stage fetched content as an artifact instead of inlining it.
    #[must_use]
    pub fn with_artifact_store(mut self, store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// Override the DNS resolver seam with a scripted resolver (tests only).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_resolver(mut self, resolver: Arc<dyn HostResolver>) -> Self {
        let rebuilt = FetchState::new(self.state.config.clone(), self.state.patterns.clone())
            .with_resolver(resolver);
        self.state = Arc::new(rebuilt);
        self
    }
}

impl fmt::Debug for HttpFetchToolset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpFetchToolset")
            .field("config", &self.state.config)
            .field("patterns", &self.state.patterns.len())
            .field("artifact_store", &self.artifact_store.is_some())
            .finish_non_exhaustive()
    }
}

impl Toolset for HttpFetchToolset {
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
        let expected_id = self.tool_id.clone();
        let state = Arc::clone(&self.state);
        let artifact_store = self.artifact_store.clone();
        Box::pin(async move {
            validate_call_context(&ctx, &call, &expected_id)?;
            let arguments: FetchArguments =
                serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
                    tool_error(
                        FETCH_INVALID_ARGUMENTS,
                        ErrorCategory::Validation,
                        "http fetch arguments are invalid",
                    )
                })?;
            let delivered = execute_fetch(&state, &ctx, arguments, artifact_store.as_ref()).await?;
            let output = serde_json::to_vec(&delivered.value).map_err(|_| {
                tool_error(
                    FETCH_INVALID_ARGUMENTS,
                    ErrorCategory::Internal,
                    "http fetch result serialization failed",
                )
            })?;
            // JSON escaping (`\n` -> `\\n`, etc.) can inflate the serialized
            // output past the raw byte budget already enforced on `content`
            // in `deliver.rs`. Re-check the *serialized* size against the
            // same ceiling declared on the ToolSpec so the runtime never
            // hard-rejects with its generic `tool_result_limit_exceeded`
            // instead of our stable `fetch_limit_exceeded`.
            let ceiling = result_ceiling(state.config.max_response_bytes);
            if u64::try_from(output.len()).unwrap_or(u64::MAX) > ceiling {
                return Err(tool_error(
                    FETCH_LIMIT_EXCEEDED,
                    ErrorCategory::Limit,
                    "fetch content exceeds the configured byte limit",
                ));
            }
            let result = finstack_ai_runtime::ToolResult {
                output: RawJson::parse(output).map_err(|_| {
                    tool_error(
                        FETCH_INVALID_ARGUMENTS,
                        ErrorCategory::Internal,
                        "http fetch result normalization failed",
                    )
                })?,
                is_error: false,
            };
            let mut items = Vec::with_capacity(2);
            if let Some(artifact) = delivered.artifact {
                items.push(Ok(ToolStreamItem::Artifact(artifact)));
            }
            items.push(Ok(ToolStreamItem::Completed(result)));
            Ok(Box::pin(stream::iter(items)) as ToolEventStream)
        })
    }
}

fn validate_call_context(
    ctx: &ToolCallContext,
    call: &ValidatedToolCall,
    expected_id: &ToolId,
) -> Result<(), ToolError> {
    if call.tool_id != *expected_id || call.call.tool_name() != TOOL_NAME {
        return Err(tool_error(
            FETCH_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "http fetch call identity is invalid",
        ));
    }
    let locator_scope = ctx.run.locator.tenant_scope.as_ref();
    if ctx
        .run
        .authorization
        .principal
        .tenant_scope()
        .is_some_and(|scope| scope != locator_scope)
    {
        return Err(tool_error(
            FETCH_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            "http fetch principal scope does not match the committed effect",
        ));
    }
    Ok(())
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}
