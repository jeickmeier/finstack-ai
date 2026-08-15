//! Isolated Wasmtime adapters that register as ordinary SDK ports.

use std::sync::Arc;

use finstack_ai::{
    ComponentConstructionContext, Extension, ExtensionDescriptor, LifecycleBinding, ReadyComponent,
    Registrar, RegistrationError, RegistrationMetadata,
};
use finstack_ai_runtime::{
    ComponentRef, ContextCallContext, ContextContribution, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest, Digest, ErrorCategory, InvocationRecovery, Metadata,
    PortFuture, RawJson, Timestamp, ToolCallContext, ToolError, ToolEventStream, ToolResult,
    ToolSpec, ToolStreamItem, Toolset, ToolsetDescriptor, ValidatedToolCall, Version,
};
use finstack_ai_wit::{
    NoopPluginHooks, PluginGuestHooks, PluginLifecycle, PluginManifest, honor_deadline,
    map_context_item, map_query, merge_call_deadline, register_catalog, sanitize_call_context,
};
use futures_util::stream;
use tokio::sync::{Mutex, Semaphore};
use wasmtime::Store;

use crate::bindings::context::ContextPlugin;
use crate::bindings::toolset::ToolsetPlugin;
use crate::convert::{
    format_plugin_error, wasm_call_context, wasm_context_query, wit_context_item, wit_plugin_error,
    wit_tool_catalog, wit_tool_result,
};
use crate::error::PluginHostError;
use crate::host::{InstancePolicy, PluginHost, PluginWorld, ReadyWasm};
use crate::instantiate::{
    HostState, host_state_for, map_wasmtime_error, new_store, with_cancellation,
};
use crate::limits::effective_limits;

type LiveToolset = (Store<HostState>, ToolsetPlugin);
type LiveContext = (Store<HostState>, ContextPlugin);
type SerializedToolset = Arc<Mutex<Option<LiveToolset>>>;
type SerializedContext = Arc<Mutex<Option<LiveContext>>>;

fn adapter_version(manifest: &PluginManifest) -> Version {
    match manifest.version.as_str() {
        "1.0.0" => Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        _ => Version {
            major: 0,
            minor: 0,
            patch: 4,
        },
    }
}

/// Isolated (T3) `Toolset` adapter over a compiled `toolset-plugin` component.
pub struct WasmToolsetAdapter {
    host: Arc<PluginHost>,
    ready: ReadyWasm,
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    lifecycle: Arc<PluginLifecycle>,
    exclusive: Arc<Semaphore>,
    serialized: SerializedToolset,
}

impl WasmToolsetAdapter {
    /// Compile, instantiate once for `list-tools`, and initialize lifecycle.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the manifest, compile, instantiate,
    /// catalog, or lifecycle step fails.
    pub async fn try_new(
        host: Arc<PluginHost>,
        bytes: &[u8],
        manifest: PluginManifest,
        construction: &ComponentConstructionContext,
        hooks: Arc<dyn PluginGuestHooks>,
    ) -> Result<Self, PluginHostError> {
        let limits = effective_limits(&manifest, host.default_limits());
        let mut construction = construction.clone();
        construction.deadline = merge_call_deadline(construction.deadline, limits.call_timeout_ms);
        honor_deadline(&construction).map_err(|error| PluginHostError::from_lifecycle(&error))?;
        let ready = host.load(bytes, manifest, PluginWorld::Toolset)?;
        let metadata = plugin_metadata(&ready);
        let lifecycle = Arc::new(PluginLifecycle::new(hooks));
        lifecycle
            .run_construction(&construction)
            .map_err(|error| PluginHostError::from_lifecycle(&error))?;
        let exclusive = Arc::new(Semaphore::new(host.max_concurrent_instances() as usize));
        let serialized = Arc::new(Mutex::new(None));
        let catalog = list_tools(
            &host,
            &ready,
            &exclusive,
            &serialized,
            &construction.cancellation,
            construction.deadline,
        )
        .await?;
        let tools =
            register_catalog(&catalog).map_err(|error| PluginHostError::from_map(&error))?;
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from(ready.manifest.identity.as_str()),
                metadata,
            },
            tools: tools.into(),
            lifecycle,
            exclusive,
            serialized,
            ready,
            host,
        })
    }

    /// Lifecycle hooks attached to the ready component.
    #[must_use]
    pub fn lifecycle(&self) -> Arc<PluginLifecycle> {
        Arc::clone(&self.lifecycle)
    }

    #[cfg(test)]
    pub(crate) fn hold_exclusive_slot(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, PluginHostError> {
        Arc::clone(&self.exclusive)
            .try_acquire_owned()
            .map_err(|_| PluginHostError::InstanceLimit)
    }

    #[cfg(test)]
    pub(crate) fn hold_serialized_slot(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<LiveToolset>>, PluginHostError> {
        self.serialized
            .try_lock()
            .map_err(|_| PluginHostError::InstanceLimit)
    }
}

impl Toolset for WasmToolsetAdapter {
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
        let host = Arc::clone(&self.host);
        let ready = self.ready.clone();
        let exclusive = Arc::clone(&self.exclusive);
        let serialized = Arc::clone(&self.serialized);
        let context = sanitize_call_context(&ctx.run);
        let tool_id = call.tool_id.to_string();
        let args = call.call.arguments().as_bytes().to_vec();
        let cancel = ctx.run.cancellation.clone();
        let metadata = self.descriptor.metadata.clone();
        let deadline = merge_call_deadline(
            ctx.run.deadline,
            effective_limits(&self.ready.manifest, self.host.default_limits()).call_timeout_ms,
        );
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(tool_error(&PluginHostError::Timeout, metadata));
            }
            let result = call_tool(
                &host,
                &ready,
                &exclusive,
                &serialized,
                &cancel,
                deadline,
                &context,
                &tool_id,
                &args,
            )
            .await
            .map_err(|error| tool_error(&error, metadata.clone()))?;
            let output = RawJson::parse(&result.content_json).map_err(|_| {
                tool_error(
                    &PluginHostError::Mapped(
                        "plugin_result_invalid: tool result JSON is invalid".into(),
                    ),
                    metadata,
                )
            })?;
            let completed = ToolResult {
                output,
                is_error: result.is_error,
            };
            Ok(Box::pin(stream::once(async move {
                Ok(ToolStreamItem::Completed(completed))
            })) as ToolEventStream)
        })
    }
}

/// Isolated (T3) `ContextProvider` adapter over a compiled `context-plugin`.
pub struct WasmContextAdapter {
    host: Arc<PluginHost>,
    ready: ReadyWasm,
    descriptor: ContextProviderDescriptor,
    lifecycle: Arc<PluginLifecycle>,
    exclusive: Arc<Semaphore>,
    serialized: SerializedContext,
    limits: Option<finstack_ai_wit::PluginResourceLimits>,
}

impl WasmContextAdapter {
    /// Compile and initialize one isolated context adapter.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when the manifest, compile, or lifecycle
    /// step fails.
    ///
    /// Construction stays async so it matches [`WasmToolsetAdapter::try_new`].
    #[allow(clippy::unused_async)]
    pub async fn try_new(
        host: Arc<PluginHost>,
        bytes: &[u8],
        manifest: PluginManifest,
        construction: &ComponentConstructionContext,
        hooks: Arc<dyn PluginGuestHooks>,
    ) -> Result<Self, PluginHostError> {
        let limits = effective_limits(&manifest, host.default_limits());
        let mut construction = construction.clone();
        construction.deadline = merge_call_deadline(construction.deadline, limits.call_timeout_ms);
        honor_deadline(&construction).map_err(|error| PluginHostError::from_lifecycle(&error))?;
        let ready = host.load(bytes, manifest, PluginWorld::Context)?;
        let metadata = plugin_metadata(&ready);
        let lifecycle = Arc::new(PluginLifecycle::new(hooks));
        lifecycle
            .run_construction(&construction)
            .map_err(|error| PluginHostError::from_lifecycle(&error))?;
        Ok(Self {
            descriptor: ContextProviderDescriptor {
                invocation: finstack_ai_runtime::ComponentInvocation {
                    component: ready.manifest.identity.clone(),
                    version: adapter_version(&ready.manifest),
                    configuration_digest: Digest::raw_json(b"{}"),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                trusted_application_instructions: false,
                metadata,
            },
            limits: ready.manifest.resource_limits.clone(),
            exclusive: Arc::new(Semaphore::new(host.max_concurrent_instances() as usize)),
            serialized: Arc::new(Mutex::new(None)),
            lifecycle,
            ready,
            host,
        })
    }

    /// Lifecycle hooks attached to the ready component.
    #[must_use]
    pub fn lifecycle(&self) -> Arc<PluginLifecycle> {
        Arc::clone(&self.lifecycle)
    }
}

impl ContextProvider for WasmContextAdapter {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let metadata = self.descriptor.metadata.clone();
        let query = match map_query(&ctx.run, &request, self.limits.as_ref()) {
            Ok(query) => query,
            Err(error) => {
                let error = context_error(&PluginHostError::from_map(&error), metadata);
                return Box::pin(async move { Err(error) });
            }
        };
        let host = Arc::clone(&self.host);
        let ready = self.ready.clone();
        let exclusive = Arc::clone(&self.exclusive);
        let serialized = Arc::clone(&self.serialized);
        let descriptor = self.descriptor.clone();
        let cancel = ctx.run.cancellation.clone();
        let deadline = merge_call_deadline(
            ctx.run.deadline,
            effective_limits(&ready.manifest, host.default_limits()).call_timeout_ms,
        );
        Box::pin(async move {
            let items = collect_items(
                &host,
                &ready,
                &exclusive,
                &serialized,
                &cancel,
                deadline,
                &query,
            )
            .await
            .map_err(|error| context_error(&error, metadata.clone()))?;
            let native = items
                .iter()
                .map(|item| map_context_item(item, &descriptor))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    context_error(&PluginHostError::from_map(&error), metadata.clone())
                })?;
            ContextContribution::try_new(native, None::<&str>).map_err(|_| {
                context_error(
                    &PluginHostError::Mapped(
                        "plugin_context_item_invalid: context contribution is invalid".into(),
                    ),
                    metadata,
                )
            })
        })
    }
}

/// Extension that registers isolated Wasmtime adapters named by one manifest.
pub struct WasmPluginExtension {
    manifest: PluginManifest,
    context: Option<Arc<dyn ContextProvider>>,
    context_lifecycle: Option<Arc<PluginLifecycle>>,
    toolset: Option<Arc<dyn Toolset>>,
    toolset_lifecycle: Option<Arc<PluginLifecycle>>,
}

impl WasmPluginExtension {
    /// Register an isolated toolset guest.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when compile, instantiate, or catalog mapping
    /// fails.
    pub async fn toolset(
        host: Arc<PluginHost>,
        bytes: &[u8],
        manifest: PluginManifest,
    ) -> Result<Self, PluginHostError> {
        let construction = construction_context(&manifest);
        let adapter = WasmToolsetAdapter::try_new(
            host,
            bytes,
            manifest.clone(),
            &construction,
            Arc::new(NoopPluginHooks),
        )
        .await?;
        Ok(Self {
            toolset_lifecycle: Some(adapter.lifecycle()),
            toolset: Some(Arc::new(adapter)),
            context: None,
            context_lifecycle: None,
            manifest,
        })
    }

    /// Register an isolated context guest.
    ///
    /// # Errors
    ///
    /// Returns [`PluginHostError`] when compile or lifecycle fails.
    pub async fn context(
        host: Arc<PluginHost>,
        bytes: &[u8],
        manifest: PluginManifest,
    ) -> Result<Self, PluginHostError> {
        let construction = construction_context(&manifest);
        let adapter = WasmContextAdapter::try_new(
            host,
            bytes,
            manifest.clone(),
            &construction,
            Arc::new(NoopPluginHooks),
        )
        .await?;
        Ok(Self {
            context_lifecycle: Some(adapter.lifecycle()),
            context: Some(Arc::new(adapter)),
            toolset: None,
            toolset_lifecycle: None,
            manifest,
        })
    }
}

impl Extension for WasmPluginExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        // Preview residual: ExtensionTrust has no IsolatedWasm variant.
        // Isolation is a property of PluginHost, not of this descriptor.
        ExtensionDescriptor::trusted_in_process(
            self.manifest.identity.clone(),
            adapter_version(&self.manifest),
        )
    }

    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
        let metadata = RegistrationMetadata::new(
            self.manifest.identity.clone(),
            adapter_version(&self.manifest),
        );
        match (&self.context, &self.context_lifecycle) {
            (Some(provider), Some(lifecycle)) => {
                let hooks: Arc<dyn finstack_ai::ComponentLifecycle> =
                    Arc::clone(lifecycle) as Arc<dyn finstack_ai::ComponentLifecycle>;
                registrar.context_provider(
                    metadata.clone(),
                    ReadyComponent::new(Arc::clone(provider))
                        .with_lifecycle(LifecycleBinding::resolved_agent(hooks)),
                )?;
            }
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                return Err(RegistrationError::InvalidDescriptor {
                    code: "registration_invalid_descriptor",
                    message: Arc::from("context adapter is missing lifecycle hooks"),
                });
            }
        }
        match (&self.toolset, &self.toolset_lifecycle) {
            (Some(toolset), Some(lifecycle)) => {
                let hooks: Arc<dyn finstack_ai::ComponentLifecycle> =
                    Arc::clone(lifecycle) as Arc<dyn finstack_ai::ComponentLifecycle>;
                registrar.toolset(
                    metadata,
                    ReadyComponent::new(Arc::clone(toolset))
                        .with_lifecycle(LifecycleBinding::resolved_agent(hooks)),
                )?;
            }
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                return Err(RegistrationError::InvalidDescriptor {
                    code: "registration_invalid_descriptor",
                    message: Arc::from("toolset adapter is missing lifecycle hooks"),
                });
            }
        }
        if self.context.is_none() && self.toolset.is_none() {
            return Err(RegistrationError::InvalidDescriptor {
                code: "registration_invalid_descriptor",
                message: Arc::from("plugin extension registered no port"),
            });
        }
        Ok(())
    }
}

fn construction_context(manifest: &PluginManifest) -> ComponentConstructionContext {
    ComponentConstructionContext {
        component: ComponentRef::new(manifest.identity.clone(), Some(adapter_version(manifest))),
        configuration: None,
        cancellation: finstack_ai_runtime::CancellationSignal::new(),
        deadline: None,
        metadata: Metadata::empty(),
    }
}

async fn instantiate_toolset(
    host: &PluginHost,
    ready: &ReadyWasm,
    cancel: &finstack_ai_runtime::CancellationSignal,
) -> Result<(Store<HostState>, ToolsetPlugin), PluginHostError> {
    let limits = effective_limits(&ready.manifest, host.default_limits());
    let mut store = new_store(
        host.engine(),
        host_state_for(limits, &ready.granted, host.grant_resources())?,
        limits.fuel,
    )?;
    let linker = host.linker_for_world(PluginWorld::Toolset, &ready.granted)?;
    let bindings = ToolsetPlugin::instantiate_async(&mut store, ready.component(), &linker)
        .await
        .map_err(|error| map_wasmtime_error(&error, cancel.is_cancelled()))?;
    Ok((store, bindings))
}

async fn instantiate_context(
    host: &PluginHost,
    ready: &ReadyWasm,
    cancel: &finstack_ai_runtime::CancellationSignal,
) -> Result<(Store<HostState>, ContextPlugin), PluginHostError> {
    let limits = effective_limits(&ready.manifest, host.default_limits());
    let mut store = new_store(
        host.engine(),
        host_state_for(limits, &ready.granted, host.grant_resources())?,
        limits.fuel,
    )?;
    let linker = host.linker_for_world(PluginWorld::Context, &ready.granted)?;
    let bindings = ContextPlugin::instantiate_async(&mut store, ready.component(), &linker)
        .await
        .map_err(|error| map_wasmtime_error(&error, cancel.is_cancelled()))?;
    Ok((store, bindings))
}

fn map_guest_result<T, U>(
    result: Result<
        Result<T, crate::bindings::toolset::finstack::ai_types::types::PluginError>,
        wasmtime::Error,
    >,
    cancel: &finstack_ai_runtime::CancellationSignal,
    ok: impl FnOnce(T) -> U,
) -> Result<U, PluginHostError> {
    match result {
        Ok(Ok(value)) => Ok(ok(value)),
        Ok(Err(error)) => Err(PluginHostError::Mapped(format_plugin_error(
            &wit_plugin_error(error),
        ))),
        Err(error) => Err(map_wasmtime_error(&error, cancel.is_cancelled())),
    }
}

async fn list_tools(
    host: &PluginHost,
    ready: &ReadyWasm,
    exclusive: &Semaphore,
    serialized: &SerializedToolset,
    cancel: &finstack_ai_runtime::CancellationSignal,
    deadline: Option<Timestamp>,
) -> Result<finstack_ai_wit::ToolCatalog, PluginHostError> {
    match host.instance_policy() {
        InstancePolicy::Exclusive => {
            let _permit = exclusive
                .try_acquire()
                .map_err(|_| PluginHostError::InstanceLimit)?;
            with_cancellation(host.engine(), cancel, deadline, async {
                let (mut store, bindings) = instantiate_toolset(host, ready, cancel).await?;
                map_guest_result(
                    bindings
                        .finstack_ai_toolset_toolset()
                        .call_list_tools(&mut store)
                        .await,
                    cancel,
                    wit_tool_catalog,
                )
            })
            .await
        }
        InstancePolicy::Serialized => {
            let mut slot = serialized
                .try_lock()
                .map_err(|_| PluginHostError::InstanceLimit)?;
            with_cancellation(host.engine(), cancel, deadline, async {
                if slot.is_none() {
                    *slot = Some(instantiate_toolset(host, ready, cancel).await?);
                }
                let (store, bindings) = slot.as_mut().expect("serialized slot");
                map_guest_result(
                    bindings
                        .finstack_ai_toolset_toolset()
                        .call_list_tools(store)
                        .await,
                    cancel,
                    wit_tool_catalog,
                )
            })
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn call_tool(
    host: &PluginHost,
    ready: &ReadyWasm,
    exclusive: &Semaphore,
    serialized: &SerializedToolset,
    cancel: &finstack_ai_runtime::CancellationSignal,
    deadline: Option<Timestamp>,
    context: &finstack_ai_wit::CallContext,
    tool_id: &str,
    args: &[u8],
) -> Result<finstack_ai_wit::ToolResult, PluginHostError> {
    let wasm_ctx = wasm_call_context(context);
    match host.instance_policy() {
        InstancePolicy::Exclusive => {
            let _permit = exclusive
                .try_acquire()
                .map_err(|_| PluginHostError::InstanceLimit)?;
            with_cancellation(host.engine(), cancel, deadline, async {
                let (mut store, bindings) = instantiate_toolset(host, ready, cancel).await?;
                map_guest_result(
                    bindings
                        .finstack_ai_toolset_toolset()
                        .call_call(&mut store, &wasm_ctx, tool_id, args)
                        .await,
                    cancel,
                    wit_tool_result,
                )
            })
            .await
        }
        InstancePolicy::Serialized => {
            let mut slot = serialized
                .try_lock()
                .map_err(|_| PluginHostError::InstanceLimit)?;
            with_cancellation(host.engine(), cancel, deadline, async {
                if slot.is_none() {
                    *slot = Some(instantiate_toolset(host, ready, cancel).await?);
                }
                let (store, bindings) = slot.as_mut().expect("serialized slot");
                map_guest_result(
                    bindings
                        .finstack_ai_toolset_toolset()
                        .call_call(store, &wasm_ctx, tool_id, args)
                        .await,
                    cancel,
                    wit_tool_result,
                )
            })
            .await
        }
    }
}

async fn collect_items(
    host: &PluginHost,
    ready: &ReadyWasm,
    exclusive: &Semaphore,
    serialized: &SerializedContext,
    cancel: &finstack_ai_runtime::CancellationSignal,
    deadline: Option<Timestamp>,
    query: &finstack_ai_wit::ContextQuery,
) -> Result<Vec<finstack_ai_wit::generated::ContextItem>, PluginHostError> {
    let wasm_query = wasm_context_query(query);
    match host.instance_policy() {
        InstancePolicy::Exclusive => {
            let _permit = exclusive
                .try_acquire()
                .map_err(|_| PluginHostError::InstanceLimit)?;
            with_cancellation(host.engine(), cancel, deadline, async {
                let (mut store, bindings) = instantiate_context(host, ready, cancel).await?;
                map_guest_result(
                    bindings
                        .finstack_ai_context_context_provider()
                        .call_collect(&mut store, &wasm_query)
                        .await,
                    cancel,
                    |items| items.into_iter().map(wit_context_item).collect(),
                )
            })
            .await
        }
        InstancePolicy::Serialized => {
            let mut slot = serialized
                .try_lock()
                .map_err(|_| PluginHostError::InstanceLimit)?;
            with_cancellation(host.engine(), cancel, deadline, async {
                if slot.is_none() {
                    *slot = Some(instantiate_context(host, ready, cancel).await?);
                }
                let (store, bindings) = slot.as_mut().expect("serialized slot");
                map_guest_result(
                    bindings
                        .finstack_ai_context_context_provider()
                        .call_collect(store, &wasm_query)
                        .await,
                    cancel,
                    |items| items.into_iter().map(wit_context_item).collect(),
                )
            })
            .await
        }
    }
}

fn tool_error(error: &PluginHostError, metadata: Metadata) -> ToolError {
    ToolError::try_new(
        error.code(),
        ErrorCategory::Plugin,
        false,
        error.to_string(),
        metadata,
    )
    .unwrap_or_else(|fallback| fallback)
}

fn context_error(error: &PluginHostError, metadata: Metadata) -> ContextError {
    ContextError::try_new(
        error.code(),
        ErrorCategory::Plugin,
        error.to_string(),
        metadata,
    )
    .unwrap_or_else(|fallback| fallback)
}

fn plugin_metadata(ready: &ReadyWasm) -> Metadata {
    let granted: Vec<&str> = ready.granted.iter().map(String::as_str).collect();
    let encoded = serde_json::json!({
        "plugin.identity": ready.manifest.identity.as_str(),
        "plugin.manifest_digest": ready.manifest.digest,
        "plugin.granted_permissions": granted,
    });
    Metadata::parse(serde_json::to_vec(&encoded).unwrap_or_else(|_| b"{}".to_vec()))
        .unwrap_or_else(|_| Metadata::empty())
}

#[cfg(test)]
mod tests {
    use super::construction_context;
    use crate::error::PluginHostError;
    use crate::host::{InstancePolicy, PluginHost, PluginHostConfig};
    use finstack_ai_wit::manifest::manifest_digest_hex;
    use finstack_ai_wit::parse_manifest;

    fn manifest_bytes(identity: &str, worlds: &[&str]) -> Vec<u8> {
        let owned: Vec<String> = worlds.iter().map(|world| (*world).to_owned()).collect();
        let digest = manifest_digest_hex(identity, "0.0.4", &owned).expect("digest");
        serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json")
    }

    #[test]
    fn world_mismatch_is_rejected_before_compile() {
        let host = PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 1).expect("cfg"),
        )
        .expect("host");
        let manifest = parse_manifest(&manifest_bytes(
            "finstack.plugin.echo.toolset",
            &["context-plugin"],
        ))
        .expect("manifest");
        let Err(error) = host.load(
            b"\0asm not-a-component",
            manifest,
            crate::host::PluginWorld::Toolset,
        ) else {
            panic!("mismatch");
        };
        assert_eq!(error.code(), "plugin_registration_invalid");
        let _ = construction_context;
        let _ = PluginHostError::Timeout;
    }

    #[test]
    fn reference_context_item_bytes() {
        use finstack_ai_runtime::{
            ContentBlock, ContextAuthority, ContextItemKind, ContextProvenance, Sensitivity,
            TextBlock,
        };
        use finstack_ai_wit::encode_guest_item;
        use std::sync::Arc;

        let quoted = encode_guest_item(
            ContextItemKind::QuotedSource,
            vec![ContentBlock::Text(
                TextBlock::try_new("reference quoted source").expect("text"),
            )],
            ContextProvenance {
                source_id: Arc::from("finstack.plugin.reference.context"),
                source_ref: None,
                external: true,
            },
            ContextAuthority::Untrusted,
            1,
            8,
            Sensitivity::Internal,
            false,
        )
        .expect("quoted");
        let reference = encode_guest_item(
            ContextItemKind::Reference,
            vec![ContentBlock::Text(
                TextBlock::try_new("reference locator").expect("text"),
            )],
            ContextProvenance {
                source_id: Arc::from("finstack.plugin.reference.context"),
                source_ref: None,
                external: true,
            },
            ContextAuthority::Untrusted,
            0,
            6,
            Sensitivity::Internal,
            false,
        )
        .expect("reference");
        assert_eq!(quoted.estimated_tokens, 8);
        assert_eq!(reference.estimated_tokens, 6);
        assert_eq!(quoted.bytes, 50);
        assert_eq!(reference.bytes, 44);
        assert!(!quoted.item_json.is_empty());
        assert!(!reference.item_json.is_empty());
    }
}
