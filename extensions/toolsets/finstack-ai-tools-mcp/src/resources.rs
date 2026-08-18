//! Frozen `resources/list` snapshot and the MCP `ContextProvider`.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_runtime::{
    ComponentId, ComponentInvocation, ContentBlock, ContextAuthority, ContextCallContext,
    ContextContribution, ContextError, ContextItem, ContextItemKind, ContextOverflowPolicy,
    ContextProvenance, ContextProvider, ContextProviderDescriptor, ContextReconcileResult,
    ContextRequest, Digest, ErrorCategory, InvocationRecovery, Metadata, PendingContextEffect,
    PortFuture, ReconcileContext, Sensitivity, TextBlock, Version,
};

use crate::classify::MAX_LIST_PAGES;
use crate::protocol::{
    ListResourcesResult, ReadResourceResult, Resource, ResourceContents, ResultType,
};
use crate::transport::McpTransport;
use crate::{
    CONTEXT_PROVIDER_COMPONENT, MCP_PROTOCOL_VIOLATION, MCP_RESULT_UNSUPPORTED, McpConfig, McpError,
};

const RESOURCE_SOURCE_ID: &str = "finstack.context.mcp";

/// One name/URI pair frozen from `resources/list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrozenResource {
    name: Arc<str>,
    uri: Arc<str>,
}

impl FrozenResource {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn uri(&self) -> &str {
        &self.uri
    }
}

/// Context provider over one frozen MCP `resources/list` snapshot.
///
/// `collect` fetches only the names captured at construction. Mid-run
/// `resources/list` changes are ignored. The provider is never an
/// application-instruction authority.
pub struct McpContextProvider {
    descriptor: ContextProviderDescriptor,
    snapshot: Arc<[FrozenResource]>,
    transport: Arc<dyn McpTransport>,
    inline_result_bytes: u64,
}

impl McpContextProvider {
    /// Enumerate `resources/list` once and freeze the name set.
    ///
    /// # Errors
    ///
    /// Fails on protocol violations, unsupported result types, empty names or
    /// URIs, or duplicate names/URIs.
    pub(crate) async fn connect(
        transport: Arc<dyn McpTransport>,
        config: &McpConfig,
    ) -> Result<Self, McpError> {
        let listed = enumerate_resources(transport.as_ref()).await?;
        let snapshot = listed
            .iter()
            .map(|resource| FrozenResource {
                name: Arc::from(resource.name.as_str()),
                uri: Arc::from(resource.uri.as_str()),
            })
            .collect::<Arc<[_]>>();
        let identity = config.identity();
        let digest = resource_snapshot_digest(&identity, &listed);
        let metadata = Metadata::parse(
            serde_json::to_vec(&serde_json::json!({
                "protocol": crate::protocol::PROTOCOL_VERSION,
                "server": config.identity(),
                "resources": listed.iter().map(|resource| resource.name.as_str()).collect::<Vec<_>>(),
                "snapshot_digest": digest.to_string(),
            }))
            .unwrap_or_else(|_| Vec::from(b"{}")),
        )
        .unwrap_or_else(|_| Metadata::empty());
        Ok(Self {
            descriptor: ContextProviderDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse(CONTEXT_PROVIDER_COMPONENT).map_err(|_| {
                        McpError::stable(
                            MCP_PROTOCOL_VIOLATION,
                            "MCP context provider component id is invalid",
                        )
                    })?,
                    version: Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    configuration_digest: digest,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                trusted_application_instructions: false,
                metadata,
            },
            snapshot,
            transport,
            inline_result_bytes: config.inline_result_bytes(),
        })
    }

    /// Frozen resource names in list order.
    #[must_use]
    pub fn frozen_names(&self) -> Vec<&str> {
        self.snapshot
            .iter()
            .map(FrozenResource::name)
            .collect::<Vec<_>>()
    }
}

impl ContextProvider for McpContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let transport = Arc::clone(&self.transport);
        let snapshot = Arc::clone(&self.snapshot);
        let max_bytes = self.inline_result_bytes;
        Box::pin(async move { collect_frozen(&transport, &snapshot, &request, max_bytes).await })
    }

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingContextEffect,
    ) -> PortFuture<Result<ContextReconcileResult, ContextError>> {
        Box::pin(async { Ok(ContextReconcileResult::RetrySafe) })
    }
}

pub(crate) async fn enumerate_resources(
    transport: &dyn McpTransport,
) -> Result<Vec<Resource>, McpError> {
    let mut resources = Vec::new();
    let mut seen_names = BTreeSet::new();
    let mut seen_uris = BTreeSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_LIST_PAGES {
        let mut params = serde_json::json!({});
        if let Some(cursor) = cursor.as_ref() {
            params.as_object_mut().expect("object").insert(
                "cursor".to_owned(),
                serde_json::Value::String(cursor.clone()),
            );
        }
        let value = transport.request("resources/list", params).await?;
        let page: ListResourcesResult = serde_json::from_value(value).map_err(|error| {
            McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                format!("resources/list result is invalid: {error}"),
            )
        })?;
        if page.result_type != ResultType::Complete {
            return Err(McpError::stable(
                MCP_RESULT_UNSUPPORTED,
                "resources/list resultType is not complete",
            ));
        }
        for resource in page.resources {
            if resource.name.is_empty() {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "resources/list returned an empty resource name",
                ));
            }
            if resource.uri.is_empty() {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "resources/list returned an empty resource uri",
                ));
            }
            if !seen_names.insert(resource.name.clone()) {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "resources/list returned a duplicate resource name",
                ));
            }
            if !seen_uris.insert(resource.uri.clone()) {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "resources/list returned a duplicate resource uri",
                ));
            }
            resources.push(resource);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(resources),
        }
    }
    Err(McpError::stable(
        MCP_PROTOCOL_VIOLATION,
        "resources/list exceeded the page cap",
    ))
}

pub(crate) fn resource_snapshot_digest(server_identity: &str, resources: &[Resource]) -> Digest {
    let names = resources
        .iter()
        .map(|resource| serde_json::json!([resource.name, resource.uri]))
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "server": server_identity,
        "resources": names,
    });
    let bytes = serde_json_canonicalizer::to_vec(&payload).unwrap_or_else(|_| Vec::from(b"{}"));
    Digest::raw_json(&bytes)
}

async fn collect_frozen(
    transport: &Arc<dyn McpTransport>,
    snapshot: &[FrozenResource],
    request: &ContextRequest,
    max_bytes: u64,
) -> Result<ContextContribution, ContextError> {
    let mut items = Vec::with_capacity(snapshot.len());
    for resource in snapshot {
        let value = transport
            .request(
                "resources/read",
                serde_json::json!({
                    "uri": resource.uri(),
                    "name": resource.name(),
                }),
            )
            .await
            .map_err(|error| context_error_from_mcp(&error))?;
        let result: ReadResourceResult = serde_json::from_value(value).map_err(|_| {
            context_error(
                MCP_PROTOCOL_VIOLATION,
                ErrorCategory::Validation,
                "resources/read result is invalid",
            )
        })?;
        if result.result_type != ResultType::Complete {
            return Err(context_error(
                MCP_RESULT_UNSUPPORTED,
                ErrorCategory::Validation,
                "resources/read resultType is not complete",
            ));
        }
        items.push(item_from_read(resource, &result.contents, max_bytes)?);
    }
    apply_budget(items, request)
}

fn item_from_read(
    resource: &FrozenResource,
    contents: &[ResourceContents],
    max_bytes: u64,
) -> Result<ContextItem, ContextError> {
    let text = render_contents(contents, resource, max_bytes);
    ContextItem::try_new(
        ContextItemKind::QuotedSource,
        vec![ContentBlock::Text(TextBlock::try_new(&text).map_err(
            |_| contribution_invalid("MCP resource text is invalid"),
        )?)],
        ContextProvenance {
            source_id: Arc::from(RESOURCE_SOURCE_ID),
            source_ref: Some(Arc::clone(&resource.uri)),
            external: true,
        },
        ContextAuthority::Untrusted,
        0,
        estimate_tokens(&text),
        Sensitivity::Internal,
        false,
    )
}

fn render_contents(
    contents: &[ResourceContents],
    resource: &FrozenResource,
    max_bytes: u64,
) -> String {
    let mut parts = Vec::new();
    for content in contents {
        if let Some(text) = &content.text {
            parts.push(truncate(text, max_bytes));
            continue;
        }
        if content.blob.is_some() {
            let mime = content
                .mime_type
                .as_deref()
                .unwrap_or("application/octet-stream");
            parts.push(format!(
                "binary resource omitted ({mime}) uri={}",
                content.uri
            ));
        }
    }
    if parts.is_empty() {
        format!("empty resource {}", resource.name())
    } else {
        parts.join("\n")
    }
}

fn truncate(text: &str, max_bytes: u64) -> String {
    let max = usize::try_from(max_bytes).unwrap_or(usize::MAX).max(1);
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max.saturating_sub(16).min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated]", &text[..end])
}

fn apply_budget(
    items: Vec<ContextItem>,
    request: &ContextRequest,
) -> Result<ContextContribution, ContextError> {
    let mut accepted = Vec::new();
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for item in items {
        let next_tokens = tokens.saturating_add(item.estimated_tokens);
        let next_bytes = bytes.saturating_add(item.bytes);
        if accepted.len() >= request.budget.max_items
            || next_tokens > request.budget.max_tokens
            || next_bytes > request.budget.max_bytes
        {
            return match request.budget.overflow {
                ContextOverflowPolicy::Reject => Err(context_error(
                    finstack_ai_runtime::CONTEXT_BUDGET_EXCEEDED,
                    ErrorCategory::Limit,
                    "MCP resource contribution exceeds the committed budget",
                )),
                ContextOverflowPolicy::TruncateWithDiagnostic => break,
            };
        }
        tokens = next_tokens;
        bytes = next_bytes;
        accepted.push(item);
    }
    let _ = (tokens, bytes);
    ContextContribution::try_new(accepted, Some("finstack.context.mcp"))
}

fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(1).max(1)
}

fn contribution_invalid(message: &'static str) -> ContextError {
    context_error(
        finstack_ai_runtime::CONTEXT_CONTRIBUTION_INVALID,
        ErrorCategory::Validation,
        message,
    )
}

fn context_error(
    code: &'static str,
    category: ErrorCategory,
    message: &'static str,
) -> ContextError {
    ContextError::try_new(code, category, message, Metadata::empty()).unwrap_or_else(Into::into)
}

fn context_error_from_mcp(error: &McpError) -> ContextError {
    let category = match error.code() {
        crate::MCP_LIMIT_EXCEEDED | crate::MCP_ARTIFACT_REQUIRED => ErrorCategory::Limit,
        crate::MCP_TRANSPORT_ERROR | crate::MCP_SERVER_NOT_ALLOWLISTED => ErrorCategory::Context,
        _ => ErrorCategory::Validation,
    };
    ContextError::try_new(error.code(), category, error.message(), Metadata::empty())
        .unwrap_or_else(Into::into)
}
