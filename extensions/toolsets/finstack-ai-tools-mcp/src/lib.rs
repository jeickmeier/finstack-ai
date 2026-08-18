//! Model Context Protocol client implementation of the public `Toolset` port.
//!
//! Protocol revision `2026-07-28`. There is no `initialize` handshake.
//! Sampling, elicitation, and `resources/*` are not implemented.

#![warn(missing_docs)]

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_runtime::{
    Digest, ErrorCategory, Metadata, PendingToolEffect, PortFuture, RawJson, ReconcileContext,
    TOOL_OUTPUT_INVALID, ToolCallContext, ToolError, ToolEventStream, ToolReconcileResult,
    ToolResult, ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor, ValidatedToolCall,
};
use futures_util::stream;
use thiserror::Error;

mod classify;
mod protocol;
mod transport;

#[cfg(test)]
mod tests;

pub use transport::{HttpConfig, StdioConfig};

use classify::{catalog_digest, enumerate_catalog, invocation_digest, to_tool_spec};
use protocol::{CallToolResult, ResultType, content_to_json};
use transport::{HttpTransport, McpTransport, StdioTransport, authorize_http, authorize_stdio};

/// Stable protocol-violation code.
pub const MCP_PROTOCOL_VIOLATION: &str = "mcp_protocol_violation";
/// Stable deny-by-default allowlist code.
pub const MCP_SERVER_NOT_ALLOWLISTED: &str = "mcp_server_not_allowlisted";
/// Stable transport-failure code.
pub const MCP_TRANSPORT_ERROR: &str = "mcp_transport_error";
/// Stable catalog-drift code.
pub const MCP_CATALOG_DRIFT: &str = "mcp_catalog_drift";
/// Stable unsupported-result code.
pub const MCP_RESULT_UNSUPPORTED: &str = "mcp_result_unsupported";
/// Stable output-limit code.
pub const MCP_LIMIT_EXCEEDED: &str = "mcp_limit_exceeded";
/// Stable required-artifact-service code.
pub const MCP_ARTIFACT_REQUIRED: &str = "mcp_artifact_required";

const DEFAULT_INLINE_RESULT_BYTES: u64 = 64 * 1024;

/// Host-supplied MCP client configuration.
///
/// The default is fail-closed: empty allowlists, conservative classification,
/// and approval required for every tool that is not host-declared read-only.
#[derive(Debug, Clone)]
pub struct McpConfig {
    allowed_commands: BTreeSet<Arc<str>>,
    allowed_urls: BTreeSet<Arc<str>>,
    read_only_tools: BTreeSet<Arc<str>>,
    idempotent_tools: BTreeSet<Arc<str>>,
    server: Option<McpServerSpec>,
    inline_result_bytes: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            allowed_commands: BTreeSet::new(),
            allowed_urls: BTreeSet::new(),
            read_only_tools: BTreeSet::new(),
            idempotent_tools: BTreeSet::new(),
            server: None,
            inline_result_bytes: DEFAULT_INLINE_RESULT_BYTES,
        }
    }
}

impl McpConfig {
    /// Allow one stdio program (basename or exact path).
    #[must_use]
    pub fn allow_command(mut self, command: impl AsRef<str>) -> Self {
        self.allowed_commands
            .insert(Arc::<str>::from(command.as_ref()));
        self
    }

    /// Allow one streamable-HTTP URL.
    ///
    /// # Errors
    ///
    /// Rejects an empty URL.
    pub fn allow_url(mut self, url: impl AsRef<str>) -> Result<Self, McpError> {
        let url = url.as_ref();
        if url.is_empty() {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "allowlisted url is empty",
            ));
        }
        self.allowed_urls.insert(Arc::<str>::from(url));
        Ok(self)
    }

    /// Bind an allowlisted stdio server. Catalog freeze happens at construct.
    ///
    /// # Errors
    ///
    /// Rejects a program that is not on the command allowlist.
    pub fn stdio(mut self, config: StdioConfig) -> Result<Self, McpError> {
        authorize_stdio(
            &self.allowed_commands.iter().cloned().collect::<Vec<_>>(),
            &config.program,
        )?;
        self.server = Some(McpServerSpec::Stdio(config));
        Ok(self)
    }

    /// Bind an allowlisted streamable-HTTP server.
    ///
    /// # Errors
    ///
    /// Rejects a URL that is not on the URL allowlist.
    pub fn http(mut self, config: HttpConfig) -> Result<Self, McpError> {
        authorize_http(
            &self.allowed_urls.iter().cloned().collect::<Vec<_>>(),
            config.url(),
        )?;
        self.server = Some(McpServerSpec::Http(config));
        Ok(self)
    }

    /// Host-declared read-only tool names. Server annotations never grant this.
    #[must_use]
    pub fn with_read_only_tools<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.read_only_tools = names
            .into_iter()
            .map(|name| Arc::<str>::from(name.as_ref()))
            .collect();
        self
    }

    /// Host-declared retry-safe tool names. Server annotations never grant this.
    #[must_use]
    pub fn with_idempotent_tools<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.idempotent_tools = names
            .into_iter()
            .map(|name| Arc::<str>::from(name.as_ref()))
            .collect();
        self
    }

    /// Client-side result ceiling. Oversize output is truncated with a marker.
    #[must_use]
    pub fn with_inline_result_bytes(mut self, bytes: u64) -> Self {
        self.inline_result_bytes = bytes.max(1);
        self
    }

    pub(crate) fn is_read_only(&self, name: &str) -> bool {
        self.read_only_tools
            .iter()
            .any(|entry| entry.as_ref() == name)
    }

    pub(crate) fn is_idempotent(&self, name: &str) -> bool {
        self.idempotent_tools
            .iter()
            .any(|entry| entry.as_ref() == name)
            || self.is_read_only(name)
    }

    pub(crate) const fn inline_result_bytes(&self) -> u64 {
        self.inline_result_bytes
    }

    pub(crate) fn is_retry_safe(&self, name: &str) -> bool {
        self.is_read_only(name) || self.is_idempotent(name)
    }

    fn identity(&self) -> String {
        match &self.server {
            Some(McpServerSpec::Stdio(config)) => format!("stdio:{}", config.identity()),
            Some(McpServerSpec::Http(config)) => format!("http:{}", config.url()),
            None => "scripted".to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
enum McpServerSpec {
    Stdio(StdioConfig),
    Http(HttpConfig),
}

/// Construction-time factory. Enumerates and freezes the catalog once.
#[derive(Debug, Clone)]
pub struct McpToolsetFactory {
    config: McpConfig,
}

impl McpToolsetFactory {
    /// Bind one host configuration.
    #[must_use]
    pub const fn new(config: McpConfig) -> Self {
        Self { config }
    }

    /// Construct the toolset. This is the catalog-freeze point.
    ///
    /// Hosts should call this from `ComponentFactory::construct`. Mid-run
    /// `list_changed` notifications are not applied.
    ///
    /// # Errors
    ///
    /// Fails when the server is not allowlisted, transport setup fails, or
    /// `tools/list` violates protocol rules.
    pub async fn construct(&self) -> Result<McpToolset, McpError> {
        let transport: Arc<dyn McpTransport> = match &self.config.server {
            Some(McpServerSpec::Stdio(config)) => Arc::new(StdioTransport::try_spawn(config)?),
            Some(McpServerSpec::Http(config)) => Arc::new(HttpTransport::try_new(config)?),
            None => {
                return Err(McpError::stable(
                    MCP_SERVER_NOT_ALLOWLISTED,
                    "MCP factory requires an allowlisted server",
                ));
            }
        };
        McpToolset::connect(transport, self.config.clone()).await
    }
}

/// Frozen MCP tool catalog backed by one transport.
pub struct McpToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    catalog_digest: Digest,
    invocation_digest: Digest,
    transport: Arc<dyn McpTransport>,
    config: McpConfig,
}

impl McpToolset {
    /// Connect, enumerate `tools/list`, and freeze the catalog.
    ///
    /// # Errors
    ///
    /// Fails on protocol violations, network `$ref`, or unsupported result types.
    pub(crate) async fn connect(
        transport: Arc<dyn McpTransport>,
        config: McpConfig,
    ) -> Result<Self, McpError> {
        let listed = enumerate_catalog(transport.as_ref()).await?;
        let tools = listed
            .iter()
            .map(|tool| to_tool_spec(tool, &config))
            .collect::<Result<Vec<_>, _>>()?;
        let catalog = catalog_digest(&listed);
        let invocation = invocation_digest(&config.identity(), &listed);
        let metadata = Metadata::parse(
            serde_json::to_vec(&serde_json::json!({
                "protocol": protocol::PROTOCOL_VERSION,
                "server": config.identity(),
                "tools": listed.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
                "catalog_digest": catalog.to_string(),
                "invocation_digest": invocation.to_string(),
            }))
            .unwrap_or_else(|_| Vec::from(b"{}")),
        )
        .unwrap_or_else(|_| Metadata::empty());
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-mcp"),
                metadata,
            },
            tools: tools.into(),
            catalog_digest: catalog,
            invocation_digest: invocation,
            transport,
            config,
        })
    }

    /// Frozen catalog digest over `(name, input_schema, output_schema)`.
    #[must_use]
    pub const fn catalog_digest(&self) -> Digest {
        self.catalog_digest
    }

    /// Digest of server identity, command/URL, and the resolved tool-name set.
    #[must_use]
    pub const fn invocation_digest(&self) -> Digest {
        self.invocation_digest
    }
}

impl Toolset for McpToolset {
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
        let transport = Arc::clone(&self.transport);
        let max_bytes = self.config.inline_result_bytes();
        Box::pin(async move {
            let name = call.call.tool_name();
            let arguments: serde_json::Value =
                serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
                    tool_error(
                        MCP_PROTOCOL_VIOLATION,
                        ErrorCategory::Validation,
                        "MCP tool arguments are not json",
                    )
                })?;
            let params = serde_json::json!({
                "name": name,
                "arguments": arguments,
            });
            let value = transport
                .request_identified(
                    serde_json::json!(ctx.run.effect_id.to_string()),
                    "tools/call",
                    params,
                )
                .await
                .map_err(tool_error_from_mcp)?;
            let result: CallToolResult = serde_json::from_value(value).map_err(|_| {
                tool_error(
                    MCP_PROTOCOL_VIOLATION,
                    ErrorCategory::Validation,
                    "tools/call result is invalid",
                )
            })?;
            if result.result_type != ResultType::Complete {
                return Err(tool_error(
                    MCP_RESULT_UNSUPPORTED,
                    ErrorCategory::Validation,
                    "MCP resultType is not complete",
                ));
            }
            if result.is_error {
                return Err(tool_error(
                    TOOL_OUTPUT_INVALID,
                    ErrorCategory::Validation,
                    "MCP tool reported isError",
                ));
            }
            let output = normalize_call_result(&result, max_bytes)?;
            Ok(completed(output, false))
        })
    }

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        let retry_safe = self.config.is_retry_safe(effect.call.call.tool_name());
        Box::pin(async move {
            if retry_safe {
                Ok(ToolReconcileResult::Unknown)
            } else {
                Ok(ToolReconcileResult::NonRepeatable)
            }
        })
    }
}

fn normalize_call_result(result: &CallToolResult, max_bytes: u64) -> Result<RawJson, ToolError> {
    let mut output = serde_json::json!({
        "content": result.content.iter().map(content_to_json).collect::<Vec<_>>(),
    });
    if let Some(structured) = &result.structured_content {
        output
            .as_object_mut()
            .expect("object")
            .insert("structured_content".to_owned(), structured.clone());
    }
    let mut bytes = serde_json::to_vec(&output).map_err(|_| {
        tool_error(
            MCP_TRANSPORT_ERROR,
            ErrorCategory::Internal,
            "MCP result serialization failed",
        )
    })?;
    let max = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    if bytes.len() > max {
        let preview_len = max.saturating_sub(128).min(bytes.len());
        let preview = String::from_utf8_lossy(&bytes[..preview_len]).into_owned();
        output = serde_json::json!({
            "truncated": true,
            "marker": MCP_LIMIT_EXCEEDED,
            "max_result_bytes": max_bytes,
            "preview": preview,
        });
        bytes = serde_json::to_vec(&output).map_err(|_| {
            tool_error(
                MCP_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "MCP truncated marker serialization failed",
            )
        })?;
    }
    RawJson::parse(bytes).map_err(|_| {
        tool_error(
            MCP_TRANSPORT_ERROR,
            ErrorCategory::Internal,
            "MCP result normalization failed",
        )
    })
}

fn completed(output: RawJson, is_error: bool) -> ToolEventStream {
    Box::pin(stream::once(async move {
        Ok(ToolStreamItem::Completed(ToolResult { output, is_error }))
    }))
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

fn tool_error_from_mcp(error: McpError) -> ToolError {
    let category = match error.code {
        MCP_LIMIT_EXCEEDED | MCP_ARTIFACT_REQUIRED => ErrorCategory::Limit,
        MCP_SERVER_NOT_ALLOWLISTED | MCP_TRANSPORT_ERROR => ErrorCategory::Tool,
        _ => ErrorCategory::Validation,
    };
    ToolError::try_new(
        error.code,
        category,
        false,
        error.message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

/// Stable MCP client failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct McpError {
    code: &'static str,
    message: String,
}

impl McpError {
    pub(crate) fn stable(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Safe source-free message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<McpError> for ToolError {
    fn from(error: McpError) -> Self {
        tool_error_from_mcp(error)
    }
}
