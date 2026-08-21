use core::sync::atomic::{AtomicUsize, Ordering};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, ComponentRef, Digest, Metadata, RawJson, Version};
use finstack_ai_runtime::{
    JournalStore, Model, ModelContextProfile, ModelName, Observer, ObserverDescriptor,
    ObserverPayloadMode, PortFuture, TokenEstimatorRef, TokenEstimatorSource,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::ScriptedModel;

use super::*;

const VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

fn component(value: &str) -> ComponentId {
    ComponentId::parse(value).expect("valid namespaced test component")
}

fn exact(value: &str) -> ComponentSelector {
    ComponentSelector::Component(ComponentRef::new(component(value), Some(VERSION)))
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("scripted-1").expect("valid model name"),
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

fn model() -> Arc<ScriptedModel> {
    Arc::new(ScriptedModel::from_plans(profile(), Vec::new()))
}

fn store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 16,
            records_per_session: 64,
            snapshot_bytes: 4_096,
        })
        .expect("valid memory store limits"),
    )
}

struct ModelExtension {
    source: ComponentId,
    id: ComponentId,
    alias: Option<ComponentAlias>,
    model: Arc<ScriptedModel>,
}

impl Extension for ModelExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor::trusted_in_process(self.source.clone(), VERSION)
    }

    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
        let mut metadata = RegistrationMetadata::new(self.id.clone(), VERSION);
        if let Some(alias) = &self.alias {
            metadata = metadata.with_alias(alias.clone())?;
        }
        let handle: Arc<dyn Model> = self.model.clone();
        registrar.model(metadata, ReadyComponent::new(handle))
    }
}

struct StoreExtension {
    source: ComponentId,
    id: ComponentId,
    store: Arc<MemoryJournalStore>,
}

impl Extension for StoreExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor::trusted_in_process(self.source.clone(), VERSION)
    }

    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
        let handle: Arc<dyn JournalStore> = self.store.clone();
        registrar.store(
            RegistrationMetadata::new(self.id.clone(), VERSION),
            ReadyComponent::new(handle),
        )
    }
}

fn extensions() -> (ModelExtension, StoreExtension) {
    (
        ModelExtension {
            source: component("test.extension.model"),
            id: component("test.model.scripted"),
            alias: Some(ComponentAlias::try_new("primary").expect("valid alias")),
            model: model(),
        },
        StoreExtension {
            source: component("test.extension.store"),
            id: component("test.store.memory"),
            store: store(),
        },
    )
}

fn request_with_model(model: ComponentSelector) -> ResolveRequest {
    ResolveRequest::new(
        component("test.agent.source"),
        AgentComponentSelection::new(model, exact("test.store.memory")),
    )
}

fn bundle_spec(
    agent: crate::AgentSpec,
    capability: crate::CapabilitySpec,
    requirements: Vec<crate::BundleRequirement>,
    conflicts: Vec<crate::BundleConflict>,
) -> crate::BundleSpec {
    crate::BundleSpec {
        schema_version: crate::BUNDLE_SCHEMA_VERSION,
        id: finstack_ai_kernel::BundleId::parse("test.bundle.composition").expect("bundle id"),
        version: VERSION,
        agents: Arc::from([agent]),
        capabilities: Arc::from([capability]),
        requirements: requirements.into(),
        conflicts: conflicts.into(),
        defaults: crate::BundleDefaults::default(),
        config_schema: None,
        compatibility: crate::CompatibilityRequirements::default(),
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one vertical acceptance fixture covers resolve, activation rebuild, digest invalidation, and exact lock re-import"
)]
async fn bundle_resolution_locks_capabilities_and_rebuilds_application_plan() {
    let (model_extension, store_extension) = extensions();
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&model_extension)
        .expect("model extension");
    registrar
        .register_extension(&store_extension)
        .expect("store extension");
    let mut registry = registrar.into_registry();

    let capability_id =
        finstack_ai_kernel::CapabilityId::parse("test.capability.research").expect("capability id");
    let agent = crate::AgentBuilder::new(
        finstack_ai_kernel::AgentId::parse("test.agent.research").expect("agent id"),
        ComponentRef::new(component("test.model.scripted"), Some(VERSION)),
        ComponentRef::new(component("test.store.memory"), Some(VERSION)),
    )
    .capabilities(Arc::from([crate::CapabilityRef {
        id: capability_id.clone(),
        bundle: None,
    }]))
    .build()
    .expect("agent spec");
    let capability = crate::CapabilitySpec {
        id: capability_id.clone(),
        description: Arc::from("Research instruction set"),
        instructions: Arc::from([
            crate::InstructionSpec::try_new("Cite primary sources.").expect("instruction")
        ]),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation: crate::CapabilityActivation::Application,
    };
    let bundle = bundle_spec(
        agent,
        capability,
        vec![
            crate::BundleRequirement::RequiredComponent {
                component: component("test.model.scripted"),
                version: crate::VersionRequirement::Exact { version: VERSION },
            },
            crate::BundleRequirement::RequiredComponent {
                component: component("test.store.memory"),
                version: crate::VersionRequirement::CompatibleMajor { major: 1 },
            },
        ],
        vec![],
    );
    let bundle_id = bundle.id.clone();
    let agent_id = bundle.agents[0].id.clone();
    let mut catalog = crate::BundleCatalog::default();
    catalog.install(bundle).expect("install bundle");
    let bundle_resolver = crate::BundleResolver::new(
        &catalog,
        Version {
            major: 0,
            minor: 0,
            patch: 1,
        },
        BTreeSet::new(),
        crate::RuntimeServices::default(),
    );
    let bundle_agent = bundle_resolver
        .resolve_agent(
            &mut registry,
            &bundle_id,
            &agent_id,
            BTreeMap::new(),
            AgentConstructionContext::new(),
        )
        .await
        .expect("resolve bundle agent");
    let base_lock = bundle_agent.lock().expect("base lock");
    assert_eq!(base_lock.capabilities.len(), 1);
    assert!(!base_lock.capabilities[0].active);
    let base_fingerprint = base_lock.fingerprint().expect("base fingerprint");

    let activated = bundle_resolver
        .activate_application(
            &mut registry,
            &bundle_agent,
            [capability_id],
            AgentConstructionContext::new(),
        )
        .await
        .expect("activate application capability");
    let activated_lock = activated.lock().expect("activated lock");
    assert!(activated_lock.capabilities[0].active);
    assert_ne!(
        activated_lock.fingerprint().expect("activated fingerprint"),
        base_fingerprint,
        "active capability membership must invalidate plan/checkpoint digests"
    );

    let exported = activated_lock.to_json().expect("lock export");
    let imported = crate::ResolvedAgentLock::from_json(&exported).expect("lock import");

    let mut wrong_config = imported.clone();
    wrong_config.effective_config_digest = Digest::raw_json(b"wrong-config");
    assert!(
        bundle_resolver
            .resolve_lock(
                &mut registry,
                &wrong_config,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "effective configuration selection is exact"
    );

    let mut wrong_schema = imported.clone();
    wrong_schema.schema_digests = Arc::from([Digest::raw_json(b"wrong-schema")]);
    assert!(
        bundle_resolver
            .resolve_lock(
                &mut registry,
                &wrong_schema,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "schema selection is exact"
    );

    let mut wrong_version = imported.clone();
    let mut components = wrong_version.components.to_vec();
    let selected = &components[0].component;
    components[0].component = ComponentRef::new(
        selected.id().clone(),
        Some(Version {
            major: 9,
            minor: 0,
            patch: 0,
        }),
    );
    wrong_version.components = components.into();
    assert!(
        bundle_resolver
            .resolve_lock(
                &mut registry,
                &wrong_version,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "component version selection is exact"
    );

    let reconstructed = bundle_resolver
        .resolve_lock(
            &mut registry,
            &imported,
            BTreeMap::new(),
            AgentConstructionContext::new(),
        )
        .await
        .expect("exact lock reconstruction");
    assert_eq!(reconstructed.lock().expect("lock").as_ref(), &imported);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one negative fixture proves conflict, missing service, and unresolved agent admission failures"
)]
async fn bundle_conflicts_unresolved_refs_and_missing_services_fail_before_start() {
    let (model_extension, store_extension) = extensions();
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&model_extension)
        .expect("model extension");
    registrar
        .register_extension(&store_extension)
        .expect("store extension");
    let mut registry = registrar.into_registry();
    let capability = crate::CapabilitySpec {
        id: finstack_ai_kernel::CapabilityId::parse("test.capability.required")
            .expect("capability"),
        description: Arc::from("Required capability"),
        instructions: Arc::from([]),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation: crate::CapabilityActivation::Always,
    };
    let agent = crate::AgentBuilder::new(
        finstack_ai_kernel::AgentId::parse("test.agent.required").expect("agent"),
        ComponentRef::new(component("test.model.scripted"), Some(VERSION)),
        ComponentRef::new(component("test.store.memory"), Some(VERSION)),
    )
    .build()
    .expect("agent spec");

    let conflicting = bundle_spec(
        agent.clone(),
        capability.clone(),
        vec![],
        vec![crate::BundleConflict::Component {
            component: component("test.model.scripted"),
        }],
    );
    let conflict_id = conflicting.id.clone();
    let conflict_agent = conflicting.agents[0].id.clone();
    let mut conflict_catalog = crate::BundleCatalog::default();
    conflict_catalog
        .install(conflicting)
        .expect("conflict bundle installs");
    let conflict_resolver = crate::BundleResolver::new(
        &conflict_catalog,
        VERSION,
        BTreeSet::new(),
        crate::RuntimeServices::default(),
    );
    assert!(
        conflict_resolver
            .resolve_agent(
                &mut registry,
                &conflict_id,
                &conflict_agent,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err()
    );

    let incompatible_bundle = bundle_spec(
        agent.clone(),
        capability.clone(),
        vec![crate::BundleRequirement::RequiredComponent {
            component: component("test.model.scripted"),
            version: crate::VersionRequirement::Exact {
                version: Version {
                    major: 2,
                    minor: 0,
                    patch: 0,
                },
            },
        }],
        vec![],
    );
    let incompatible_id = incompatible_bundle.id.clone();
    let incompatible_agent = incompatible_bundle.agents[0].id.clone();
    let mut incompatible_catalog = crate::BundleCatalog::default();
    incompatible_catalog
        .install(incompatible_bundle)
        .expect("incompatible bundle installs");
    let incompatible_resolver = crate::BundleResolver::new(
        &incompatible_catalog,
        VERSION,
        BTreeSet::new(),
        crate::RuntimeServices::default(),
    );
    assert!(
        incompatible_resolver
            .resolve_agent(
                &mut registry,
                &incompatible_id,
                &incompatible_agent,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "incompatible component versions must fail before start"
    );

    let mut unresolved_capability_agent = agent.clone();
    unresolved_capability_agent.capabilities = Arc::from([crate::CapabilityRef {
        id: finstack_ai_kernel::CapabilityId::parse("test.capability.missing")
            .expect("missing capability"),
        bundle: None,
    }]);
    let unresolved_capability_bundle = bundle_spec(
        unresolved_capability_agent,
        capability.clone(),
        vec![],
        vec![],
    );
    let unresolved_capability_id = unresolved_capability_bundle.id.clone();
    let unresolved_capability_agent_id = unresolved_capability_bundle.agents[0].id.clone();
    let mut unresolved_capability_catalog = crate::BundleCatalog::default();
    unresolved_capability_catalog
        .install(unresolved_capability_bundle)
        .expect("unresolved capability bundle installs");
    let unresolved_capability_resolver = crate::BundleResolver::new(
        &unresolved_capability_catalog,
        VERSION,
        BTreeSet::new(),
        crate::RuntimeServices::default(),
    );
    assert!(
        unresolved_capability_resolver
            .resolve_agent(
                &mut registry,
                &unresolved_capability_id,
                &unresolved_capability_agent_id,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "explicit capability refs must resolve to a locked definition"
    );

    let mut missing_store_agent = agent.clone();
    missing_store_agent.store = None;
    let missing_store_bundle = bundle_spec(missing_store_agent, capability.clone(), vec![], vec![]);
    let missing_store_id = missing_store_bundle.id.clone();
    let missing_store_agent_id = missing_store_bundle.agents[0].id.clone();
    let mut missing_store_catalog = crate::BundleCatalog::default();
    missing_store_catalog
        .install(missing_store_bundle)
        .expect("declarative bundle installs");
    let missing_store_resolver = crate::BundleResolver::new(
        &missing_store_catalog,
        VERSION,
        BTreeSet::new(),
        crate::RuntimeServices::default(),
    );
    assert!(
        missing_store_resolver
            .resolve_agent(
                &mut registry,
                &missing_store_id,
                &missing_store_agent_id,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "an executable resolved agent requires a journal store"
    );

    let service_bundle = bundle_spec(
        agent.clone(),
        capability.clone(),
        vec![crate::BundleRequirement::RequiredHostFeature {
            feature: crate::HostFeature::BudgetLedger,
        }],
        vec![],
    );
    let service_id = service_bundle.id.clone();
    let service_agent = service_bundle.agents[0].id.clone();
    let mut service_catalog = crate::BundleCatalog::default();
    service_catalog
        .install(service_bundle)
        .expect("service bundle installs");
    let service_resolver = crate::BundleResolver::new(
        &service_catalog,
        VERSION,
        BTreeSet::from([crate::HostFeature::BudgetLedger]),
        crate::RuntimeServices::default(),
    );
    assert!(
        service_resolver
            .resolve_agent(
                &mut registry,
                &service_id,
                &service_agent,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "required ledger must not be treated as optional"
    );
    assert!(
        service_resolver
            .resolve_agent(
                &mut registry,
                &service_id,
                &finstack_ai_kernel::AgentId::parse("test.agent.missing")
                    .expect("missing agent id"),
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "unresolved agent refs must fail before start"
    );

    let artifact_bundle = bundle_spec(
        agent,
        capability,
        vec![crate::BundleRequirement::RequiredHostFeature {
            feature: crate::HostFeature::ArtifactStore,
        }],
        vec![],
    );
    let artifact_id = artifact_bundle.id.clone();
    let artifact_agent = artifact_bundle.agents[0].id.clone();
    let mut artifact_catalog = crate::BundleCatalog::default();
    artifact_catalog
        .install(artifact_bundle)
        .expect("artifact bundle installs");
    let artifact_resolver = crate::BundleResolver::new(
        &artifact_catalog,
        VERSION,
        BTreeSet::from([crate::HostFeature::ArtifactStore]),
        crate::RuntimeServices::default(),
    );
    assert!(
        artifact_resolver
            .resolve_agent(
                &mut registry,
                &artifact_id,
                &artifact_agent,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .is_err(),
        "required artifact storage must not be treated as optional"
    );
}

#[tokio::test]
async fn duplicate_registration_and_missing_resolution_are_source_aware() {
    let (first, store_extension) = extensions();
    let duplicate = ModelExtension {
        source: component("test.extension.duplicate"),
        id: first.id.clone(),
        alias: None,
        model: model(),
    };
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&first)
        .expect("first extension registers");
    let error = registrar
        .register_extension(&duplicate)
        .expect_err("duplicate component must fail");
    match error {
        RegistrationError::Duplicate {
            existing_source,
            attempted_source,
            ..
        } => {
            assert_eq!(existing_source, component("test.extension.model"));
            assert_eq!(attempted_source, component("test.extension.duplicate"));
        }
        other => panic!("unexpected duplicate error: {other:?}"),
    }
    registrar
        .register_extension(&store_extension)
        .expect("store extension registers");
    let mut registry = registrar.into_registry();
    let missing = request_with_model(exact("test.model.missing"));
    let Err(error) = registry
        .resolve(missing, AgentConstructionContext::new())
        .await
    else {
        panic!("missing model must fail");
    };
    match error {
        AgentBuildError::MissingComponent {
            request_source,
            selector,
            expected_kind,
            ..
        } => {
            assert_eq!(request_source, component("test.agent.source"));
            assert_eq!(&*selector, "test.model.missing");
            assert_eq!(expected_kind, ComponentKind::Model);
        }
        other => panic!("unexpected build error: {other:?}"),
    }
}

#[tokio::test]
async fn resolution_is_stable_across_registration_order_and_expands_aliases() {
    let (model_extension, store_extension) = extensions();
    let mut forward = Registrar::new();
    forward
        .register_extension(&model_extension)
        .expect("model registers");
    forward
        .register_extension(&store_extension)
        .expect("store registers");

    let mut reverse = Registrar::new();
    reverse
        .register_extension(&store_extension)
        .expect("store registers");
    reverse
        .register_extension(&model_extension)
        .expect("model registers");

    let alias = ComponentSelector::Alias {
        alias: ComponentAlias::try_new("primary").expect("valid alias"),
        version: Some(VERSION),
    };
    let first = forward
        .into_registry()
        .resolve(
            request_with_model(alias.clone()),
            AgentConstructionContext::new(),
        )
        .await
        .expect("forward resolution succeeds");
    let second = reverse
        .into_registry()
        .resolve(request_with_model(alias), AgentConstructionContext::new())
        .await
        .expect("reverse resolution succeeds");
    assert_eq!(first.resolution_report(), second.resolution_report());
    assert_eq!(
        first.resolution_report().diagnostics[0].kind,
        ResolutionDiagnosticKind::AliasExpanded
    );
}

#[tokio::test]
async fn explicit_replacement_checks_and_records_the_existing_source() {
    struct ReplacementExtension {
        model: Arc<ScriptedModel>,
    }

    impl Extension for ReplacementExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            ExtensionDescriptor::trusted_in_process(
                component("test.extension.replacement"),
                VERSION,
            )
        }

        fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
            let model: Arc<dyn Model> = self.model.clone();
            registrar.model(
                RegistrationMetadata::new(component("test.model.scripted"), VERSION)
                    .replacing(component("test.extension.model")),
                ReadyComponent::new(model),
            )
        }
    }

    let (original, store_extension) = extensions();
    let replacement_model = model();
    let replacement = ReplacementExtension {
        model: Arc::clone(&replacement_model),
    };
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&original)
        .expect("original model registers");
    registrar
        .register_extension(&replacement)
        .expect("source-checked replacement registers");
    registrar
        .register_extension(&store_extension)
        .expect("store registers");
    assert_eq!(
        registrar.events[1].replaced_source,
        Some(component("test.extension.model"))
    );

    let agent = registrar
        .into_registry()
        .resolve(
            request_with_model(exact("test.model.scripted")),
            AgentConstructionContext::new(),
        )
        .await
        .expect("replacement resolves");
    let expected: Arc<dyn Model> = replacement_model;
    assert!(Arc::ptr_eq(
        &agent.run_plan().model().handle().shared_model(),
        &expected
    ));
}

#[tokio::test]
async fn resolved_plan_retains_direct_handles_without_registry_lookup() {
    let (model_extension, store_extension) = extensions();
    let expected_model: Arc<dyn Model> = model_extension.model.clone();
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&model_extension)
        .expect("model registers");
    registrar
        .register_extension(&store_extension)
        .expect("store registers");
    let mut registry = registrar.into_registry();
    let agent = registry
        .resolve(
            request_with_model(exact("test.model.scripted")),
            AgentConstructionContext::new(),
        )
        .await
        .expect("resolution succeeds");
    assert_eq!(registry.lookup_count, 2);
    let plan = agent.run_plan();
    assert!(Arc::ptr_eq(
        &plan.model().handle().shared_model(),
        &expected_model
    ));
    let _ = plan.model().handle().descriptor();
    let _ = plan.store().handle().health().await;
    let _ = agent.run_plan();
    assert_eq!(registry.lookup_count, 2);
}

#[tokio::test]
async fn ready_model_rejects_per_request_configuration() {
    let (model_extension, store_extension) = extensions();
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&model_extension)
        .expect("model registers");
    registrar
        .register_extension(&store_extension)
        .expect("store registers");
    let Err(error) = registrar
        .into_registry()
        .resolve(
            request_with_model(exact("test.model.scripted"))
                .with_configuration(
                    component("test.model.scripted"),
                    RawJson::parse(br#"{"temperature":0}"#).expect("config"),
                )
                .expect("request"),
            AgentConstructionContext::new(),
        )
        .await
    else {
        panic!("ready handle must fail closed");
    };
    assert_eq!(error.code(), AGENT_BUILD_CONFIGURATION_CONFLICT);
}

struct FactoryExtension {
    model: Arc<ScriptedModel>,
    constructions: Arc<AtomicUsize>,
}

impl Extension for FactoryExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor::trusted_in_process(component("test.extension.factory"), VERSION)
    }

    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
        let model = Arc::clone(&self.model);
        let constructions = Arc::clone(&self.constructions);
        let factory: Arc<dyn ComponentFactory<dyn Model>> = Arc::new(move |_context| {
            let model = Arc::clone(&model);
            let constructions = Arc::clone(&constructions);
            async move {
                constructions.fetch_add(1, Ordering::AcqRel);
                let handle: Arc<dyn Model> = model;
                Ok(ReadyComponent::new(handle))
            }
        });
        registrar.model_factory(
            RegistrationMetadata::new(component("test.model.factory"), VERSION),
            factory,
        )
    }
}

#[tokio::test]
async fn selected_factory_and_model_warmup_execute_once() {
    let scripted = model();
    let constructions = Arc::new(AtomicUsize::new(0));
    let factory_extension = FactoryExtension {
        model: Arc::clone(&scripted),
        constructions: Arc::clone(&constructions),
    };
    let (_, store_extension) = extensions();
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&factory_extension)
        .expect("factory registers");
    registrar
        .register_extension(&store_extension)
        .expect("store registers");
    let mut registry = registrar.into_registry();
    let mut first_ready = None;

    for expected in [
        ResolutionDiagnosticKind::FactoryConstructed,
        ResolutionDiagnosticKind::CachedFactoryReused,
    ] {
        let agent = registry
            .resolve(
                request_with_model(exact("test.model.factory")),
                AgentConstructionContext::new(),
            )
            .await
            .expect("factory resolution succeeds");
        assert_eq!(agent.resolution_report().diagnostics[0].kind, expected);
        let ready = Arc::clone(agent.run_plan().model().handle());
        if let Some(first) = &first_ready {
            assert!(Arc::ptr_eq(first, &ready));
        } else {
            first_ready = Some(ready);
        }
    }
    assert_eq!(constructions.load(Ordering::Acquire), 1);
    assert_eq!(scripted.warmup_count(), 1);
}

struct CountingLifecycle {
    shutdowns: AtomicUsize,
}

impl ComponentLifecycle for CountingLifecycle {
    fn health(&self) -> PortFuture<Result<ComponentHealth, LifecycleError>> {
        Box::pin(async {
            Ok(ComponentHealth {
                ready: true,
                detail: Arc::from("ready"),
            })
        })
    }

    fn shutdown(
        &self,
        _context: AgentConstructionContext,
    ) -> PortFuture<Result<(), LifecycleError>> {
        self.shutdowns.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn resolved_agent_owns_lifecycle_shutdown_at_most_once() {
    struct LifecycleExtension {
        model: Arc<ScriptedModel>,
        lifecycle: Arc<CountingLifecycle>,
    }

    impl Extension for LifecycleExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            ExtensionDescriptor::trusted_in_process(component("test.extension.lifecycle"), VERSION)
        }

        fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
            let model: Arc<dyn Model> = self.model.clone();
            let lifecycle: Arc<dyn ComponentLifecycle> = self.lifecycle.clone();
            registrar.model(
                RegistrationMetadata::new(component("test.model.lifecycle"), VERSION),
                ReadyComponent::new(model)
                    .with_lifecycle(LifecycleBinding::resolved_agent(lifecycle)),
            )
        }
    }

    let lifecycle = Arc::new(CountingLifecycle {
        shutdowns: AtomicUsize::new(0),
    });
    let extension = LifecycleExtension {
        model: model(),
        lifecycle: Arc::clone(&lifecycle),
    };
    let (_, store_extension) = extensions();
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&extension)
        .expect("lifecycle model registers");
    registrar
        .register_extension(&store_extension)
        .expect("store registers");
    let agent = registrar
        .into_registry()
        .resolve(
            request_with_model(exact("test.model.lifecycle")),
            AgentConstructionContext::new(),
        )
        .await
        .expect("resolution succeeds");

    let health = agent.health().await;
    assert_eq!(health.len(), 1);
    assert!(health[0].result.as_ref().expect("health succeeds").ready);
    assert_eq!(
        agent.shutdown(AgentConstructionContext::new()).await[0].outcome,
        ComponentShutdownOutcome::Completed
    );
    assert_eq!(
        agent.shutdown(AgentConstructionContext::new()).await[0].outcome,
        ComponentShutdownOutcome::AlreadyCompleted
    );
    assert_eq!(lifecycle.shutdowns.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn observer_descriptor_must_match_its_registration() {
    struct ObserverExtension;

    impl Extension for ObserverExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            ExtensionDescriptor::trusted_in_process(component("test.extension.observer"), VERSION)
        }

        fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
            let observer: Arc<dyn Observer> =
                Arc::new(finstack_ai_runtime::NoopObserver::new(ObserverDescriptor {
                    component: ComponentRef::new(component("test.observer.wrong"), Some(VERSION)),
                    payload_mode: ObserverPayloadMode::MetadataOnly,
                    metadata: Metadata::empty(),
                }));
            registrar.observer(
                RegistrationMetadata::new(component("test.observer.noop"), VERSION),
                ReadyComponent::new(observer),
            )
        }
    }

    let (model_extension, store_extension) = extensions();
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&model_extension)
        .expect("model registers");
    registrar
        .register_extension(&store_extension)
        .expect("store registers");
    registrar
        .register_extension(&ObserverExtension)
        .expect("observer registers");
    let mut selection =
        AgentComponentSelection::new(exact("test.model.scripted"), exact("test.store.memory"));
    selection.observers = Arc::from([exact("test.observer.noop")]);
    let Err(error) = registrar
        .into_registry()
        .resolve(
            ResolveRequest::new(component("test.agent.source"), selection),
            AgentConstructionContext::new(),
        )
        .await
    else {
        panic!("descriptor mismatch must fail");
    };
    assert_eq!(error.code(), AGENT_BUILD_INVALID_DESCRIPTOR);
}
