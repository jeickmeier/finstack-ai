use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_runtime::{
    ArtifactError, ArtifactId, ArtifactMetadata, ArtifactRef, ArtifactScope, ArtifactStore,
    AuthorizationContext, BlobRef, Bytes, CancellationSignal, Digest, EffectId,
    EffectOutputContract, EffectOutputKind, LaneId, OperationLocator, PortFuture, PrincipalRef,
    RawJson, RunCallContext, RunId, Sensitivity, SessionId, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolFailurePolicy, ToolStreamItem, Toolset,
};
use futures_util::StreamExt;
use tempfile::TempDir;

use super::*;

fn id<T>(value: u64, parse: impl FnOnce(&str) -> T) -> T {
    parse(&format!("00000000-0000-7000-8000-{value:012x}"))
}

fn context() -> ToolCallContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    ToolCallContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                id(1, |value| SessionId::parse(value).expect("session")),
                id(2, |value| LaneId::parse(value).expect("lane")),
                id(3, |value| RunId::parse(value).expect("run")),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal,
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: id(4, |value| EffectId::parse(value).expect("effect")),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        tool_batch_id: id(5, |value| ToolBatchId::parse(value).expect("batch")),
        tool_call_id: id(6, |value| ToolCallId::parse(value).expect("call")),
    }
}

fn call(toolset: &ShellToolset, arguments: &serde_json::Value) -> ValidatedToolCall {
    let spec = &toolset.tools()[0];
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            context().tool_call_id,
            spec.model_name.as_ref(),
            RawJson::parse(serde_json::to_vec(arguments).expect("arguments")).expect("raw"),
        )
        .expect("tool call"),
        tool_id: spec.id.clone(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"{}"),
        },
        retry_safety: spec.retry_safety,
        deadline: None,
        execution: spec.execution,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

async fn invoke(
    toolset: &ShellToolset,
    arguments: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let mut stream = toolset.call(context(), call(toolset, &arguments)).await?;
    let item = stream.next().await.expect("item").expect("ok");
    assert!(stream.next().await.is_none());
    match item {
        ToolStreamItem::Completed(result) => Ok(result),
        _ => panic!("expected completed tool result"),
    }
}

fn echo_policy() -> ShellPolicy {
    ShellPolicy::try_new(["/bin/echo", "/bin/sleep", "/bin/cat", "/usr/bin/env"])
        .expect("policy")
        .with_locale_env()
}

#[cfg(unix)]
#[tokio::test]
async fn deny_by_default_rejects_unknown_and_relative_path_search() {
    let toolset = ShellToolset::try_new(echo_policy(), None).expect("shell");
    let denied = invoke(&toolset, serde_json::json!({"argv":["printenv"]}))
        .await
        .expect_err("denied");
    assert_eq!(denied.code(), SHELL_POLICY_DENIED);

    let relative = invoke(&toolset, serde_json::json!({"argv":["echo","hi"]}))
        .await
        .expect_err("basename without exact path");
    assert_eq!(relative.code(), SHELL_POLICY_DENIED);
}

#[cfg(unix)]
#[allow(unsafe_code)]
#[tokio::test]
async fn empty_environment_does_not_leak_host_secrets() {
    // SAFETY: the test process owns this temporary canary variable and
    // removes it before the test returns.
    unsafe {
        std::env::set_var("HOST_SECRET", "canary-shell-secret");
    }
    let toolset = ShellToolset::try_new(echo_policy(), None).expect("shell");
    let result = invoke(&toolset, serde_json::json!({"argv":["/usr/bin/env"]}))
        .await
        .expect("env");
    assert!(!result.output.as_str().contains("canary-shell-secret"));
    assert!(!result.output.as_str().contains("HOST_SECRET"));
    unsafe {
        std::env::remove_var("HOST_SECRET");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_sleep_without_fabricating_success() {
    let toolset = ShellToolset::try_new(echo_policy(), None)
        .expect("shell")
        .try_with_limits(ShellLimits {
            timeout: Duration::from_millis(100),
            max_output_bytes: 4_096,
            inline_result_bytes: 4_096,
        })
        .expect("limits");
    let error = invoke(&toolset, serde_json::json!({"argv":["/bin/sleep","2"]}))
        .await
        .expect_err("timeout");
    assert_eq!(error.code(), SHELL_TIMEOUT);
}

#[cfg(unix)]
#[tokio::test]
async fn output_flood_fails_closed_or_stages_artifact() {
    let root = TempDir::new().expect("root");
    std::fs::write(root.path().join("huge.txt"), "x".repeat(8_192)).expect("huge");
    let limits = ShellLimits {
        timeout: Duration::from_secs(2),
        max_output_bytes: 1_024,
        inline_result_bytes: 256,
    };
    let huge = root.path().join("huge.txt");
    let without_store = ShellToolset::try_new(echo_policy(), Some(root.path()))
        .expect("shell")
        .try_with_limits(limits)
        .expect("limits");
    let flood = invoke(
        &without_store,
        serde_json::json!({"argv":["/bin/cat", huge.to_string_lossy()]}),
    )
    .await
    .expect_err("flood");
    assert_eq!(flood.code(), SHELL_LIMIT_EXCEEDED);

    let store = CaptureArtifactStore::default();
    let with_store = ShellToolset::try_new(echo_policy(), Some(root.path()))
        .expect("shell")
        .try_with_limits(ShellLimits {
            timeout: Duration::from_secs(2),
            max_output_bytes: 16_384,
            inline_result_bytes: 256,
        })
        .expect("limits")
        .with_artifact_store(Arc::new(store), Sensitivity::Internal);
    let staged = invoke(
        &with_store,
        serde_json::json!({"argv":["/bin/cat", huge.to_string_lossy()]}),
    )
    .await
    .expect("artifact staged");
    assert!(staged.output.as_str().contains("artifact"));
}

#[cfg(unix)]
#[tokio::test]
async fn confined_shell_cannot_read_outside_declared_root() {
    let root = TempDir::new().expect("root");
    let root_path = root.path().canonicalize().expect("canonical root");
    std::fs::write(root_path.join("inside.txt"), "inside-ok").expect("inside");
    let outside = TempDir::new().expect("outside");
    let secret_path = outside
        .path()
        .canonicalize()
        .expect("canonical outside")
        .join("secret.txt");
    std::fs::write(&secret_path, "SECRET-OUTSIDE-ROOT").expect("secret");
    let policy = ShellPolicy::try_new(["/bin/cat"]).expect("policy");
    let toolset = ShellToolset::try_new(policy, Some(&root_path))
        .expect("shell")
        .try_with_confinement()
        .expect("confinement");
    assert_eq!(
        ProcessCommandSandbox::confined(
            finstack_ai_runtime::ConfinementProfile::try_new(&root_path).expect("profile")
        )
        .expect("sandbox")
        .kind(),
        ProcessSandboxKind::Confined
    );

    let inside_path = root_path.join("inside.txt");
    let inside = invoke(
        &toolset,
        serde_json::json!({"argv":["/bin/cat", inside_path.to_string_lossy()]}),
    )
    .await
    .expect("inside readable");
    assert!(
        inside.output.as_str().contains("inside-ok"),
        "inside cat failed: {}",
        inside.output.as_str()
    );

    let outside_result = invoke(
        &toolset,
        serde_json::json!({"argv":["/bin/cat", secret_path.to_string_lossy()]}),
    )
    .await;
    match outside_result {
        Ok(result) => {
            assert!(
                !result.output.as_str().contains("SECRET-OUTSIDE-ROOT"),
                "confined cat must not return bytes from outside the root"
            );
            assert!(result.is_error);
        }
        Err(error) => {
            assert_ne!(error.code(), SHELL_POLICY_DENIED);
            assert!(!error.message().contains("SECRET-OUTSIDE-ROOT"));
        }
    }
}

#[test]
fn unconfined_process_runner_is_labeled() {
    assert_eq!(
        ProcessCommandSandbox::unconfined().kind(),
        ProcessSandboxKind::UnconfinedStdProcess
    );
}

#[cfg(unix)]
#[tokio::test]
async fn echo_under_policy_returns_stdout() {
    let toolset = ShellToolset::try_new(echo_policy(), None).expect("shell");
    let result = invoke(
        &toolset,
        serde_json::json!({"argv":["/bin/echo","hello-shell"]}),
    )
    .await
    .expect("echo");
    assert!(result.output.as_str().contains("hello-shell"));
    assert!(!result.is_error);
}

#[derive(Clone, Default)]
struct CaptureArtifactStore {
    staged: Arc<Mutex<Vec<Bytes>>>,
}

impl ArtifactStore for CaptureArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        let staged = Arc::clone(&self.staged);
        Box::pin(async move {
            staged.lock().expect("lock").push(content.clone());
            let digest = Digest::blob_content(&content);
            let blob = BlobRef::try_new(
                "shell-blob",
                metadata.media_type.as_ref(),
                u64::try_from(content.len()).expect("len"),
                Some(digest),
                metadata.name.as_deref(),
            )
            .expect("blob");
            Ok(ArtifactRef::try_new(
                ArtifactId::from_bytes([7; 16]),
                metadata.kind.as_ref(),
                blob,
                digest,
                scope.digest()?,
                metadata.attributes,
            )
            .expect("artifact"))
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        _artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        Box::pin(async { Err(ArtifactError::NotFound) })
    }
}
