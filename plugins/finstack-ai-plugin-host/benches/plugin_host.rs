//! Warning-only plugin-host compile-cache and call benches.

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use finstack_ai::ComponentConstructionContext;
use finstack_ai_plugin_host::{
    CacheKeyParts, InstancePolicy, PluginHost, PluginHostConfig, WasmToolsetAdapter, abi_identity,
    cache_key, component_digest, engine_fingerprint, host_target,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, ComponentId, ComponentRef, Digest, EffectId,
    EffectOutputContract, EffectOutputKind, LaneId, Metadata, OperationLocator, PrincipalRef,
    RawJson, RetrySafety, RunCallContext, RunId, SessionId, ToolCallBlock, ToolCallContext,
    ToolExecutionMode, ToolFailurePolicy, ToolId, Toolset, ValidatedToolCall, Version,
};
use finstack_ai_wit::{NoopPluginHooks, parse_manifest};
use futures_util::StreamExt;

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
    let owned: Vec<String> = worlds.iter().map(|world| (*world).to_owned()).collect();
    let digest = finstack_ai_wit::manifest_digest_hex(identity, "0.0.4", &owned).expect("digest");
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

fn host() -> Arc<PluginHost> {
    Arc::new(
        PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
        )
        .expect("host"),
    )
}

fn construction(identity: &str) -> ComponentConstructionContext {
    ComponentConstructionContext {
        component: ComponentRef::new(
            ComponentId::parse(identity).expect("id"),
            Some(EXPERIMENTAL),
        ),
        configuration: None,
        cancellation: CancellationSignal::new(),
        deadline: None,
        metadata: Metadata::empty(),
    }
}

fn tool_ctx() -> ToolCallContext {
    ToolCallContext {
        run: RunCallContext {
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
        },
        tool_batch_id: finstack_ai_runtime::ToolBatchId::from_bytes([7; 16]),
        tool_call_id: finstack_ai_runtime::ToolCallId::from_bytes([6; 16]),
    }
}

fn validated_call(tool_id: &str, tool_name: &str, arguments: &[u8]) -> ValidatedToolCall {
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            finstack_ai_runtime::ToolCallId::from_bytes([6; 16]),
            tool_name,
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

fn compile_cache(criterion: &mut Criterion) {
    let bytes = fixture_wasm("echo-toolset");
    criterion.bench_function("plugin_host_echo_compile_cache_miss_then_hit", |bencher| {
        bencher.iter(|| {
            let host = PluginHost::try_new(
                PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
            )
            .expect("host");
            let parts = CacheKeyParts {
                digest: component_digest(&bytes),
                engine: engine_fingerprint(),
                target: host_target(),
                abi: abi_identity("toolset-plugin", "0.0.4"),
            };
            let miss = host.compile_with_key(&bytes, &parts).expect("miss");
            let hit = host.compile_with_key(&bytes, &parts).expect("hit");
            black_box((miss, hit, cache_key(&parts)))
        });
    });
}

fn echo_and_calculator_calls(criterion: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let echo = runtime.block_on(async {
        WasmToolsetAdapter::try_new(
            host(),
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
        .expect("echo")
    });
    let calculator = runtime.block_on(async {
        WasmToolsetAdapter::try_new(
            host(),
            &published_wasm("calculator"),
            parse_manifest(&manifest_bytes(
                "finstack.plugin.calculator",
                &["toolset-plugin"],
            ))
            .expect("manifest"),
            &construction("finstack.plugin.calculator"),
            Arc::new(NoopPluginHooks),
        )
        .await
        .expect("calculator")
    });

    criterion.bench_function("plugin_host_echo_call", |bencher| {
        bencher.iter(|| {
            runtime.block_on(async {
                let mut stream = echo
                    .call(
                        tool_ctx(),
                        validated_call("finstack.plugin.echo", "echo", br#"{"text":"bench"}"#),
                    )
                    .await
                    .expect("call");
                black_box(stream.next().await)
            })
        });
    });
    criterion.bench_function("plugin_host_calculator_add_1_2_3", |bencher| {
        bencher.iter(|| {
            runtime.block_on(async {
                let mut stream = calculator
                    .call(
                        tool_ctx(),
                        validated_call(
                            "finstack.plugin.calculator",
                            "calculator",
                            br#"{"operation":"add","operands":[1,2,3]}"#,
                        ),
                    )
                    .await
                    .expect("call");
                black_box(stream.next().await)
            })
        });
    });
}

criterion_group!(benches, compile_cache, echo_and_calculator_calls);
criterion_main!(benches);
