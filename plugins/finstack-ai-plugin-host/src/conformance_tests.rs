//! Published G6 hostile and lockfile conformance index.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use finstack_ai::ComponentConstructionContext;
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, ComponentId, ComponentRef, Digest, EffectId,
    EffectOutputContract, EffectOutputKind, LaneId, Metadata, OperationLocator, PrincipalRef,
    RawJson, RetrySafety, RunCallContext, RunId, SessionId, Timestamp, ToolCallBlock,
    ToolCallContext, ToolExecutionMode, ToolFailurePolicy, ToolId, Toolset, ValidatedToolCall,
    Version,
};
use finstack_ai_wit::{
    MAX_RAW_JSON_BYTES, NoopPluginHooks, parse_manifest, reject_before_allocation,
};
use futures_util::StreamExt;

use crate::adapters::WasmToolsetAdapter;
use crate::cache::{
    CONFIG_FINGERPRINT, CacheKeyParts, abi_identity, component_digest, engine_fingerprint,
    engine_fingerprint_parts, host_target,
};
use crate::host::{InstancePolicy, PluginHost, PluginHostConfig, PluginWorld};
use crate::lockfile::resolve_lockfile;
use crate::signature::{SignaturePolicy, verify_manifest};

const EXPERIMENTAL: Version = Version {
    major: 0,
    minor: 0,
    patch: 4,
};

const PLUGIN_CONFORMANCE_SUITE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginConformanceFailure {
    world: &'static str,
    contract: &'static str,
    suite_version: &'static str,
    detail: String,
}

impl PluginConformanceFailure {
    fn new(world: &'static str, contract: &'static str, detail: impl Into<String>) -> Self {
        Self {
            world,
            contract,
            suite_version: PLUGIN_CONFORMANCE_SUITE_VERSION,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for PluginConformanceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "plugin world `{}` contract `{}` suite {} violated: {}",
            self.world, self.contract, self.suite_version, self.detail
        )
    }
}

fn unexpected_success(world: &'static str, contract: &'static str, detail: &'static str) -> ! {
    panic!("{}", PluginConformanceFailure::new(world, contract, detail));
}

fn published_wasm(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../reference/{name}/component.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn fixture_wasm(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/fixtures/guests/{name}/component.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn reference_lock() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../reference/plugin.lock.json")
}

fn default_host() -> Arc<PluginHost> {
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

fn validated_call(tool_id: &str, arguments: &[u8]) -> ValidatedToolCall {
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            finstack_ai_runtime::ToolCallId::from_bytes([6; 16]),
            "echo",
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

fn echo_manifest_bytes() -> Vec<u8> {
    manifest_bytes("finstack.plugin.echo.toolset", &["toolset-plugin"])
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

async fn echo_adapter() -> WasmToolsetAdapter {
    WasmToolsetAdapter::try_new(
        default_host(),
        &fixture_wasm("echo-toolset"),
        parse_manifest(&echo_manifest_bytes()).expect("manifest"),
        &construction("finstack.plugin.echo.toolset"),
        Arc::new(NoopPluginHooks),
    )
    .await
    .expect("echo")
}

#[tokio::test]
async fn lifecycle_fired_deadline_is_timeout() {
    let mut ctx = construction("finstack.plugin.echo.toolset");
    ctx.deadline = Some(Timestamp::from_unix_ms(1).expect("past deadline"));
    let Err(error) = WasmToolsetAdapter::try_new(
        default_host(),
        &fixture_wasm("echo-toolset"),
        parse_manifest(&echo_manifest_bytes()).expect("manifest"),
        &ctx,
        Arc::new(NoopPluginHooks),
    )
    .await
    else {
        unexpected_success(
            "toolset-plugin",
            "plugin.lifecycle.deadline",
            "fired deadline constructed an adapter",
        );
    };
    assert_eq!(error.code(), "plugin_lifecycle_timeout");
}

#[test]
fn malformed_manifest_is_rejected() {
    let omitted = parse_manifest(br#"{"identity":"finstack.plugin.echo.toolset"}"#)
        .expect_err("omitted digest");
    assert_eq!(omitted.code(), "plugin_registration_invalid");
    let unknown = parse_manifest(br#"{"identity":"finstack.plugin.echo.toolset","version":"0.0.4","worlds":["toolset-plugin"],"permissions":["logging"],"configuration_schema":{},"digest":"00","extra":true}"#)
        .expect_err("unknown");
    assert_eq!(unknown.code(), "plugin_registration_invalid");
    let identity = "finstack.plugin.echo.toolset";
    let worlds = vec!["toolset-plugin".to_owned()];
    let mismatch = parse_manifest(
        &serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": "00".repeat(32)
        }))
        .expect("json"),
    )
    .expect_err("digest");
    assert_eq!(mismatch.code(), "plugin_manifest_digest_mismatch");
}

#[test]
fn permission_denial_without_host_grant() {
    let host = PluginHost::try_new(
        PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
    )
    .expect("host");
    let identity = "finstack.plugin.filesystem.sandbox";
    let worlds = vec!["toolset-plugin".to_owned()];
    let digest = finstack_ai_wit::manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
    let manifest = parse_manifest(
        &serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging", "filesystem"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json"),
    )
    .expect("manifest");
    let Err(error) = host.load(b"(component)", manifest, PluginWorld::Toolset) else {
        unexpected_success(
            "toolset-plugin",
            "plugin.permission.granted",
            "filesystem permission loaded without a host grant",
        );
    };
    assert_eq!(error.code(), "plugin_permission_denied");
}

#[tokio::test]
async fn ungranted_wasi_instantiate_fails_closed() {
    let grants = ["logging", "blobs", "filesystem"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let host = Arc::new(
        PluginHost::try_new(
            PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2)
                .expect("cfg")
                .with_application_grants(grants)
                .expect("grants"),
        )
        .expect("host"),
    );
    let Err(error) = WasmToolsetAdapter::try_new(
        host,
        &published_wasm("filesystem-sandbox"),
        parse_manifest(&manifest_bytes(
            "finstack.plugin.filesystem.sandbox",
            &["toolset-plugin"],
        ))
        .expect("manifest"),
        &construction("finstack.plugin.filesystem.sandbox"),
        Arc::new(NoopPluginHooks),
    )
    .await
    else {
        unexpected_success(
            "toolset-plugin",
            "plugin.wasi.ungranted",
            "ungranted WASI filesystem instantiated",
        );
    };
    assert_eq!(error.code(), "plugin_instantiate_failed");
}

#[tokio::test]
async fn trap_guest_is_contained() {
    let adapter = WasmToolsetAdapter::try_new(
        default_host(),
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
        unexpected_success(
            "toolset-plugin",
            "plugin.call.trap",
            "trap guest returned success",
        );
    };
    assert_eq!(error.code(), "plugin_trap");
}

#[tokio::test]
async fn fuel_burner_trips_resource_limit() {
    let adapter = WasmToolsetAdapter::try_new(
        default_host(),
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
    let Err(error) = adapter
        .call(tool_ctx(), validated_call("finstack.plugin.burn", b"{}"))
        .await
    else {
        unexpected_success(
            "toolset-plugin",
            "plugin.resource.fuel",
            "fuel burner returned success",
        );
    };
    assert_eq!(error.code(), "plugin_resource_limit");
}

#[test]
fn signature_unsigned_and_untrusted_fail_strict() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/manifests");
    let unsigned = parse_manifest(&std::fs::read(root.join("unsigned.json")).expect("unsigned"))
        .expect("parse");
    assert_eq!(
        verify_manifest(
            &unsigned,
            SignaturePolicy::Strict,
            &std::collections::BTreeMap::new()
        )
        .expect_err("unsigned")
        .code(),
        "plugin_signature_untrusted"
    );
    let untrusted =
        parse_manifest(&std::fs::read(root.join("signed-untrusted.json")).expect("untrusted"))
            .expect("parse");
    let public = hex_to_32(&std::fs::read_to_string(root.join("test-root.pub.hex")).expect("root"));
    let mut roots = std::collections::BTreeMap::new();
    roots.insert("test-root".into(), public);
    assert_eq!(
        verify_manifest(&untrusted, SignaturePolicy::Strict, &roots)
            .expect_err("untrusted")
            .code(),
        "plugin_signature_untrusted"
    );
}

#[tokio::test]
async fn cancellation_during_echo_call_is_timeout() {
    let adapter = echo_adapter().await;
    let ctx = tool_ctx();
    ctx.run.cancellation.cancel();
    let Err(error) = adapter
        .call(
            ctx,
            validated_call("finstack.plugin.echo", br#"{"text":"nope"}"#),
        )
        .await
    else {
        unexpected_success(
            "toolset-plugin",
            "plugin.call.cancelled",
            "cancelled echo call returned success",
        );
    };
    assert_eq!(error.code(), "plugin_lifecycle_timeout");
}

#[tokio::test]
async fn large_payload_is_rejected_before_guest_allocation() {
    let oversize = vec![b'x'; MAX_RAW_JSON_BYTES + 1];
    let mapped =
        reject_before_allocation(&oversize, MAX_RAW_JSON_BYTES, "args-json").expect_err("oversize");
    assert_eq!(mapped.code(), "plugin_payload_too_large");
    assert!(RawJson::parse(&oversize).is_err());
    let adapter = echo_adapter().await;
    let mut stream = adapter
        .call(
            tool_ctx(),
            validated_call("finstack.plugin.echo", br#"{"text":"ok"}"#),
        )
        .await
        .expect("live adapter");
    let item = stream.next().await.expect("item").expect("ok");
    assert!(matches!(
        item,
        finstack_ai_runtime::ToolStreamItem::Completed(_)
    ));
}

#[test]
fn abi_mismatch_calculator_as_context_fails() {
    let host = PluginHost::try_new(
        PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
    )
    .expect("host");
    let identity = "finstack.plugin.calculator";
    let worlds = vec!["toolset-plugin".to_owned()];
    let digest = finstack_ai_wit::manifest_digest_hex(identity, "0.0.4", &worlds).expect("digest");
    let manifest = parse_manifest(
        &serde_json::to_vec(&serde_json::json!({
            "identity": identity,
            "version": "0.0.4",
            "worlds": worlds,
            "permissions": ["logging"],
            "configuration_schema": {},
            "digest": digest
        }))
        .expect("json"),
    )
    .expect("manifest");
    let Err(error) = host.load(
        &published_wasm("calculator"),
        manifest,
        PluginWorld::Context,
    ) else {
        unexpected_success(
            "context-plugin",
            "plugin.world.abi",
            "toolset guest loaded as a context world",
        );
    };
    assert_eq!(error.code(), "plugin_registration_invalid");
}

#[test]
fn lockfile_duplicate_identity_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let hex = "ab".repeat(32);
    let path = dir.path().join("plugin.lock.json");
    std::fs::write(
        &path,
        format!(
            r#"{{
  "lockfile_version": 1,
  "plugins": [
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": true,
      "component": "a/component.wasm",
      "component_digest": "{hex}",
      "manifest": "a/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }},
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": false,
      "component": "b/component.wasm",
      "component_digest": "{hex}",
      "manifest": "b/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }}
  ]
}}
"#
        ),
    )
    .expect("write");
    assert_eq!(
        resolve_lockfile(&path).expect_err("duplicate").code(),
        "plugin_lock_duplicate"
    );
}

#[test]
fn lockfile_substitution_is_digest_mismatch() {
    let host = PluginHost::try_new(
        PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
    )
    .expect("host");
    let bytes = published_wasm("calculator");
    let mut flipped = bytes.clone();
    flipped[0] ^= 0x01;
    let dir = tempfile::tempdir().expect("tempdir");
    let wasm = dir.path().join("component.wasm");
    std::fs::write(&wasm, &flipped).expect("write");
    let entry = crate::LockedPlugin {
        identity: "finstack.plugin.calculator".into(),
        version: "0.0.4".into(),
        enabled: true,
        component: "component.wasm".into(),
        component_path: wasm,
        component_digest: component_digest(&bytes),
        manifest: "plugin.manifest.json".into(),
        manifest_path: dir.path().join("plugin.manifest.json"),
        manifest_digest: "00".repeat(32),
    };
    let Err(error) = host.load_locked(&entry) else {
        unexpected_success(
            "toolset-plugin",
            "plugin.lock.digest",
            "substituted component loaded against the lock digest",
        );
    };
    assert_eq!(error.code(), "plugin_lock_digest_mismatch");
}

#[test]
fn lockfile_registry_url_path_is_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let hex = "ab".repeat(32);
    let path = dir.path().join("plugin.lock.json");
    std::fs::write(
        &path,
        format!(
            r#"{{
  "lockfile_version": 1,
  "plugins": [
    {{
      "identity": "finstack.plugin.calculator",
      "version": "0.0.4",
      "enabled": true,
      "component": "https://example.invalid/p.wasm",
      "component_digest": "{hex}",
      "manifest": "calculator/plugin.manifest.json",
      "manifest_digest": "{hex}"
    }}
  ]
}}
"#
        ),
    )
    .expect("write");
    assert_eq!(
        resolve_lockfile(&path).expect_err("url").code(),
        "plugin_lock_invalid"
    );
}

#[test]
fn disabled_sandbox_is_absent_from_load_enabled() {
    let host = PluginHost::try_new(
        PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
    )
    .expect("host");
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
    assert!(!identities.contains(&"finstack.plugin.filesystem.sandbox"));
}

#[test]
fn cache_abi_field_still_distinguishes_worlds() {
    let host = PluginHost::try_new(
        PluginHostConfig::try_new(None, InstancePolicy::Exclusive, 2).expect("cfg"),
    )
    .expect("host");
    let bytes = fixture_wasm("echo-toolset");
    let base = CacheKeyParts {
        digest: component_digest(&bytes),
        engine: engine_fingerprint(),
        target: host_target(),
        abi: abi_identity("toolset-plugin", "0.0.4"),
    };
    assert!(!host.compile_with_key(&bytes, &base).expect("miss"));
    assert!(host.compile_with_key(&bytes, &base).expect("hit"));
    assert!(!host.cache_hit(&CacheKeyParts {
        abi: abi_identity("context-plugin", "0.0.4"),
        ..base.clone()
    }));
    assert!(!host.cache_hit(&CacheKeyParts {
        engine: engine_fingerprint_parts("0.0.0-test", CONFIG_FINGERPRINT),
        ..base
    }));
}

#[test]
fn published_failure_names_world_contract_and_suite_version() {
    let failure = PluginConformanceFailure::new(
        "toolset-plugin",
        "plugin.call.trap",
        "deliberate suite failure",
    );
    let text = failure.to_string();
    assert!(text.contains("toolset-plugin"), "{text}");
    assert!(text.contains("plugin.call.trap"), "{text}");
    assert!(text.contains(PLUGIN_CONFORMANCE_SUITE_VERSION), "{text}");
    assert!(!text.contains("assert failed"), "{text}");
}

fn hex_to_32(input: &str) -> [u8; 32] {
    let trimmed = input.trim();
    let mut out = [0_u8; 32];
    for (index, chunk) in trimmed.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).expect("utf8");
        out[index] = u8::from_str_radix(text, 16).expect("hex");
    }
    out
}
