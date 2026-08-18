//! Calculator, context-provider, filesystem-sandbox, and template-run proofs.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use finstack_ai::ComponentConstructionContext;
use finstack_ai_runtime::{
    AssembledToolStream, AuthorizationContext, CancellationSignal, ComponentId, ComponentRef,
    ContextAuthority, ContextBudget, ContextCallContext, ContextItemKind, ContextOverflowPolicy,
    ContextProvider, ContextRequest, Digest, EffectId, EffectOutputContract, EffectOutputKind,
    LaneId, Metadata, OperationLocator, PrincipalRef, RawJson, RetrySafety, RunCallContext, RunId,
    SessionId, ToolCallBlock, ToolCallContext, ToolDeferralSupport, ToolExecutionMode,
    ToolFailurePolicy, ToolId, ToolResult, ToolStreamAssembler, ToolStreamLimits, ToolTerminal,
    Toolset, ValidatedToolCall, Version,
};
use finstack_ai_test::{
    ContextConformanceCase, ToolsetConformanceCase, check_context_conformance,
    check_toolset_conformance,
};
use finstack_ai_tools_calculator::CalculatorToolset;
use finstack_ai_wit::{
    NoopPluginHooks, ReferenceContextProvider, WitContextAdapter, manifest::manifest_digest_hex,
    parse_manifest,
};
use futures_util::StreamExt;

use crate::adapters::{WasmContextAdapter, WasmToolsetAdapter};
use crate::cache::component_digest;
use crate::grants::FilesystemPreopen;
use crate::host::{InstancePolicy, PluginHost, PluginHostConfig, PluginWorld};
use crate::limits::EffectiveLimits;
use crate::lockfile::LockedPlugin;

const EXPERIMENTAL: Version = Version {
    major: 0,
    minor: 0,
    patch: 4,
};

fn reference_lock() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../reference/plugin.lock.json")
}

fn published_wasm(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../reference/{name}/component.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn manifest_bytes(identity: &str, worlds: &[&str], permissions: &[&str]) -> Vec<u8> {
    let owned: Vec<String> = worlds.iter().map(|world| (*world).to_owned()).collect();
    let digest = manifest_digest_hex(identity, "0.0.4", &owned).expect("digest");
    serde_json::to_vec(&serde_json::json!({
        "identity": identity,
        "version": "0.0.4",
        "worlds": worlds,
        "permissions": permissions,
        "configuration_schema": {},
        "digest": digest
    }))
    .expect("json")
}

fn component(value: &str) -> ComponentId {
    ComponentId::parse(value).expect("component")
}

fn default_host() -> Arc<PluginHost> {
    Arc::new(
        PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2)
                .expect("cfg")
                .with_default_limits(EffectiveLimits::for_tests()),
        )
        .expect("host"),
    )
}

fn sandbox_manifest_bytes() -> Vec<u8> {
    let identity = "finstack.plugin.filesystem.sandbox";
    let worlds = ["toolset-plugin"];
    let owned: Vec<String> = worlds.iter().map(|world| (*world).to_owned()).collect();
    let digest = manifest_digest_hex(identity, "0.0.4", &owned).expect("digest");
    serde_json::to_vec(&serde_json::json!({
        "identity": identity,
        "version": "0.0.4",
        "worlds": worlds,
        "permissions": ["logging", "filesystem"],
        "configuration_schema": {},
        "digest": digest,
        "resource_limits": {
            "max_output_bytes": 65_536,
            "call_timeout_ms": 60_000,
            "max_tables": 16,
            "max_instances": 16
        }
    }))
    .expect("json")
}

fn sandbox_host(preopens: Vec<FilesystemPreopen>) -> Arc<PluginHost> {
    let grants = ["logging", "blobs", "filesystem"]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    Arc::new(
        PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2)
                .expect("cfg")
                .with_application_grants(grants)
                .expect("grants")
                .with_filesystem_preopens(preopens)
                .with_default_limits(EffectiveLimits::for_tests()),
        )
        .expect("host"),
    )
}

fn filesystem_offered_host() -> Arc<PluginHost> {
    let grants = ["logging", "blobs", "filesystem"]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    Arc::new(
        PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2)
                .expect("cfg")
                .with_application_grants(grants)
                .expect("grants")
                .with_default_limits(EffectiveLimits::for_tests()),
        )
        .expect("host"),
    )
}

fn construction(identity: &str) -> ComponentConstructionContext {
    ComponentConstructionContext {
        component: ComponentRef::new(component(identity), Some(EXPERIMENTAL)),
        configuration: None,
        cancellation: CancellationSignal::new(),
        deadline: None,
        metadata: Metadata::empty(),
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

fn tool_ctx() -> ToolCallContext {
    ToolCallContext {
        run: run_context(),
        tool_batch_id: finstack_ai_runtime::ToolBatchId::from_bytes([7; 16]),
        tool_call_id: finstack_ai_runtime::ToolCallId::from_bytes([6; 16]),
    }
}

fn validated_call(
    tool_id: &str,
    tool_name: &str,
    arguments: &[u8],
    execution: ToolExecutionMode,
) -> ValidatedToolCall {
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
        execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

fn context_request() -> ContextRequest {
    ContextRequest {
        session_id: SessionId::from_bytes([1; 16]),
        lane_id: LaneId::from_bytes([2; 16]),
        run_id: RunId::from_bytes([3; 16]),
        user_input: Arc::from([finstack_ai_runtime::ContentBlock::Text(
            finstack_ai_runtime::TextBlock::try_new("hello").expect("text"),
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

fn context_call() -> ContextCallContext {
    ContextCallContext {
        run: run_context(),
        provider_index: 0,
        chain_digest: Digest::raw_json(b"{}"),
    }
}

async fn assemble(
    toolset: &dyn Toolset,
    call: ValidatedToolCall,
) -> Result<AssembledToolStream, finstack_ai_runtime::ToolError> {
    let stream = toolset.call(tool_ctx(), call).await?;
    ToolStreamAssembler::new(ToolStreamLimits::default())
        .assemble(stream, None, 1_024, ToolDeferralSupport::Never)
        .await
}

fn completed_result(assembled: &AssembledToolStream) -> &ToolResult {
    match &assembled.terminal {
        ToolTerminal::Completed(result) => result,
        ToolTerminal::Deferred(_) => panic!("completed-only fixture deferred"),
    }
}

#[test]
fn on_disk_reference_manifests_use_host_digests() {
    for (name, identity, worlds) in [
        (
            "calculator",
            "finstack.plugin.calculator",
            vec!["toolset-plugin".to_owned()],
        ),
        (
            "context-provider",
            "finstack.plugin.reference.context",
            vec!["context-plugin".to_owned()],
        ),
        (
            "filesystem-sandbox",
            "finstack.plugin.filesystem.sandbox",
            vec!["toolset-plugin".to_owned()],
        ),
    ] {
        let path = format!(
            "{}/../reference/{name}/plugin.manifest.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let manifest = parse_manifest(&std::fs::read(&path).expect("read")).expect("parse");
        let expected = manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
        assert_eq!(manifest.digest, expected, "{name}");
        assert_eq!(manifest.identity.as_str(), identity);
    }
}

async fn calculator_adapter() -> WasmToolsetAdapter {
    WasmToolsetAdapter::try_new(
        default_host(),
        &published_wasm("calculator"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.calculator",
            &["toolset-plugin"],
            &["logging"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.calculator"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("calculator")
}

#[tokio::test]
async fn load_enabled_reference_lock_passes_published_conformance() {
    let host = default_host();
    let loaded = host.load_enabled(&reference_lock()).expect("load_enabled");
    let identities: Vec<&str> = loaded
        .iter()
        .map(|ready| ready.manifest().identity.as_str())
        .collect();
    assert_eq!(
        identities,
        [
            "finstack.plugin.calculator",
            "finstack.plugin.reference.context"
        ]
    );
    assert_eq!(loaded[0].world(), PluginWorld::Toolset);
    assert_eq!(loaded[1].world(), PluginWorld::Context);

    let native = CalculatorToolset::try_new().expect("native");
    let wasm = calculator_adapter().await;
    let add_args = br#"{"operation":"add","operands":[1,2,3]}"#;
    let native_add = assemble(
        &native,
        validated_call(
            "finstack.tools.calculator",
            "calculator",
            add_args,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect("native add");
    check_toolset_conformance(
        &wasm,
        ToolsetConformanceCase {
            context: tool_ctx(),
            call: validated_call(
                "finstack.plugin.calculator",
                "calculator",
                add_args,
                ToolExecutionMode::Parallel,
            ),
            expected: AssembledToolStream {
                progress: Arc::from([]),
                usage: None,
                terminal: ToolTerminal::Completed(ToolResult {
                    output: completed_result(&native_add).output.clone(),
                    is_error: false,
                }),
            },
            stream_limits: ToolStreamLimits::default(),
            max_result_bytes: 1_024,
        },
    )
    .await
    .expect("wasm add conformance");
}

#[tokio::test]
async fn calculator_matches_native_evaluate_and_conformance() {
    let native = CalculatorToolset::try_new().expect("native");
    let wasm = calculator_adapter().await;
    let add_args = br#"{"operation":"add","operands":[1,2,3]}"#;
    let native_add = assemble(
        &native,
        validated_call(
            "finstack.tools.calculator",
            "calculator",
            add_args,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect("native add");
    assert_eq!(
        completed_result(&native_add).output.as_bytes(),
        br#"{"result":6}"#
    );
    check_toolset_conformance(
        &wasm,
        ToolsetConformanceCase {
            context: tool_ctx(),
            call: validated_call(
                "finstack.plugin.calculator",
                "calculator",
                add_args,
                ToolExecutionMode::Parallel,
            ),
            expected: AssembledToolStream {
                progress: Arc::from([]),
                usage: None,
                terminal: ToolTerminal::Completed(ToolResult {
                    output: completed_result(&native_add).output.clone(),
                    is_error: false,
                }),
            },
            stream_limits: ToolStreamLimits::default(),
            max_result_bytes: 1_024,
        },
    )
    .await
    .expect("wasm add conformance");
}

#[tokio::test]
async fn calculator_multiply_divide_and_overflow_match_native() {
    let native = CalculatorToolset::try_new().expect("native");
    let wasm = calculator_adapter().await;
    let multiply_args = br#"{"operation":"multiply","operands":[]}"#;
    let native_mul = assemble(
        &native,
        validated_call(
            "finstack.tools.calculator",
            "calculator",
            multiply_args,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect("native multiply");
    let wasm_mul = assemble(
        &wasm,
        validated_call(
            "finstack.plugin.calculator",
            "calculator",
            multiply_args,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect("wasm multiply");
    assert_eq!(
        completed_result(&native_mul).output,
        completed_result(&wasm_mul).output
    );
    assert_eq!(
        completed_result(&wasm_mul).output.as_bytes(),
        br#"{"result":1}"#
    );

    let divide_zero = assemble(
        &wasm,
        validated_call(
            "finstack.plugin.calculator",
            "calculator",
            br#"{"operation":"divide","operands":[1,0]}"#,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect_err("divide");
    let native_zero = assemble(
        &native,
        validated_call(
            "finstack.tools.calculator",
            "calculator",
            br#"{"operation":"divide","operands":[1,0]}"#,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect_err("native divide");
    assert_eq!(divide_zero.code(), "calculator_arithmetic_error");
    assert_eq!(native_zero.code(), divide_zero.code());

    let overflow = vec![1.0; 1_025];
    let overflow_args = serde_json::to_vec(&serde_json::json!({
        "operation": "add",
        "operands": overflow,
    }))
    .expect("overflow json");
    let wasm_overflow = assemble(
        &wasm,
        validated_call(
            "finstack.plugin.calculator",
            "calculator",
            &overflow_args,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect_err("overflow");
    let native_overflow = assemble(
        &native,
        validated_call(
            "finstack.tools.calculator",
            "calculator",
            &overflow_args,
            ToolExecutionMode::Parallel,
        ),
    )
    .await
    .expect_err("native overflow");
    assert_eq!(wasm_overflow.code(), "calculator_invalid_arguments");
    assert_eq!(native_overflow.code(), wasm_overflow.code());
}

#[tokio::test]
async fn context_provider_matches_in_process_reference() {
    let manifest = parse_manifest(&manifest_bytes(
        "finstack.plugin.reference.context",
        &["context-plugin"],
        &["logging"],
    ))
    .expect("manifest");
    let construction = construction("finstack.plugin.reference.context");
    let in_process = WitContextAdapter::try_new(
        ReferenceContextProvider::new(),
        &manifest,
        &construction,
        Arc::new(NoopPluginHooks),
    )
    .expect("in-process");
    let expected = in_process
        .collect(context_call(), context_request())
        .await
        .expect("in-process collect");
    assert_eq!(expected.items.len(), 2);
    assert!(expected.items.iter().all(|item| {
        item.authority == ContextAuthority::Untrusted
            && matches!(
                item.kind,
                ContextItemKind::QuotedSource | ContextItemKind::Reference
            )
    }));

    let wasm = WasmContextAdapter::try_new(
        default_host(),
        &published_wasm("context-provider"),
        manifest,
        &construction,
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("wasm context");
    check_context_conformance(
        &wasm,
        ContextConformanceCase {
            context: context_call(),
            request: context_request(),
            expected,
        },
    )
    .await
    .expect("context conformance");
}

#[test]
fn programmatic_enabled_sandbox_loads_with_preopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), b"hello-sandbox").expect("write");
    let host = sandbox_host(vec![FilesystemPreopen {
        guest_path: "/".into(),
        host_path: dir.path().to_path_buf(),
        read: true,
        write: false,
    }]);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../reference/filesystem-sandbox");
    let bytes = std::fs::read(root.join("component.wasm")).expect("wasm");
    let manifest_bytes = std::fs::read(root.join("plugin.manifest.json")).expect("manifest");
    let manifest = parse_manifest(&manifest_bytes).expect("parse");
    let ready = host
        .load_locked(&LockedPlugin {
            identity: "finstack.plugin.filesystem.sandbox".into(),
            version: "0.0.4".into(),
            enabled: true,
            component: "component.wasm".into(),
            component_path: root.join("component.wasm"),
            component_digest: component_digest(&bytes),
            manifest: "plugin.manifest.json".into(),
            manifest_path: root.join("plugin.manifest.json"),
            manifest_digest: manifest.digest,
        })
        .expect("load_locked");
    assert_eq!(ready.world(), PluginWorld::Toolset);
    assert_eq!(
        ready.manifest().identity.as_str(),
        "finstack.plugin.filesystem.sandbox"
    );
}

#[tokio::test]
async fn filesystem_sandbox_lists_and_reads_under_preopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), b"hello-sandbox").expect("write");
    let host = sandbox_host(vec![FilesystemPreopen {
        guest_path: "/".into(),
        host_path: dir.path().to_path_buf(),
        read: true,
        write: false,
    }]);
    let adapter = WasmToolsetAdapter::try_new(
        host,
        &published_wasm("filesystem-sandbox"),
        parse_manifest(&sandbox_manifest_bytes()).expect("manifest"),
        &construction("finstack.plugin.filesystem.sandbox"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("sandbox");

    let listed = assemble(
        &adapter,
        validated_call(
            "finstack.plugin.filesystem.list",
            "list",
            br#"{"path":"."}"#,
            ToolExecutionMode::Sequential,
        ),
    )
    .await
    .expect("list");
    let value: serde_json::Value =
        serde_json::from_slice(completed_result(&listed).output.as_bytes()).expect("list json");
    let entries = value["entries"].as_array().expect("entries");
    assert!(
        entries.iter().any(|entry| entry.as_str() == Some("a.txt")),
        "list entries: {value}"
    );

    let read = assemble(
        &adapter,
        validated_call(
            "finstack.plugin.filesystem.read",
            "read",
            br#"{"path":"a.txt"}"#,
            ToolExecutionMode::Sequential,
        ),
    )
    .await
    .expect("read");
    let read_value: serde_json::Value =
        serde_json::from_slice(completed_result(&read).output.as_bytes()).expect("read json");
    assert_eq!(read_value["bytes"].as_str(), Some("hello-sandbox"));
}

#[tokio::test]
async fn filesystem_sandbox_without_grant_is_permission_denied() {
    let Err(error) = WasmToolsetAdapter::try_new(
        default_host(),
        &published_wasm("filesystem-sandbox"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.filesystem.sandbox",
            &["toolset-plugin"],
            &["logging", "filesystem"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.filesystem.sandbox"),
        Arc::new(NoopPluginHooks),
    )
    .await
    else {
        panic!("ungranted filesystem must fail");
    };
    assert_eq!(error.code(), "plugin_permission_denied");
}

#[tokio::test]
async fn filesystem_sandbox_without_preopen_fails_instantiate() {
    let Err(error) = WasmToolsetAdapter::try_new(
        filesystem_offered_host(),
        &published_wasm("filesystem-sandbox"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.filesystem.sandbox",
            &["toolset-plugin"],
            &["logging", "filesystem"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.filesystem.sandbox"),
        Arc::new(NoopPluginHooks),
    )
    .await
    else {
        panic!("filesystem name without preopen must stay unlinked");
    };
    assert_eq!(error.code(), "plugin_instantiate_failed");
}

#[tokio::test]
#[ignore = "invoked by tools/plugin_wasm/template_check.py"]
async fn template_project_builds_and_runs() {
    let path = std::env::var("FINSTACK_TEMPLATE_WASM").expect("FINSTACK_TEMPLATE_WASM");
    let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let identity = "finstack.plugin.template.toolset";
    let worlds = ["toolset-plugin"];
    let owned: Vec<String> = worlds.iter().map(|world| (*world).to_owned()).collect();
    let digest = manifest_digest_hex(identity, "0.0.4", &owned).expect("digest");
    let manifest = parse_manifest(
        &serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest,
            "resource_limits": {
                "max_output_bytes": 65_536,
                "call_timeout_ms": 60_000,
                "max_tables": 8,
                "max_instances": 8
            }
        }))
        .expect("json"),
    )
    .expect("manifest");
    let adapter = WasmToolsetAdapter::try_new(
        default_host(),
        &bytes,
        manifest,
        &construction("finstack.plugin.template.toolset"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("template");
    let mut stream = adapter
        .call(
            tool_ctx(),
            validated_call(
                "finstack.plugin.template.echo",
                "echo",
                br#"{"text":"from-template"}"#,
                ToolExecutionMode::Sequential,
            ),
        )
        .await
        .expect("call");
    let finstack_ai_runtime::ToolStreamItem::Completed(result) =
        stream.next().await.expect("item").expect("ok")
    else {
        panic!("expected completion");
    };
    assert_eq!(result.output.as_bytes(), br#"{"text":"from-template"}"#);
}
