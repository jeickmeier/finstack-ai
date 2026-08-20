//! `Toolset` port implementation: the cached `http_fetch` `ToolSpec`,
//! argument parsing, and call dispatch.
//!
//! `execute_fetch` is a stub until Task 7 wires the bounded request flow
//! (allowlist/redirect/destination checks, the actual HTTP GET, and the
//! text/markdown/artifact response shaping).

use std::fmt;
use std::sync::Arc;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactStore, PortFuture, SideEffectClass,
    ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream, ToolResult, ToolSpec,
    ToolStreamItem, Toolset, ToolsetDescriptor,
};
use futures_util::stream;
use serde::Deserialize;

use crate::config::{self, HostPattern};
use crate::{FETCH_INVALID_ARGUMENTS, FETCH_TRANSPORT_FAILED, HttpFetchConfig, HttpFetchError};

// `finstack.tools.http_fetch`: verified against
// `finstack_ai_kernel::primitives::ids::validate_key`, which accepts
// `[a-z][a-z0-9._-]*` for the namespaced form, so `_` is fine alongside `.`.
const TOOL_ID: &str = "finstack.tools.http_fetch";
const TOOL_NAME: &str = "http_fetch";
/// Bytes of headroom added on top of `HttpFetchConfig::max_response_bytes`
/// for the `max_result_bytes` ceiling reported on the `ToolSpec` (envelope
/// for the JSON wrapper around the fetched body).
const RESULT_ENVELOPE_BYTES: u64 = 4_096;

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
///
/// Fields are unread until Task 7 wires `execute_fetch`'s request flow; the
/// struct exists now so argument *parsing* (including `deny_unknown_fields`
/// rejection) is exercised end to end by this task's tests.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
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
    config: HttpFetchConfig,
    patterns: Vec<HostPattern>,
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

        let max_result_bytes = u64::try_from(config.max_response_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(RESULT_ENVELOPE_BYTES);

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
            config,
            patterns,
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
}

impl fmt::Debug for HttpFetchToolset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpFetchToolset")
            .field("config", &self.config)
            .field("patterns", &self.patterns.len())
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
            let result = execute_fetch(arguments)?;
            Ok(Box::pin(stream::once(async move {
                Ok(ToolStreamItem::Completed(result))
            })) as ToolEventStream)
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

/// Stub request flow: real allowlist/redirect/transport handling lands in
/// Task 7.
fn execute_fetch(_arguments: FetchArguments) -> Result<ToolResult, ToolError> {
    Err(tool_error(
        FETCH_TRANSPORT_FAILED,
        ErrorCategory::Tool,
        "http fetch is not wired yet",
    ))
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}
