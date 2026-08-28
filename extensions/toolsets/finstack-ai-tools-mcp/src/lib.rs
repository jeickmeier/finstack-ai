//! Model Context Protocol client implementation of the public `Toolset` and
//! `ContextProvider` ports.
//!
//! Protocol revision `2026-07-28`. There is no `initialize` handshake.
//! Sampling is a nested committed model child of the open tool (ADR-046).
//! Elicitation / `input_required` maps onto
//! existing [`InteractionRequest`] and parks the committed tool on durable
//! HITL. An MCP server is not an application-instruction authority.

#![warn(missing_docs)]
// Child-process stdio ownership uses `from_raw_fd` / `from_raw_handle`.
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{Digest, ErrorCategory, Metadata, RawJson, ValidatedToolCall};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{ReconcileContext, ToolSpec};
use finstack_ai_runtime::ports::tool::{
    NestedSample, PendingToolEffect, TOOL_INTERACTION_REQUIRED, TOOL_OUTPUT_INVALID,
    ToolCallContext, ToolError, ToolEventStream, ToolReconcileResult, ToolResult, ToolStreamItem,
    Toolset, ToolsetDescriptor, verify_authority,
};
use futures_util::stream;
use thiserror::Error;

mod classify;
mod interaction;
mod protocol;
mod resources;
mod transport;

#[cfg(test)]
mod tests;

pub use resources::McpContextProvider;
pub use transport::{HttpConfig, StdioConfig};

use classify::{
    catalog_digest, enumerate_catalog, enumerate_prompts, invocation_digest, to_tool_spec,
};
use finstack_ai_kernel::InteractionRequest;
use protocol::{CallToolResult, ResultType, content_to_json};
use transport::{
    HttpTransport, McpTransport, RequestControl, StdioTransport, authorize_http, authorize_stdio,
    validate_http_url,
};

/// Stable protocol-violation code.
pub const MCP_PROTOCOL_VIOLATION: &str = "mcp_protocol_violation";
/// Stable deny-by-default allowlist code.
pub const MCP_SERVER_NOT_ALLOWLISTED: &str = "mcp_server_not_allowlisted";
/// Stable transport-failure code.
pub const MCP_TRANSPORT_ERROR: &str = "mcp_transport_error";
/// Stable cancellation/deadline code for an interrupted MCP request.
pub const MCP_TIMEOUT: &str = "mcp_timeout";
/// Stable catalog-drift code.
pub const MCP_CATALOG_DRIFT: &str = "mcp_catalog_drift";
/// Stable unsupported-result code.
pub const MCP_RESULT_UNSUPPORTED: &str = "mcp_result_unsupported";
/// Stable fail-closed code when `resources/subscribe` names a missing resource.
pub const MCP_SUBSCRIBE_UNKNOWN: &str = "mcp_subscribe_unknown";
/// Stable intercept when `tools/call` asks the host to run `sampling/createMessage`.
pub use finstack_ai_runtime::ports::tool::MCP_SAMPLING_REQUIRED;
/// Stable output-limit code.
pub const MCP_LIMIT_EXCEEDED: &str = "mcp_limit_exceeded";
/// Stable required-artifact-service code.
pub const MCP_ARTIFACT_REQUIRED: &str = "mcp_artifact_required";
/// Stable code when a tool parks on MCP elicitation / `input_required`.
///
/// Aliased to [`TOOL_INTERACTION_REQUIRED`] so the runtime journals
/// `InteractionRequested` instead of failing the committed tool effect.
pub const MCP_INPUT_REQUIRED: &str = TOOL_INTERACTION_REQUIRED;

const DEFAULT_INLINE_RESULT_BYTES: u64 = 64 * 1024;
const CONTEXT_PROVIDER_COMPONENT: &str = "finstack.context.mcp";

/// One prompt frozen from construct-time `prompts/list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPrompt {
    name: Arc<str>,
    text: Arc<str>,
}

impl McpPrompt {
    /// Prompt name from the frozen snapshot.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Untrusted instruction text (`description`, then `title`, then `name`).
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Non-semantic observer for mid-run MCP `list_changed` notifications.
///
/// Implementations must not mutate the frozen catalog or lock.
pub trait McpListChangedObserver: Send + Sync {
    /// Observe one notification method name.
    fn on_list_changed(&self, method: &str);
}

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
    /// Rejects an empty URL, credentials, query, fragment, a non-HTTP
    /// scheme, or plaintext HTTP that is not a literal loopback IP.
    pub fn allow_url(mut self, url: impl AsRef<str>) -> Result<Self, McpError> {
        let url = url.as_ref();
        validate_http_url(url)?;
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

    /// Bind an allowlisted stdio server under [`finstack_ai_runtime::confinement::ProcessConfinement`].
    ///
    /// Requested-and-unavailable fails closed at construct. Unconfined
    /// [`Self::stdio`] stays the T1 path.
    ///
    /// # Errors
    ///
    /// Rejects a program that is not on the command allowlist.
    pub fn stdio_confined(
        self,
        config: StdioConfig,
        confinement: finstack_ai_runtime::confinement::ProcessConfinement,
        profile: finstack_ai_runtime::confinement::ConfinementProfile,
    ) -> Result<Self, McpError> {
        self.stdio(config.with_confinement(confinement, profile))
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

    /// Snapshot one `mcp.json` document at construction. Not a run-time scan.
    ///
    /// Allowlists every listed command or URL. When the document names exactly
    /// one server, that server is bound.
    ///
    /// # Errors
    ///
    /// Rejects invalid JSON, empty server entries, and unsupported keys that
    /// would require a run-time scan.
    pub fn try_from_mcp_json(document: &str) -> Result<Self, McpError> {
        snapshot_mcp_json(document)
    }

    pub(crate) fn identity(&self) -> String {
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

fn snapshot_mcp_json(document: &str) -> Result<McpConfig, McpError> {
    let value: serde_json::Value = serde_json::from_str(document)
        .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "mcp.json is not json"))?;
    let servers = value
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            McpError::stable(MCP_PROTOCOL_VIOLATION, "mcp.json is missing mcpServers")
        })?;
    let mut config = McpConfig::default();
    let mut bound: Option<McpServerSpec> = None;
    for (name, server) in servers {
        let _ = name;
        if server.get("cwd").is_some() || server.get("envFile").is_some() {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "mcp.json run-time scan keys are rejected",
            ));
        }
        if let Some(command) = server.get("command").and_then(serde_json::Value::as_str) {
            if command.is_empty() {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "mcp.json command is empty",
                ));
            }
            let args = server
                .get("args")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            config = config.allow_command(command);
            let spec = McpServerSpec::Stdio(StdioConfig::new(command, args));
            bound = match bound {
                Some(_) => None,
                None => Some(spec),
            };
            continue;
        }
        if let Some(url) = server
            .get("url")
            .or_else(|| server.get("serverUrl"))
            .and_then(serde_json::Value::as_str)
        {
            config = config.allow_url(url)?;
            let spec = McpServerSpec::Http(HttpConfig::try_new(url)?);
            bound = match bound {
                Some(_) => None,
                None => Some(spec),
            };
            continue;
        }
        return Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "mcp.json server needs command or url",
        ));
    }
    if servers.len() == 1 {
        config.server = bound;
    }
    Ok(config)
}

/// Construction-time factory. Enumerates and freezes the catalog once.
#[derive(Clone)]
pub struct McpToolsetFactory {
    config: McpConfig,
    list_changed: Option<Arc<dyn McpListChangedObserver>>,
}

impl std::fmt::Debug for McpToolsetFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpToolsetFactory")
            .field("config", &self.config)
            .field("list_changed", &self.list_changed.is_some())
            .finish()
    }
}

impl McpToolsetFactory {
    /// Bind one host configuration.
    #[must_use]
    pub const fn new(config: McpConfig) -> Self {
        Self {
            config,
            list_changed: None,
        }
    }

    /// Observe `list_changed` without mutating the frozen catalog.
    #[must_use]
    pub fn with_list_changed_observer(mut self, observer: Arc<dyn McpListChangedObserver>) -> Self {
        self.list_changed = Some(observer);
        self
    }

    /// Construct the toolset. This is the catalog-freeze point.
    ///
    /// Hosts should call this from `ComponentFactory::construct`. Mid-run
    /// `list_changed` notifications are observer-only and do not mutate the
    /// frozen catalog.
    ///
    /// # Errors
    ///
    /// Fails when the server is not allowlisted, transport setup fails, or
    /// `tools/list` violates protocol rules.
    pub async fn construct(&self) -> Result<McpToolset, McpError> {
        let transport = self.open_transport()?;
        McpToolset::connect(transport, self.config.clone(), self.list_changed.clone()).await
    }

    /// Enumerate `resources/list` once and freeze the context-provider snapshot.
    ///
    /// Mid-run list changes are ignored. The provider never sets
    /// `trusted_application_instructions`.
    ///
    /// # Errors
    ///
    /// Fails when the server is not allowlisted, transport setup fails, or
    /// `resources/list` violates protocol rules.
    pub async fn construct_context_provider(&self) -> Result<McpContextProvider, McpError> {
        let transport = self.open_transport()?;
        McpContextProvider::connect(transport, &self.config, self.list_changed.clone()).await
    }

    /// Enumerate tools and resources on one shared transport.
    ///
    /// # Errors
    ///
    /// Fails when either catalog freeze fails.
    pub async fn construct_with_context(
        &self,
    ) -> Result<(McpToolset, McpContextProvider), McpError> {
        let transport = self.open_transport()?;
        let toolset = McpToolset::connect(
            Arc::clone(&transport),
            self.config.clone(),
            self.list_changed.clone(),
        )
        .await?;
        let provider =
            McpContextProvider::connect(transport, &self.config, self.list_changed.clone()).await?;
        Ok((toolset, provider))
    }

    fn open_transport(&self) -> Result<Arc<dyn McpTransport>, McpError> {
        match &self.config.server {
            Some(McpServerSpec::Stdio(config)) => Ok(Arc::new(StdioTransport::try_spawn(config)?)),
            Some(McpServerSpec::Http(config)) => Ok(Arc::new(HttpTransport::try_new(config)?)),
            None => Err(McpError::stable(
                MCP_SERVER_NOT_ALLOWLISTED,
                "MCP factory requires an allowlisted server",
            )),
        }
    }
}

/// Frozen MCP tool catalog backed by one transport.
pub struct McpToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    prompts: Arc<[McpPrompt]>,
    catalog_digest: Digest,
    invocation_digest: Digest,
    transport: Arc<dyn McpTransport>,
    config: McpConfig,
    list_changed: Option<Arc<dyn McpListChangedObserver>>,
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
        list_changed: Option<Arc<dyn McpListChangedObserver>>,
    ) -> Result<Self, McpError> {
        let listed = enumerate_catalog(transport.as_ref()).await?;
        emit_list_changed(transport.as_ref(), list_changed.as_deref());
        let prompts = enumerate_prompts(transport.as_ref())
            .await?
            .into_iter()
            .map(|prompt| McpPrompt {
                text: Arc::from(
                    prompt
                        .description
                        .as_deref()
                        .or(prompt.title.as_deref())
                        .unwrap_or(prompt.name.as_str()),
                ),
                name: Arc::from(prompt.name),
            })
            .collect::<Arc<[_]>>();
        emit_list_changed(transport.as_ref(), list_changed.as_deref());
        let tools = listed
            .iter()
            .map(|tool| to_tool_spec(tool, &config))
            .collect::<Result<Vec<_>, _>>()?;
        let mut seen_ids = BTreeSet::new();
        for spec in &tools {
            if !seen_ids.insert(spec.id.clone()) {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "tools/list returned colliding sanitized tool ids",
                ));
            }
        }
        let catalog = catalog_digest(&listed);
        let invocation = invocation_digest(&config.identity(), &listed);
        let metadata = Metadata::parse(
            serde_json::to_vec(&serde_json::json!({
                "protocol": protocol::PROTOCOL_VERSION,
                "server": config.identity(),
                "tools": listed.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
                "prompts": prompts.iter().map(McpPrompt::name).collect::<Vec<_>>(),
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
            prompts,
            catalog_digest: catalog,
            invocation_digest: invocation,
            transport,
            config,
            list_changed,
        })
    }

    /// Frozen `prompts/list` snapshot. Hosts map [`McpPrompt::text`] onto an
    /// untrusted `InstructionSpec`; this crate cannot set
    /// `trusted_application_instructions`.
    #[must_use]
    pub fn frozen_prompts(&self) -> &[McpPrompt] {
        &self.prompts
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
        let list_changed = self.list_changed.clone();
        Box::pin(async move {
            verify_authority(&ctx)?;
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
                .request_identified_controlled(
                    serde_json::json!(ctx.run.effect_id.to_string()),
                    "tools/call",
                    params,
                    RequestControl::new(ctx.run.cancellation.clone(), ctx.run.deadline),
                )
                .await
                .map_err(tool_error_from_mcp)?;
            emit_list_changed(transport.as_ref(), list_changed.as_deref());
            if value.get("method").and_then(serde_json::Value::as_str)
                == Some("sampling/createMessage")
            {
                return Err(sampling_required_error(value.get("params")));
            }
            let result: CallToolResult = serde_json::from_value(value).map_err(|_| {
                tool_error(
                    MCP_PROTOCOL_VIOLATION,
                    ErrorCategory::Validation,
                    "tools/call result is invalid",
                )
            })?;
            if result.result_type == ResultType::InputRequired {
                let request =
                    interaction::interaction_from_input_required(ctx.run.effect_id, &result)
                        .map_err(tool_error_from_mcp)?;
                return Err(interaction_required_error(&request));
            }
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
        let name = effect.call.call.tool_name();
        let retry_safe = self.config.is_read_only(name) || self.config.is_idempotent(name);
        Box::pin(async move {
            if retry_safe {
                Ok(ToolReconcileResult::Unknown)
            } else {
                Ok(ToolReconcileResult::NonRepeatable)
            }
        })
    }

    fn reconstruct(&self) -> PortFuture<Result<Option<Arc<dyn Toolset>>, ToolError>> {
        let transport = Arc::clone(&self.transport);
        let config = self.config.clone();
        let list_changed = self.list_changed.clone();
        let frozen = self.invocation_digest;
        Box::pin(async move {
            let toolset = McpToolset::connect(transport, config, list_changed)
                .await
                .map_err(tool_error_from_mcp)?;
            if toolset.invocation_digest != frozen {
                return Err(tool_error(
                    MCP_CATALOG_DRIFT,
                    ErrorCategory::Validation,
                    "reconstructed MCP catalog does not match the frozen invocation digest",
                ));
            }
            Ok(Some(Arc::new(toolset) as Arc<dyn Toolset>))
        })
    }

    fn complete_nested_sample(
        &self,
        ctx: ToolCallContext,
        _call: ValidatedToolCall,
        sample: NestedSample,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let transport = Arc::clone(&self.transport);
        let max_bytes = self.config.inline_result_bytes();
        let list_changed = self.list_changed.clone();
        Box::pin(async move {
            let params = serde_json::from_slice(sample.output.as_bytes())
                .unwrap_or_else(|_| serde_json::json!({}));
            let value = transport
                .request_identified_controlled(
                    serde_json::json!(ctx.run.effect_id.to_string()),
                    "sampling/createMessage",
                    params,
                    RequestControl::new(ctx.run.cancellation.clone(), ctx.run.deadline),
                )
                .await
                .map_err(tool_error_from_mcp)?;
            emit_list_changed(transport.as_ref(), list_changed.as_deref());
            let result: CallToolResult = serde_json::from_value(value).map_err(|_| {
                tool_error(
                    MCP_PROTOCOL_VIOLATION,
                    ErrorCategory::Validation,
                    "sampling completion result is invalid",
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
}

fn normalize_call_result(result: &CallToolResult, max_bytes: u64) -> Result<RawJson, ToolError> {
    let mut output = serde_json::json!({
        "content": result.content.iter().map(content_to_json).collect::<Vec<_>>(),
    });
    if let (Some(structured), Some(object)) = (&result.structured_content, output.as_object_mut()) {
        object.insert("structured_content".to_owned(), structured.clone());
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
        let mut preview_cap = max.saturating_div(2).max(1);
        loop {
            let preview = utf8_prefix(&bytes, preview_cap);
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
            if bytes.len() <= max || preview_cap <= 8 {
                break;
            }
            preview_cap = preview_cap.saturating_div(2).max(1);
        }
    }
    RawJson::parse(bytes).map_err(|_| {
        tool_error(
            MCP_TRANSPORT_ERROR,
            ErrorCategory::Internal,
            "MCP result normalization failed",
        )
    })
}

fn utf8_prefix(bytes: &[u8], max: usize) -> String {
    let end = max.min(bytes.len());
    match std::str::from_utf8(&bytes[..end]) {
        Ok(text) => text.to_owned(),
        Err(error) => {
            let valid = error.valid_up_to();
            String::from_utf8_lossy(&bytes[..valid]).into_owned()
        }
    }
}

fn completed(output: RawJson, is_error: bool) -> ToolEventStream {
    Box::pin(stream::once(async move {
        Ok(ToolStreamItem::Completed(ToolResult { output, is_error }))
    }))
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

/// Reconstruct the mapped HITL request from [`MCP_INPUT_REQUIRED`].
#[must_use]
pub fn interaction_request_from_tool_error(error: &ToolError) -> Option<InteractionRequest> {
    if error.code() != MCP_INPUT_REQUIRED {
        return None;
    }
    serde_json::from_slice(error.metadata().as_bytes()).ok()
}

fn sampling_required_error(params: Option<&serde_json::Value>) -> ToolError {
    let bytes = params
        .and_then(|value| serde_json::to_vec(value).ok())
        .unwrap_or_else(|| Vec::from(b"{}"));
    let metadata = Metadata::parse(bytes).unwrap_or_else(|_| Metadata::empty());
    ToolError::try_new(
        MCP_SAMPLING_REQUIRED,
        ErrorCategory::Tool,
        false,
        "MCP tool requested sampling",
        metadata,
    )
    .unwrap_or_else(Into::into)
}

fn interaction_required_error(request: &InteractionRequest) -> ToolError {
    let metadata = serde_json::to_vec(request)
        .ok()
        .and_then(|bytes| Metadata::parse(bytes).ok())
        .unwrap_or_else(Metadata::empty);
    ToolError::try_new(
        MCP_INPUT_REQUIRED,
        ErrorCategory::Tool,
        false,
        "MCP tool requested elicitation",
        metadata,
    )
    .unwrap_or_else(Into::into)
}

pub(crate) fn emit_list_changed(
    transport: &dyn McpTransport,
    observer: Option<&dyn McpListChangedObserver>,
) {
    let Some(observer) = observer else {
        return;
    };
    for method in transport.take_notifications() {
        if method == "notifications/tools/list_changed"
            || method == "notifications/resources/list_changed"
            || method == "notifications/prompts/list_changed"
        {
            observer.on_list_changed(&method);
        }
    }
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
    jsonrpc_code: Option<i64>,
}

impl McpError {
    pub(crate) fn stable(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            jsonrpc_code: None,
        }
    }

    pub(crate) fn with_jsonrpc(mut self, jsonrpc_code: i64) -> Self {
        self.jsonrpc_code = Some(jsonrpc_code);
        self
    }

    pub(crate) const fn jsonrpc_code(&self) -> Option<i64> {
        self.jsonrpc_code
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
