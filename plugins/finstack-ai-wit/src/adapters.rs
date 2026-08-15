//! In-process adapters that register WIT guests as ordinary SDK ports.

use std::sync::Arc;

use finstack_ai::{
    ComponentConstructionContext, Extension, ExtensionDescriptor, LifecycleBinding, ReadyComponent,
    Registrar, RegistrationError, RegistrationMetadata,
};
use finstack_ai_runtime::{
    ComponentRef, ContextCallContext, ContextContribution, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest, Digest, ErrorCategory, InvocationRecovery, Metadata,
    PortFuture, RawJson, ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolSpec,
    ToolStreamItem, Toolset, ToolsetDescriptor, ValidatedToolCall, Version,
};
use futures_util::stream;

use crate::context_mapping::{map_context_item, map_query};
use crate::error::WitMapError;
use crate::generated::{GuestContextProvider, GuestToolset};
use crate::lifecycle::{NoopPluginHooks, PluginGuestHooks, PluginLifecycle, PluginLifecycleError};
use crate::manifest::PluginManifest;
use crate::mapping::{register_catalog, sanitize_call_context};
use crate::reference::{ReferenceContextProvider, ReferenceToolset};

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

/// `ContextProvider` adapter over an in-process `GuestContextProvider`.
pub struct WitContextAdapter<G> {
    guest: G,
    descriptor: ContextProviderDescriptor,
    lifecycle: Arc<PluginLifecycle>,
    limits: Option<crate::manifest::PluginResourceLimits>,
}

impl<G: GuestContextProvider + Send + Sync + 'static> WitContextAdapter<G> {
    /// Construct and initialize one context adapter.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLifecycleError`] when host-side initialize or warmup fails.
    pub fn try_new(
        guest: G,
        manifest: &PluginManifest,
        construction: &ComponentConstructionContext,
        hooks: Arc<dyn PluginGuestHooks>,
    ) -> Result<Self, PluginLifecycleError> {
        require_world(manifest, "context-plugin")
            .map_err(|error| map_lifecycle_registration(&error))?;
        let lifecycle = Arc::new(PluginLifecycle::new(hooks));
        lifecycle.run_construction(construction)?;
        Ok(Self {
            guest,
            descriptor: ContextProviderDescriptor {
                invocation: finstack_ai_runtime::ComponentInvocation {
                    component: manifest.identity.clone(),
                    version: adapter_version(manifest),
                    configuration_digest: Digest::raw_json(b"{}"),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                trusted_application_instructions: false,
                metadata: Metadata::empty(),
            },
            lifecycle,
            limits: manifest.resource_limits.clone(),
        })
    }

    /// Lifecycle hooks attached to the ready component.
    #[must_use]
    pub fn lifecycle(&self) -> Arc<PluginLifecycle> {
        Arc::clone(&self.lifecycle)
    }
}

impl<G: GuestContextProvider + Send + Sync + 'static> ContextProvider for WitContextAdapter<G> {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let query = match map_query(&ctx.run, &request, self.limits.as_ref()) {
            Ok(query) => query,
            Err(error) => {
                let error = context_error(&error);
                return Box::pin(async move { Err(error) });
            }
        };
        let items = match self.guest.collect(&query) {
            Ok(items) => items,
            Err(error) => {
                let error = guest_context_error(error);
                return Box::pin(async move { Err(error) });
            }
        };
        let descriptor = self.descriptor.clone();
        Box::pin(async move {
            let native = items
                .iter()
                .map(|item| map_context_item(item, &descriptor))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| context_error(&error))?;
            ContextContribution::try_new(native, None::<&str>).map_err(|_| {
                context_error(&WitMapError::ContextItemInvalid(
                    "context contribution is invalid",
                ))
            })
        })
    }
}

/// `Toolset` adapter over an in-process `GuestToolset`.
pub struct WitToolsetAdapter<G> {
    guest: G,
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    lifecycle: Arc<PluginLifecycle>,
}

impl<G: GuestToolset + Send + Sync + 'static> WitToolsetAdapter<G> {
    /// Construct and initialize one toolset adapter.
    ///
    /// # Errors
    ///
    /// Returns [`PluginLifecycleError`] when catalog mapping or lifecycle fails.
    pub fn try_new(
        guest: G,
        manifest: &PluginManifest,
        construction: &ComponentConstructionContext,
        hooks: Arc<dyn PluginGuestHooks>,
    ) -> Result<Self, PluginLifecycleError> {
        require_world(manifest, "toolset-plugin")
            .map_err(|error| map_lifecycle_registration(&error))?;
        let lifecycle = Arc::new(PluginLifecycle::new(hooks));
        lifecycle.run_construction(construction)?;
        let catalog = guest
            .list_tools()
            .map_err(|_| PluginLifecycleError::InitializeFailed("guest list-tools failed"))?;
        let tools = register_catalog(&catalog)
            .map_err(|_| PluginLifecycleError::InitializeFailed("catalog mapping failed"))?;
        Ok(Self {
            guest,
            descriptor: ToolsetDescriptor {
                name: Arc::from(manifest.identity.as_str()),
                metadata: Metadata::empty(),
            },
            tools: tools.into(),
            lifecycle,
        })
    }

    /// Lifecycle hooks attached to the ready component.
    #[must_use]
    pub fn lifecycle(&self) -> Arc<PluginLifecycle> {
        Arc::clone(&self.lifecycle)
    }
}

impl<G: GuestToolset + Send + Sync + 'static> Toolset for WitToolsetAdapter<G> {
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
        let context = sanitize_call_context(&ctx.run);
        let result = self.guest.call(
            &context,
            call.tool_id.as_str(),
            call.call.arguments().as_bytes(),
        );
        Box::pin(async move {
            let result = result.map_err(|error| {
                let code = if error.code.is_empty() {
                    "plugin_lifecycle_failed"
                } else {
                    error.code.as_str()
                };
                tool_error(code, &error.message)
            })?;
            let output = RawJson::parse(&result.content_json)
                .map_err(|_| tool_error("plugin_result_invalid", "tool result JSON is invalid"))?;
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

/// Extension that registers the adapters named by one experimental manifest.
pub struct WitPluginExtension {
    manifest: PluginManifest,
    context: Option<Arc<dyn ContextProvider>>,
    context_lifecycle: Option<Arc<PluginLifecycle>>,
    toolset: Option<Arc<dyn Toolset>>,
    toolset_lifecycle: Option<Arc<PluginLifecycle>>,
}

impl WitPluginExtension {
    /// Register the in-process reference context guest.
    ///
    /// # Errors
    ///
    /// Returns [`WitMapError`] when the manifest does not declare `context-plugin`.
    pub fn reference_context(manifest: PluginManifest) -> Result<Self, WitMapError> {
        let construction = construction_context(&manifest);
        let adapter = WitContextAdapter::try_new(
            ReferenceContextProvider::new(),
            &manifest,
            &construction,
            Arc::new(NoopPluginHooks),
        )
        .map_err(|error| map_wit(&error))?;
        Ok(Self {
            context_lifecycle: Some(adapter.lifecycle()),
            context: Some(Arc::new(adapter)),
            toolset: None,
            toolset_lifecycle: None,
            manifest,
        })
    }

    /// Register the in-process reference toolset guest.
    ///
    /// # Errors
    ///
    /// Returns [`WitMapError`] when the manifest does not declare `toolset-plugin`.
    pub fn reference_toolset(manifest: PluginManifest) -> Result<Self, WitMapError> {
        let construction = construction_context(&manifest);
        let adapter = WitToolsetAdapter::try_new(
            ReferenceToolset::new(),
            &manifest,
            &construction,
            Arc::new(NoopPluginHooks),
        )
        .map_err(|error| map_wit(&error))?;
        Ok(Self {
            toolset_lifecycle: Some(adapter.lifecycle()),
            toolset: Some(Arc::new(adapter)),
            context: None,
            context_lifecycle: None,
            manifest,
        })
    }
}

impl Extension for WitPluginExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
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
                    message: Arc::from("toolset adapter is missing lifecycle hooks"),
                });
            }
        }
        if self.context.is_none() && self.toolset.is_none() {
            return Err(RegistrationError::InvalidDescriptor {
                message: Arc::from("plugin extension registered no port"),
            });
        }
        Ok(())
    }
}

fn require_world(manifest: &PluginManifest, world: &str) -> Result<(), WitMapError> {
    if manifest.worlds.iter().any(|declared| declared == world) {
        Ok(())
    } else {
        Err(WitMapError::ManifestInvalid(
            "kind does not match a declared capability world",
        ))
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

fn map_lifecycle_registration(error: &WitMapError) -> PluginLifecycleError {
    match error {
        WitMapError::ManifestInvalid(_) => {
            PluginLifecycleError::InitializeFailed("kind/world mismatch")
        }
        _ => PluginLifecycleError::InitializeFailed("plugin registration is invalid"),
    }
}

fn map_wit(error: &PluginLifecycleError) -> WitMapError {
    match error {
        PluginLifecycleError::InitializeFailed(_) => {
            WitMapError::ManifestInvalid("plugin initialize failed")
        }
        PluginLifecycleError::WarmupFailed(_) => {
            WitMapError::ManifestInvalid("plugin warmup failed")
        }
        PluginLifecycleError::Timeout => WitMapError::ManifestInvalid("plugin lifecycle timed out"),
        PluginLifecycleError::Failed(_) => WitMapError::ManifestInvalid("plugin lifecycle failed"),
    }
}

fn context_error(error: &WitMapError) -> ContextError {
    ContextError::try_new(
        error.code(),
        match error {
            WitMapError::PrivateSuspension => ErrorCategory::Plugin,
            WitMapError::PayloadTooLarge { .. } => ErrorCategory::Limit,
            _ => ErrorCategory::Validation,
        },
        error.to_string(),
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

fn guest_context_error(error: crate::generated::PluginError) -> ContextError {
    let code = if error.code.is_empty() {
        "plugin_lifecycle_failed"
    } else {
        error.code.as_str()
    };
    ContextError::try_new(
        code,
        ErrorCategory::Plugin,
        error.message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

fn tool_error(code: &str, message: &str) -> ToolError {
    ToolError::try_new(
        code,
        ErrorCategory::Plugin,
        false,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests {
    use super::WitPluginExtension;
    use crate::manifest::{manifest_digest_hex, parse_manifest};
    use finstack_ai::{
        AgentComponentSelection, AgentConstructionContext, ComponentSelector, Extension,
        ReadyComponent, Registrar, RegistrationMetadata, ResolveRequest,
    };
    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, ComponentId, ContentBlock, ContextBudget,
        ContextCallContext, ContextItemKind, ContextOverflowPolicy, ContextRequest, Digest,
        EffectId, LaneId, Metadata, Model, ModelContextProfile, ModelName, OperationLocator,
        PrincipalRef, RecordedContextContribution, RunCallContext, RunId, SessionId, TextBlock,
        TokenEstimatorRef, TokenEstimatorSource, Version, assemble_context,
    };
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
    use finstack_ai_test::ScriptedModel;
    use std::sync::Arc;

    const MODEL_VERSION: Version = Version {
        major: 1,
        minor: 0,
        patch: 0,
    };

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

    fn context_manifest() -> crate::manifest::PluginManifest {
        parse_manifest(&manifest_bytes(
            "finstack.plugin.reference.context",
            &["context-plugin"],
        ))
        .expect("manifest")
    }

    fn toolset_manifest() -> crate::manifest::PluginManifest {
        parse_manifest(&manifest_bytes(
            "finstack.plugin.reference.toolset",
            &["toolset-plugin"],
        ))
        .expect("manifest")
    }

    fn component(value: &str) -> ComponentId {
        ComponentId::parse(value).expect("component")
    }

    fn profile() -> ModelContextProfile {
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("scripted-1").expect("model"),
            hard_input_bytes: 1_000,
            context_window_tokens: 100,
            max_output_tokens: 20,
            reserved_output_tokens: 20,
            provider_overhead_tokens: 5,
            estimator: TokenEstimatorRef {
                id: Arc::from("bytes-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        }
    }

    struct ModelExtension {
        model: Arc<ScriptedModel>,
    }

    impl Extension for ModelExtension {
        fn descriptor(&self) -> finstack_ai::ExtensionDescriptor {
            finstack_ai::ExtensionDescriptor::trusted_in_process(
                component("test.extension.model"),
                MODEL_VERSION,
            )
        }

        fn register(
            &self,
            registrar: &mut Registrar,
        ) -> Result<(), finstack_ai::RegistrationError> {
            let handle: Arc<dyn Model> = self.model.clone();
            registrar.model(
                RegistrationMetadata::new(component("test.model.scripted"), MODEL_VERSION),
                ReadyComponent::new(handle),
            )
        }
    }

    struct StoreExtension {
        store: Arc<MemoryJournalStore>,
    }

    impl Extension for StoreExtension {
        fn descriptor(&self) -> finstack_ai::ExtensionDescriptor {
            finstack_ai::ExtensionDescriptor::trusted_in_process(
                component("test.extension.store"),
                MODEL_VERSION,
            )
        }

        fn register(
            &self,
            registrar: &mut Registrar,
        ) -> Result<(), finstack_ai::RegistrationError> {
            let handle: Arc<dyn finstack_ai_runtime::JournalStore> = self.store.clone();
            registrar.store(
                RegistrationMetadata::new(component("test.store.memory"), MODEL_VERSION),
                ReadyComponent::new(handle),
            )
        }
    }

    fn run_context() -> RunCallContext {
        RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                authentication_method: "secret-method".into(),
                assurance_level: "high".into(),
                roles: ["admin".into()].into(),
                permitted_scopes: ["tenant-a".into()].into(),
                safe_claims: Metadata::empty(),
                policy_version: "policy-v1".into(),
                decision_id: "decision-v1".into(),
            },
            effect_id: EffectId::from_bytes([4; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        }
    }

    fn request() -> ContextRequest {
        ContextRequest {
            session_id: SessionId::from_bytes([1; 16]),
            lane_id: LaneId::from_bytes([2; 16]),
            run_id: RunId::from_bytes([3; 16]),
            user_input: Arc::from([ContentBlock::Text(
                TextBlock::try_new("hello").expect("text"),
            )]),
            recent_history: Arc::from([]),
            budget: ContextBudget {
                max_items: 8,
                max_tokens: 128,
                max_bytes: 16_384,
                overflow: ContextOverflowPolicy::Reject,
            },
            active_capabilities: Arc::from([]),
        }
    }

    #[test]
    fn kind_world_mismatch_is_rejected() {
        let Err(error) = WitPluginExtension::reference_context(toolset_manifest()) else {
            panic!("kind/world mismatch must fail");
        };
        assert_eq!(error.code(), "plugin_registration_invalid");
    }

    #[tokio::test]
    async fn resolved_context_provider_collects_through_assemble_context() {
        let extension = WitPluginExtension::reference_context(context_manifest()).expect("ext");
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&ModelExtension {
                model: Arc::new(ScriptedModel::from_plans(profile(), Vec::new())),
            })
            .expect("model");
        registrar
            .register_extension(&StoreExtension {
                store: Arc::new(
                    MemoryJournalStore::try_new(MemoryStoreLimits {
                        sessions: 4,
                        batches_per_session: 16,
                        records_per_session: 64,
                        snapshot_bytes: 4_096,
                    })
                    .expect("store"),
                ),
            })
            .expect("store");
        registrar.register_extension(&extension).expect("plugin");
        let mut selection = AgentComponentSelection::new(
            ComponentSelector::Component(finstack_ai_runtime::ComponentRef::new(
                component("test.model.scripted"),
                Some(MODEL_VERSION),
            )),
            ComponentSelector::Component(finstack_ai_runtime::ComponentRef::new(
                component("test.store.memory"),
                Some(MODEL_VERSION),
            )),
        );
        selection.context_providers = Arc::from([ComponentSelector::Component(
            finstack_ai_runtime::ComponentRef::new(
                component("finstack.plugin.reference.context"),
                Some(Version {
                    major: 0,
                    minor: 0,
                    patch: 4,
                }),
            ),
        )]);
        let resolved = registrar
            .into_registry()
            .resolve(
                ResolveRequest::new(component("test.agent.source"), selection),
                AgentConstructionContext::new(),
            )
            .await
            .expect("resolve");
        let plan = resolved.run_plan();
        let provider = plan.context_providers()[0].handle();
        let contribution = provider
            .collect(
                ContextCallContext {
                    run: run_context(),
                    provider_index: 0,
                    chain_digest: Digest::raw_json(b"{}"),
                },
                request(),
            )
            .await
            .expect("collect");
        assert_eq!(contribution.items.len(), 2);
        assert!(contribution.items.iter().all(|item| {
            item.authority == finstack_ai_runtime::ContextAuthority::Untrusted
                && matches!(
                    item.kind,
                    ContextItemKind::QuotedSource | ContextItemKind::Reference
                )
        }));
        let assembled = assemble_context(
            vec![RecordedContextContribution {
                component: component("finstack.plugin.reference.context"),
                provider_index: 0,
                contribution,
            }],
            request().budget,
        )
        .expect("assemble");
        assert_eq!(assembled.items.len(), 2);
        assert!(
            assembled
                .items
                .iter()
                .all(|item| item.authority == finstack_ai_runtime::ContextAuthority::Untrusted)
        );
        let reports = resolved.health().await;
        assert!(
            reports
                .iter()
                .any(|report| report.result.as_ref().expect("health").ready)
        );
    }

    #[tokio::test]
    async fn toolset_adapter_registers_as_ordinary_port() {
        let extension = WitPluginExtension::reference_toolset(toolset_manifest()).expect("ext");
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&ModelExtension {
                model: Arc::new(ScriptedModel::from_plans(profile(), Vec::new())),
            })
            .expect("model");
        registrar
            .register_extension(&StoreExtension {
                store: Arc::new(
                    MemoryJournalStore::try_new(MemoryStoreLimits {
                        sessions: 4,
                        batches_per_session: 16,
                        records_per_session: 64,
                        snapshot_bytes: 4_096,
                    })
                    .expect("store"),
                ),
            })
            .expect("store");
        registrar.register_extension(&extension).expect("plugin");
        let mut selection = AgentComponentSelection::new(
            ComponentSelector::Component(finstack_ai_runtime::ComponentRef::new(
                component("test.model.scripted"),
                Some(MODEL_VERSION),
            )),
            ComponentSelector::Component(finstack_ai_runtime::ComponentRef::new(
                component("test.store.memory"),
                Some(MODEL_VERSION),
            )),
        );
        selection.toolsets = Arc::from([ComponentSelector::Component(
            finstack_ai_runtime::ComponentRef::new(
                component("finstack.plugin.reference.toolset"),
                Some(Version {
                    major: 0,
                    minor: 0,
                    patch: 4,
                }),
            ),
        )]);
        let resolved = registrar
            .into_registry()
            .resolve(
                ResolveRequest::new(component("test.agent.source"), selection),
                AgentConstructionContext::new(),
            )
            .await
            .expect("resolve");
        let plan = resolved.run_plan();
        let toolset = plan.toolsets()[0].handle();
        assert_eq!(toolset.tools().len(), 2);
        let reports = resolved.health().await;
        assert!(
            reports
                .iter()
                .any(|report| report.result.as_ref().expect("health").ready)
        );
        resolved
            .shutdown(AgentConstructionContext::new())
            .await
            .iter()
            .for_each(|report| {
                let _ = report;
            });
    }
}
