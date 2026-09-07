//! Direct and linked construction must preserve descriptor identity and exact locks.
use super::*;
use finstack_ai_runtime::ports::context::{ContextProvider, ContextProviderDescriptor};
use finstack_ai_test::{ScriptedContextProvider, ScriptedMiddleware};

fn builder() -> NativeAgentBuilder {
    Agent::builder(
        AgentId::parse("test.agent.composition").expect("agent"),
        BundleId::parse("test.bundle.composition").expect("bundle"),
        (
            component("test.model.composition", VERSION),
            Arc::new(ScriptedModel::from_plans(profile(), vec![])),
        ),
        (component("test.store.composition", VERSION), store()),
    )
}

fn component(id: &str, version: Version) -> ComponentRef {
    ComponentRef::new(ComponentId::parse(id).expect("id"), Some(version))
}

fn store() -> Arc<dyn JournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4096,
        })
        .expect("store"),
    )
}

fn context(version: Version) -> Arc<dyn ContextProvider> {
    Arc::new(ScriptedContextProvider::new(
        ContextProviderDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("test.context.declared").expect("id"),
                version,
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        },
        vec![],
    ))
}

fn middleware() -> Arc<dyn Middleware> {
    Arc::new(ScriptedMiddleware::new(
        MiddlewareDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("test.middleware.declared").expect("id"),
                version: Version {
                    major: 7,
                    minor: 2,
                    patch: 3,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            stages: StageMask::from_stages([Stage::BeforeModel]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        },
        vec![],
    ))
}

#[tokio::test]
async fn descriptor_versions_and_explicit_store_produce_equivalent_locks() {
    let context = context(Version {
        major: 3,
        minor: 4,
        patch: 5,
    });
    let middleware = middleware();
    let observer: Arc<dyn Observer> = Arc::new(NoopObserver::new(ObserverDescriptor {
        component: component(
            "test.observer.declared",
            Version {
                major: 8,
                minor: 0,
                patch: 1,
            },
        ),
        ..observer_descriptor("test.observer.declared")
    }));
    let direct = builder()
        .context_provider(Arc::clone(&context))
        .middleware(Arc::clone(&middleware))
        .observer(Arc::clone(&observer))
        .build()
        .await
        .expect("direct");
    let selected = store();
    let linked = builder()
        .build_linked(
            LinkedCommon {
                journal_store: Some((
                    component("test.store.composition", VERSION),
                    Arc::clone(&selected),
                )),
                ports: LinkedAgentPorts {
                    context_providers: vec![context],
                    middleware: vec![middleware],
                    observers: vec![observer],
                    ..LinkedAgentPorts::default()
                },
                ..LinkedCommon::default()
            },
            ModelName::try_new("fixture").expect("model"),
            finstack_ai_runtime::ports::model::ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            Duration::from_secs(30),
        )
        .await
        .expect("linked");
    assert_eq!(direct.resolved().lock(), linked.agent.resolved().lock());
    assert!(Arc::ptr_eq(&linked.agent.journal_store(), &selected));
    let lock = direct.resolved().lock().expect("lock");
    for expected in [
        component(
            "test.context.declared",
            Version {
                major: 3,
                minor: 4,
                patch: 5,
            },
        ),
        component(
            "test.middleware.declared",
            Version {
                major: 7,
                minor: 2,
                patch: 3,
            },
        ),
        component(
            "test.observer.declared",
            Version {
                major: 8,
                minor: 0,
                patch: 1,
            },
        ),
    ] {
        assert!(
            lock.components
                .iter()
                .any(|item| item.component == expected)
        );
    }
}

#[tokio::test]
async fn duplicate_descriptor_versions_identify_both_conflicts() {
    let error = builder()
        .context_provider(context(VERSION))
        .context_provider(context(Version {
            major: 9,
            minor: 8,
            patch: 7,
        }))
        .build()
        .await
        .err()
        .expect("duplicate");
    let message = error.to_string();
    assert!(message.contains("test.context.declared"), "{message}");
    assert!(message.contains("0.0.1"), "{message}");
    assert!(message.contains("9.8.7"), "{message}");
}

#[tokio::test]
async fn capability_registration_keeps_descriptor_identity() {
    let context = context(Version {
        major: 3,
        minor: 4,
        patch: 5,
    });
    let middleware = middleware();
    let builder = builder()
        .capability_context_provider(Arc::clone(&context))
        .capability_middleware(Arc::clone(&middleware));
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&builder)
        .expect("register exact descriptors");
    // Base composition excludes capability-only handles until activated.
    let agent = builder.build().await.expect("base");
    assert!(agent.resolved().run_plan().context_providers().is_empty());
    assert!(agent.resolved().run_plan().middleware().is_empty());
}
