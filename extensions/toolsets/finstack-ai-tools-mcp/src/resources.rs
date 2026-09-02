//! Frozen `resources/list` snapshot and the MCP `ContextProvider`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, ErrorCategory, InvocationRecovery,
    Metadata, Sensitivity, TextBlock, Version,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::context::{
    ContextAuthority, ContextCallContext, ContextContribution, ContextError, ContextItem,
    ContextItemKind, ContextOverflowPolicy, ContextProvenance, ContextProvider,
    ContextProviderDescriptor, ContextReconcileResult, ContextRequest, PendingContextEffect,
};
use finstack_ai_runtime::ports::model::ReconcileContext;

use crate::classify::list_all;
use crate::protocol::{
    ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, ResultType,
};
use crate::transport::{McpTransport, RequestControl};
use crate::{
    CONTEXT_PROVIDER_COMPONENT, MCP_PROTOCOL_VIOLATION, MCP_RESULT_UNSUPPORTED,
    MCP_SUBSCRIBE_UNKNOWN, McpConfig, McpError,
};

const RESOURCE_SOURCE_ID: &str = "finstack.context.mcp";

/// One name/URI pair frozen from `resources/list` or `resources/templates`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrozenResource {
    name: Arc<str>,
    uri: Arc<str>,
    template: bool,
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
    config: McpConfig,
    list_changed: Option<Arc<dyn crate::McpListChangedObserver>>,
    subscribed: Mutex<BTreeSet<Arc<str>>>,
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
        list_changed: Option<Arc<dyn crate::McpListChangedObserver>>,
    ) -> Result<Self, McpError> {
        let listed = enumerate_resources(transport.as_ref()).await?;
        crate::emit_list_changed(transport.as_ref(), list_changed.as_deref());
        let templates = enumerate_templates(transport.as_ref()).await?;
        crate::emit_list_changed(transport.as_ref(), list_changed.as_deref());
        let mut snapshot = listed
            .iter()
            .map(|resource| FrozenResource {
                name: Arc::from(resource.name.as_str()),
                uri: Arc::from(resource.uri.as_str()),
                template: false,
            })
            .collect::<Vec<_>>();
        for template in &templates {
            snapshot.push(FrozenResource {
                name: Arc::from(template.name.as_str()),
                uri: Arc::from(template.uri_template.as_str()),
                template: true,
            });
        }
        let mut seen = BTreeSet::new();
        for resource in &snapshot {
            if !seen.insert(resource.name()) {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "resources snapshot has a duplicate name across list and templates",
                ));
            }
        }
        let snapshot = snapshot.into();
        let identity = config.identity();
        let digest = resource_snapshot_digest(&identity, &listed, &templates);
        let metadata = Metadata::parse(
            serde_json::to_vec(&serde_json::json!({
                "protocol": crate::protocol::PROTOCOL_VERSION,
                "server": config.identity(),
                "resources": listed.iter().map(|resource| resource.name.as_str()).collect::<Vec<_>>(),
                "templates": templates.iter().map(|template| template.name.as_str()).collect::<Vec<_>>(),
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
            config: config.clone(),
            list_changed,
            subscribed: Mutex::new(BTreeSet::new()),
        })
    }

    /// Subscribe to one frozen resource name.
    ///
    /// Unknown names fail closed. Later [`ContextProvider::collect`] re-reads
    /// subscribed URIs. Names absent from the frozen snapshot are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`MCP_SUBSCRIBE_UNKNOWN`] when the name is not frozen, or a
    /// transport/protocol error when `resources/subscribe` fails.
    pub async fn subscribe(&self, name: &str) -> Result<(), McpError> {
        let resource = self
            .snapshot
            .iter()
            .find(|resource| resource.name() == name)
            .ok_or_else(|| {
                McpError::stable(
                    MCP_SUBSCRIBE_UNKNOWN,
                    "resources/subscribe name is not in the frozen snapshot",
                )
            })?;
        self.transport
            .request(
                "resources/subscribe",
                serde_json::json!({
                    "uri": resource.uri(),
                    "name": resource.name(),
                }),
            )
            .await?;
        self.subscribed
            .lock()
            .map_err(|_| {
                McpError::stable(crate::MCP_TRANSPORT_ERROR, "subscribe lock is poisoned")
            })?
            .insert(Arc::from(name));
        Ok(())
    }

    /// Frozen `resources/templates` names in list order.
    #[must_use]
    pub fn frozen_template_names(&self) -> Vec<&str> {
        self.snapshot
            .iter()
            .filter(|resource| resource.template)
            .map(FrozenResource::name)
            .collect::<Vec<_>>()
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
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let transport = Arc::clone(&self.transport);
        let snapshot = Arc::clone(&self.snapshot);
        let subscribed = self
            .subscribed
            .lock()
            .map(|names| names.clone())
            .unwrap_or_default();
        let max_bytes = self.config.inline_result_bytes();
        let control = RequestControl::new(ctx.run.cancellation.clone(), ctx.run.deadline);
        Box::pin(async move {
            collect_frozen(
                transport.as_ref(),
                &snapshot,
                &subscribed,
                &request,
                max_bytes,
                control,
            )
            .await
        })
    }

    fn reconstruct(&self) -> PortFuture<Result<Option<Arc<dyn ContextProvider>>, ContextError>> {
        let transport = Arc::clone(&self.transport);
        let config = self.config.clone();
        let list_changed = self.list_changed.clone();
        Box::pin(async move {
            let provider = McpContextProvider::connect(transport, &config, list_changed)
                .await
                .map_err(|error| context_error_from_mcp(&error))?;
            Ok(Some(Arc::new(provider) as Arc<dyn ContextProvider>))
        })
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
    let mut seen_names = BTreeSet::new();
    let mut seen_uris = BTreeSet::new();
    list_all::<ListResourcesResult>(transport, "resources/list", false, |resource| {
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
        Ok(())
    })
    .await
}

async fn enumerate_templates(
    transport: &dyn McpTransport,
) -> Result<Vec<ResourceTemplate>, McpError> {
    let mut seen = BTreeSet::new();
    list_all::<ListResourceTemplatesResult>(transport, "resources/templates", true, |template| {
        if template.name.is_empty() || template.uri_template.is_empty() {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "resources/templates returned an empty name or uriTemplate",
            ));
        }
        if !seen.insert(template.name.clone()) {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "resources/templates returned a duplicate template name",
            ));
        }
        Ok(())
    })
    .await
}

fn resource_snapshot_digest(
    server_identity: &str,
    resources: &[Resource],
    templates: &[ResourceTemplate],
) -> Digest {
    let names = resources
        .iter()
        .map(|resource| serde_json::json!([resource.name, resource.uri]))
        .collect::<Vec<_>>();
    let template_names = templates
        .iter()
        .map(|template| serde_json::json!([template.name, template.uri_template]))
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "server": server_identity,
        "resources": names,
        "templates": template_names,
    });
    let bytes = serde_json_canonicalizer::to_vec(&payload).unwrap_or_else(|_| Vec::from(b"{}"));
    Digest::raw_json(&bytes)
}

async fn collect_frozen(
    transport: &dyn McpTransport,
    snapshot: &[FrozenResource],
    subscribed: &BTreeSet<Arc<str>>,
    request: &ContextRequest,
    max_bytes: u64,
    control: RequestControl,
) -> Result<ContextContribution, ContextError> {
    let selected: Vec<&FrozenResource> = if subscribed.is_empty() {
        snapshot.iter().collect()
    } else {
        snapshot
            .iter()
            .filter(|resource| subscribed.contains(resource.name()))
            .collect()
    };
    if selected.len() > request.budget.max_items
        && matches!(request.budget.overflow, ContextOverflowPolicy::Reject)
    {
        return Err(context_error(
            finstack_ai_runtime::ports::context::CONTEXT_BUDGET_EXCEEDED,
            ErrorCategory::Limit,
            "MCP resource contribution exceeds the committed budget",
        ));
    }
    let selected = selected
        .into_iter()
        .take(request.budget.max_items)
        .collect::<Vec<_>>();
    let mut items = Vec::with_capacity(selected.len());
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for resource in selected {
        if tokens >= request.budget.max_tokens || bytes >= request.budget.max_bytes {
            return apply_overflow(items, request);
        }
        let value = transport
            .request_controlled(
                "resources/read",
                serde_json::json!({
                    "uri": resource.uri(),
                    "name": resource.name(),
                }),
                control.clone(),
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
        let item = item_from_read(resource, &result.contents, max_bytes)?;
        let next_tokens = tokens.saturating_add(item.estimated_tokens);
        let next_bytes = bytes.saturating_add(item.bytes);
        if next_tokens > request.budget.max_tokens || next_bytes > request.budget.max_bytes {
            return apply_overflow(items, request);
        }
        tokens = next_tokens;
        bytes = next_bytes;
        items.push(item);
    }
    ContextContribution::try_new(items, Some("finstack.context.mcp"))
}

fn apply_overflow(
    items: Vec<ContextItem>,
    request: &ContextRequest,
) -> Result<ContextContribution, ContextError> {
    match request.budget.overflow {
        ContextOverflowPolicy::Reject => Err(context_error(
            finstack_ai_runtime::ports::context::CONTEXT_BUDGET_EXCEEDED,
            ErrorCategory::Limit,
            "MCP resource contribution exceeds the committed budget",
        )),
        ContextOverflowPolicy::TruncateWithDiagnostic => {
            ContextContribution::try_new(items, Some("finstack.context.mcp"))
        }
    }
}

fn item_from_read(
    resource: &FrozenResource,
    contents: &[ResourceContents],
    max_bytes: u64,
) -> Result<ContextItem, ContextError> {
    let text = render_contents(contents, resource, max_bytes);
    let block = TextBlock::try_new(&text).map_err(|_| {
        context_error(
            finstack_ai_runtime::ports::context::CONTEXT_CONTRIBUTION_INVALID,
            ErrorCategory::Validation,
            "MCP resource text is invalid",
        )
    })?;
    ContextItem::try_new(
        ContextItemKind::QuotedSource,
        vec![ContentBlock::Text(block)],
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
            parts.push(text.clone());
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
    let joined = if parts.is_empty() {
        format!("empty resource {}", resource.name())
    } else {
        parts.join("\n")
    };
    truncate(&joined, max_bytes)
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

fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(1).max(1)
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
