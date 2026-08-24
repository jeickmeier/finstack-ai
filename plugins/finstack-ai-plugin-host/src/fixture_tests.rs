//! Resolve, collect, call, trap, and instance-policy proofs against fixture guests.

use std::sync::Arc;

use finstack_ai::registry::ReadyComponent;
use finstack_ai::{
    AgentComponentSelection, AgentConstructionContext, ComponentSelector, Extension, Registrar,
    RegistrationMetadata, ResolveRequest,
};
use finstack_ai_kernel::{
    ComponentId, ComponentRef, ContentBlock, Digest, EffectId, EffectOutputContract,
    EffectOutputKind, LaneId, Metadata, OperationLocator, PrincipalRef, RawJson, RetrySafety,
    RunId, SessionId, TextBlock, ToolCallBlock, ToolCallId, ToolExecutionMode, ToolFailurePolicy,
    ToolId, ValidatedToolCall, Version,
};
use finstack_ai_runtime::ports::context::{
    ContextAuthority, ContextBudget, ContextCallContext, ContextItemKind, ContextOverflowPolicy,
    ContextRequest,
};
use finstack_ai_runtime::ports::model::{
    AuthorizationContext, CancellationSignal, Model, ModelContextProfile, ModelName,
    RunCallContext, TokenEstimatorRef, TokenEstimatorSource,
};
use finstack_ai_runtime::ports::tool::{ToolCallContext, ToolStreamItem, Toolset};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::ScriptedModel;
use finstack_ai_wit::{NoopPluginHooks, manifest::manifest_digest_hex, parse_manifest};
use futures_util::StreamExt;

use crate::adapters::{WasmPluginExtension, WasmToolsetAdapter};
use crate::host::{InstancePolicy, PluginHost, PluginHostConfig, PluginWorld};
use crate::limits::EffectiveLimits;
use crate::signature::SignaturePolicy;

const MODEL_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};
const EXPERIMENTAL: Version = Version {
    major: 0,
    minor: 0,
    patch: 4,
};

fn fixture_wasm(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/fixtures/guests/{name}/component.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn published_wasm(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../reference/{name}/component.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn manifest_bytes(identity: &str, worlds: &[&str]) -> Vec<u8> {
    manifest_bytes_for(identity, "0.0.4", worlds)
}

fn manifest_bytes_for(identity: &str, version: &str, worlds: &[&str]) -> Vec<u8> {
    let owned: Vec<String> = worlds.iter().map(|world| (*world).to_owned()).collect();
    let digest =
        manifest_digest_hex(identity, version, &owned, &["logging".to_owned()]).expect("digest");
    serde_json::to_vec(&serde_json::json!({
        "identity": identity,
        "version": version,
        "worlds": worlds,
        "permissions": ["logging"],
        "configuration_schema": {},
        "digest": digest
    }))
    .expect("json")
}

fn component(value: &str) -> ComponentId {
    ComponentId::parse(value).expect("component")
}

fn host(policy: InstancePolicy, max: u32) -> Arc<PluginHost> {
    Arc::new(
        PluginHost::try_new(
            PluginHostConfig::try_new(None, policy, max)
                .expect("cfg")
                .with_signature_policy(SignaturePolicy::Permissive)
                .with_default_limits(EffectiveLimits::for_tests()),
        )
        .expect("host"),
    )
}

fn construction(identity: &str) -> finstack_ai::registry::ComponentConstructionContext {
    finstack_ai::registry::ComponentConstructionContext {
        component: ComponentRef::new(component(identity), Some(EXPERIMENTAL)),
        configuration: None,
        cancellation: CancellationSignal::new(),
        deadline: None,
        metadata: Metadata::empty(),
    }
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

    fn register(&self, registrar: &mut Registrar) -> Result<(), finstack_ai::RegistrationError> {
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

    fn register(&self, registrar: &mut Registrar) -> Result<(), finstack_ai::RegistrationError> {
        let handle: Arc<dyn finstack_ai_runtime::ports::journal::JournalStore> = self.store.clone();
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
        relation_depth: 0,
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

fn validated_call(tool_id: &str, arguments: &[u8]) -> ValidatedToolCall {
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::from_bytes([6; 16]),
            "tool",
            RawJson::parse(arguments).expect("arguments"),
        )
        .expect("call"),
        tool_id: ToolId::parse(tool_id).expect("id"),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: RetrySafety::SafeToRetry,
        deadline: None,
        execution: ToolExecutionMode::Sequential,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

fn tool_ctx() -> ToolCallContext {
    ToolCallContext {
        run: run_context(),
        tool_batch_id: finstack_ai_kernel::ToolBatchId::from_bytes([7; 16]),
        tool_call_id: ToolCallId::from_bytes([6; 16]),
    }
}

async fn register_with_plugin(extension: &dyn Extension) -> finstack_ai::ResolvedAgent {
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
    registrar.register_extension(extension).expect("plugin");
    let mut selection = AgentComponentSelection::new(
        ComponentSelector::Component(ComponentRef::new(
            component("test.model.scripted"),
            Some(MODEL_VERSION),
        )),
        ComponentSelector::Component(ComponentRef::new(
            component("test.store.memory"),
            Some(MODEL_VERSION),
        )),
    );
    if extension.descriptor().id.as_str().contains("toolset") {
        selection.toolsets = Arc::from([ComponentSelector::Component(ComponentRef::new(
            extension.descriptor().id.clone(),
            Some(EXPERIMENTAL),
        ))]);
    } else {
        selection.context_providers = Arc::from([ComponentSelector::Component(ComponentRef::new(
            extension.descriptor().id.clone(),
            Some(EXPERIMENTAL),
        ))]);
    }
    registrar
        .into_registry()
        .resolve(
            ResolveRequest::new(component("test.agent.source"), selection),
            AgentConstructionContext::new(),
        )
        .await
        .expect("resolve")
}

#[test]
fn echo_component_imports_only_host_interfaces() {
    let host = host(InstancePolicy::Exclusive, 2);
    let component =
        wasmtime::component::Component::from_binary(host.engine(), &fixture_wasm("echo-toolset"))
            .expect("component");
    let imports: Vec<String> = component
        .component_type()
        .imports(host.engine())
        .map(|(name, _)| name.to_owned())
        .collect();
    assert!(
        imports
            .iter()
            .all(|name| name.starts_with("finstack:ai-host/")
                || name.starts_with("finstack:ai-types/")),
        "unexpected imports: {imports:?}"
    );
    assert!(
        !imports.iter().any(|name| name.starts_with("wasi:")),
        "echo guest must not import wasi: {imports:?}"
    );
}

#[tokio::test]
async fn echo_toolset_resolves_and_calls() {
    let extension = WasmPluginExtension::toolset(
        host(InstancePolicy::Exclusive, 2),
        &fixture_wasm("echo-toolset"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.echo.toolset",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
    )
    .await
    .expect("ext");
    let resolved = register_with_plugin(&extension).await;
    let plan = resolved.run_plan();
    let toolset = Arc::clone(plan.toolsets()[0].handle());
    assert_eq!(toolset.tools().len(), 2);
    let mut stream = toolset
        .call(
            tool_ctx(),
            validated_call("finstack.plugin.add", br#"{"a":2,"b":3}"#),
        )
        .await
        .expect("call");
    let ToolStreamItem::Completed(result) = stream.next().await.expect("item").expect("ok") else {
        panic!("expected completion");
    };
    assert_eq!(result.output.as_bytes(), br#"{"sum":5}"#);
    let descriptor = toolset.descriptor();
    let metadata = descriptor.metadata.as_str();
    assert!(metadata.contains("\"plugin.identity\""));
    assert!(metadata.contains("finstack.plugin.echo.toolset"));
    assert!(metadata.contains("plugin.granted_permissions"));
    assert!(metadata.contains("logging"));
    assert!(!metadata.contains("signature"));
}

#[tokio::test]
async fn echo_toolset_v1_manifest_rejects_v004_component() {
    let Err(error) = WasmPluginExtension::toolset(
        host(InstancePolicy::Exclusive, 2),
        &fixture_wasm("echo-toolset"),
        parse_manifest(&manifest_bytes_for(
            "finstack.plugin.echo.toolset",
            "1.0.0",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
    )
    .await
    else {
        panic!("0.0.4 component must not instantiate against 1.0.0 linker");
    };
    assert_eq!(error.code(), "plugin_instantiate_failed");
}

#[tokio::test]
async fn reference_context_resolves_and_collects() {
    let extension = WasmPluginExtension::context(
        host(InstancePolicy::Exclusive, 2),
        &published_wasm("context-provider"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.reference.context",
            &["context-plugin"],
        ))
        .expect("manifest"),
    )
    .await
    .expect("ext");
    let resolved = register_with_plugin(&extension).await;
    let plan = resolved.run_plan();
    let provider = Arc::clone(plan.context_providers()[0].handle());
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
        item.authority == ContextAuthority::Untrusted
            && matches!(
                item.kind,
                ContextItemKind::QuotedSource | ContextItemKind::Reference
            )
    }));
}

#[tokio::test]
async fn trap_guest_is_contained() {
    let adapter = WasmToolsetAdapter::try_new(
        host(InstancePolicy::Exclusive, 2),
        &fixture_wasm("trap-toolset"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.trap.toolset",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.trap.toolset"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("adapter");
    let Err(error) = adapter
        .call(tool_ctx(), validated_call("finstack.plugin.trap", b"{}"))
        .await
    else {
        panic!("trap");
    };
    assert_eq!(
        error.code(),
        "plugin_trap",
        "{} / {}",
        error.code(),
        error.message()
    );
}

#[tokio::test]
async fn exclusive_overflow_fails_closed() {
    let adapter = WasmToolsetAdapter::try_new(
        host(InstancePolicy::Exclusive, 1),
        &fixture_wasm("echo-toolset"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.echo.toolset",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.echo.toolset"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("adapter");
    let _held = adapter.hold_exclusive_slot().expect("hold");
    let Err(error) = adapter
        .call(
            tool_ctx(),
            validated_call("finstack.plugin.add", br#"{"a":1,"b":1}"#),
        )
        .await
    else {
        panic!("limit");
    };
    assert_eq!(error.code(), "plugin_instance_limit");
}

#[tokio::test]
async fn serialized_overflow_fails_closed() {
    let adapter = WasmToolsetAdapter::try_new(
        host(InstancePolicy::Serialized, 2),
        &fixture_wasm("echo-toolset"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.echo.toolset",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.echo.toolset"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("adapter");
    let _held = adapter.hold_serialized_slot().expect("hold");
    let Err(error) = adapter
        .call(
            tool_ctx(),
            validated_call("finstack.plugin.add", br#"{"a":1,"b":1}"#),
        )
        .await
    else {
        panic!("limit");
    };
    assert_eq!(error.code(), "plugin_instance_limit");
}

#[tokio::test]
async fn exclusive_concurrent_calls_succeed() {
    let adapter = Arc::new(
        WasmToolsetAdapter::try_new(
            host(InstancePolicy::Exclusive, 2),
            &fixture_wasm("echo-toolset"),
            parse_manifest(&manifest_bytes(
                "finstack.plugin.echo.toolset",
                &["toolset-plugin"],
            ))
            .expect("manifest"),
            &construction("finstack.plugin.echo.toolset"),
            Arc::new(NoopPluginHooks),
        )
        .await
        .expect("adapter"),
    );
    let first = Arc::clone(&adapter);
    let second = Arc::clone(&adapter);
    let (a, b) = tokio::join!(
        first.call(
            tool_ctx(),
            validated_call("finstack.plugin.add", br#"{"a":2,"b":3}"#)
        ),
        second.call(
            tool_ctx(),
            validated_call("finstack.plugin.echo", br#"{"text":"ok"}"#)
        )
    );
    assert!(a.is_ok() && b.is_ok());
}

#[tokio::test]
async fn cancelling_one_call_does_not_trap_a_concurrent_call() {
    // Regression: cancellation used to fire `Engine::increment_epoch()`, which
    // is engine-global, while every store is armed `set_epoch_deadline(1)`.
    // Cancelling one call therefore trapped every other guest already running
    // on the same host. Interruption is now per-store cooperative yielding.
    let adapter = Arc::new(
        WasmToolsetAdapter::try_new(
            host(InstancePolicy::Exclusive, 2),
            &fixture_wasm("echo-toolset"),
            parse_manifest(&manifest_bytes(
                "finstack.plugin.echo.toolset",
                &["toolset-plugin"],
            ))
            .expect("manifest"),
            &construction("finstack.plugin.echo.toolset"),
            Arc::new(NoopPluginHooks),
        )
        .await
        .expect("adapter"),
    );

    // Scope note: this covers the pre-cancelled path, which returns before
    // `with_cancellation`'s `select!`. The cross-store trap itself fired from
    // the `select!` arms, and reproducing it deterministically needs a guest
    // that blocks predictably — echo returns in microseconds and fuel-burner
    // in a few milliseconds, so a timing-based test would be flaky. What holds
    // that invariant is structural: no `Engine::increment_epoch` call remains
    // anywhere in the crate, and each store is interrupted through its own
    // `fuel_async_yield_interval`.
    let cancelled_ctx = tool_ctx();
    cancelled_ctx.run.cancellation.cancel();
    let survivor_ctx = tool_ctx();

    let cancelled = Arc::clone(&adapter);
    let survivor = Arc::clone(&adapter);
    let (cancelled_result, survivor_result) = tokio::join!(
        cancelled.call(
            cancelled_ctx,
            validated_call("finstack.plugin.echo", br#"{"text":"nope"}"#)
        ),
        survivor.call(
            survivor_ctx,
            validated_call("finstack.plugin.add", br#"{"a":2,"b":3}"#)
        )
    );

    let Err(cancelled_error) = cancelled_result else {
        panic!("the cancelled call must fail");
    };
    assert_eq!(cancelled_error.code(), "plugin_lifecycle_timeout");
    assert!(
        survivor_result.is_ok(),
        "a concurrent, un-cancelled call must still succeed"
    );
}

#[tokio::test]
async fn fuel_burner_trips_resource_limit_and_echo_still_runs() {
    let shared = host(InstancePolicy::Exclusive, 2);
    let burner = WasmToolsetAdapter::try_new(
        Arc::clone(&shared),
        &fixture_wasm("fuel-burner"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.fuel.toolset",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.fuel.toolset"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("burner");
    let Err(error) = burner
        .call(tool_ctx(), validated_call("finstack.plugin.burn", b"{}"))
        .await
    else {
        panic!("fuel");
    };
    assert_eq!(
        error.code(),
        "plugin_resource_limit",
        "{} / {}",
        error.code(),
        error.message()
    );
    assert!(error.metadata().as_str().contains("plugin.identity"));
    assert!(!error.metadata().as_str().contains("signature"));

    let echo = WasmPluginExtension::toolset(
        shared,
        &fixture_wasm("echo-toolset"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.echo.toolset",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
    )
    .await
    .expect("echo");
    let resolved = register_with_plugin(&echo).await;
    let toolset = Arc::clone(resolved.run_plan().toolsets()[0].handle());
    let mut stream = toolset
        .call(
            tool_ctx(),
            validated_call("finstack.plugin.add", br#"{"a":2,"b":3}"#),
        )
        .await
        .expect("echo after burn");
    let ToolStreamItem::Completed(result) = stream.next().await.expect("item").expect("ok") else {
        panic!("expected completion");
    };
    assert_eq!(result.output.as_bytes(), br#"{"sum":5}"#);
}

#[test]
fn load_rejects_world_mismatch_without_compiling_guest() {
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
        &fixture_wasm("echo-toolset"),
        manifest,
        PluginWorld::Toolset,
    ) else {
        panic!("world mismatch");
    };
    assert_eq!(error.code(), "plugin_registration_invalid");
}
