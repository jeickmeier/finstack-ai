use std::future::Future;
#[cfg(unix)]
use std::sync::Barrier;
use std::sync::{Arc, Mutex};

use finstack_ai_runtime::{
    ArtifactError, ArtifactId, ArtifactRef, ArtifactScope, ArtifactStore, AuthorizationContext,
    BlobRef, CancellationSignal, Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId,
    OperationLocator, PortFuture, PrincipalRef, RunCallContext, RunId, SessionId, ToolBatchId,
    ToolCallBlock, ToolCallId, ToolFailurePolicy, ToolStreamItem,
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

fn call(
    toolset: &FileSystemToolset,
    index: usize,
    arguments: &serde_json::Value,
) -> ValidatedToolCall {
    let spec = &toolset.tools[index];
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            context().tool_call_id,
            spec.model_name.as_ref(),
            RawJson::parse(serde_json::to_vec(arguments).expect("arguments"))
                .expect("raw arguments"),
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
    toolset: &FileSystemToolset,
    index: usize,
    arguments: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let mut stream = toolset
        .call(context(), call(toolset, index, &arguments))
        .await?;
    let item = stream.next().await.ok_or_else(|| {
        fs_tool_error(
            FILESYSTEM_IO_ERROR,
            ErrorCategory::Internal,
            "test stream ended early",
        )
    })??;
    assert!(stream.next().await.is_none());
    match item {
        ToolStreamItem::Completed(result) => Ok(result),
        _ => Err(fs_tool_error(
            FILESYSTEM_IO_ERROR,
            ErrorCategory::Internal,
            "test stream returned a non-terminal item",
        )),
    }
}

fn block_on_thread<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> T {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(future)
    })
    .join()
    .expect("thread")
}

#[test]
fn specifications_are_generated_once_and_reused() {
    let root = TempDir::new().expect("root");
    let toolset = FileSystemToolset::try_new(root.path()).expect("filesystem");
    let first = toolset.tools();
    let second = toolset.tools();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.len(), 6);
}

#[tokio::test]
async fn read_write_edit_list_glob_and_search_use_public_tool_calls() {
    let root = TempDir::new().expect("root");
    std::fs::create_dir(root.path().join("src")).expect("src");
    let toolset = FileSystemToolset::try_new(root.path()).expect("filesystem");

    invoke(
        &toolset,
        1,
        serde_json::json!({"path":"src/lib.rs","content":"alpha\nbeta alpha\n"}),
    )
    .await
    .expect("write");
    invoke(
        &toolset,
        2,
        serde_json::json!({"path":"src/lib.rs","old":"beta","new":"gamma"}),
    )
    .await
    .expect("edit");
    let read = invoke(&toolset, 0, serde_json::json!({"path":"src/lib.rs"}))
        .await
        .expect("read");
    assert!(read.output.as_str().contains("gamma alpha"));

    let list = invoke(&toolset, 3, serde_json::json!({"path":"src"}))
        .await
        .expect("list");
    assert!(list.output.as_str().contains("lib.rs"));
    let glob = invoke(&toolset, 4, serde_json::json!({"pattern":"src/**/*.rs"}))
        .await
        .expect("glob");
    assert!(glob.output.as_str().contains("src/lib.rs"));
    let search = invoke(
        &toolset,
        5,
        serde_json::json!({"query":"alpha","glob":"**/*.rs"}),
    )
    .await
    .expect("search");
    assert!(search.output.as_str().contains("\"line\":1"));
    assert!(search.output.as_str().contains("\"line\":2"));
}

#[cfg(unix)]
#[tokio::test]
async fn traversal_symlink_escape_and_protected_paths_fail_closed() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    std::fs::write(outside.path().join("secret"), "canary-secret").expect("secret");
    symlink(outside.path().join("secret"), root.path().join("escape")).expect("symlink");
    symlink(outside.path(), root.path().join("outside-dir")).expect("directory symlink");
    std::fs::create_dir(root.path().join(".git")).expect("git");
    std::fs::write(root.path().join(".git/config"), "protected").expect("protected");
    let toolset = FileSystemToolset::try_new(root.path()).expect("filesystem");

    let traversal = invoke(&toolset, 0, serde_json::json!({"path":"../secret"}))
        .await
        .expect_err("traversal denied");
    assert_eq!(traversal.code(), FILESYSTEM_INVALID_ARGUMENTS);
    let symlink = invoke(&toolset, 0, serde_json::json!({"path":"escape"}))
        .await
        .expect_err("symlink denied");
    assert_eq!(symlink.code(), FILESYSTEM_POLICY_DENIED);
    assert!(!symlink.to_string().contains("canary-secret"));
    let intermediate = invoke(
        &toolset,
        0,
        serde_json::json!({"path":"outside-dir/secret"}),
    )
    .await
    .expect_err("intermediate symlink denied");
    assert_eq!(intermediate.code(), FILESYSTEM_POLICY_DENIED);
    let write_escape = invoke(
        &toolset,
        1,
        serde_json::json!({"path":"escape","content":"overwrite"}),
    )
    .await
    .expect_err("write symlink denied");
    assert_eq!(write_escape.code(), FILESYSTEM_POLICY_DENIED);
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret")).expect("secret"),
        "canary-secret"
    );
    let protected = invoke(&toolset, 0, serde_json::json!({"path":".git/config"}))
        .await
        .expect_err("protected denied");
    assert_eq!(protected.code(), FILESYSTEM_POLICY_DENIED);
}

#[cfg(unix)]
#[test]
fn symlink_root_is_rejected_at_construction() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let parent = TempDir::new().expect("parent");
    let linked = parent.path().join("linked-root");
    symlink(root.path(), &linked).expect("root symlink");
    assert!(matches!(
        FileSystemToolset::try_new(linked),
        Err(FileSystemError::RootUnavailable)
    ));
}

#[cfg(unix)]
#[test]
fn symlink_swap_between_authorization_and_open_never_reads_outside() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    std::fs::write(root.path().join("target"), "inside").expect("inside");
    std::fs::write(outside.path().join("secret"), "canary-secret").expect("secret");
    let toolset = FileSystemToolset::try_new(root.path()).expect("filesystem");
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    toolset.root.set_test_hooks(crate::unix::TestHooks {
        before_final_open: Some({
            let entered = Arc::clone(&entered);
            let resume = Arc::clone(&resume);
            Arc::new(move || {
                entered.wait();
                resume.wait();
            })
        }),
        after_final_open: None,
    });
    let future = async move { invoke(&toolset, 0, serde_json::json!({"path":"target"})).await };
    let handle = std::thread::spawn(move || block_on_thread(future));
    entered.wait();
    std::fs::rename(root.path().join("target"), root.path().join("original")).expect("rename");
    symlink(outside.path().join("secret"), root.path().join("target")).expect("swap");
    resume.wait();
    let error = handle
        .join()
        .expect("call thread")
        .expect_err("symlink rejected");
    assert_eq!(error.code(), FILESYSTEM_POLICY_DENIED);
    assert!(!error.to_string().contains("canary-secret"));
}

#[cfg(unix)]
#[test]
fn rename_after_open_reads_the_authorized_object_not_replacement() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    std::fs::write(root.path().join("target"), "inside").expect("inside");
    std::fs::write(outside.path().join("secret"), "canary-secret").expect("secret");
    let toolset = FileSystemToolset::try_new(root.path()).expect("filesystem");
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    toolset.root.set_test_hooks(crate::unix::TestHooks {
        before_final_open: None,
        after_final_open: Some({
            let entered = Arc::clone(&entered);
            let resume = Arc::clone(&resume);
            Arc::new(move || {
                entered.wait();
                resume.wait();
            })
        }),
    });
    let future = async move { invoke(&toolset, 0, serde_json::json!({"path":"target"})).await };
    let handle = std::thread::spawn(move || block_on_thread(future));
    entered.wait();
    std::fs::rename(root.path().join("target"), root.path().join("original")).expect("rename");
    symlink(outside.path().join("secret"), root.path().join("target")).expect("swap");
    resume.wait();
    let result = handle
        .join()
        .expect("call thread")
        .expect("read opened object");
    assert!(result.output.as_str().contains("inside"));
    assert!(!result.output.as_str().contains("canary-secret"));
}

#[derive(Clone, Default)]
struct CaptureArtifactStore {
    staged: Arc<Mutex<Vec<(ArtifactScope, Bytes, ArtifactMetadata)>>>,
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
            staged.lock().expect("capture lock").push((
                scope.clone(),
                content.clone(),
                metadata.clone(),
            ));
            let content_digest = Digest::blob_content(&content);
            let blob = BlobRef::try_new(
                "capture-blob",
                metadata.media_type.as_ref(),
                u64::try_from(content.len()).expect("length"),
                Some(content_digest),
                metadata.name.as_deref(),
            )
            .expect("blob");
            Ok(ArtifactRef::try_new(
                ArtifactId::from_bytes([9; 16]),
                metadata.kind.as_ref(),
                blob,
                content_digest,
                scope.digest().expect("scope digest"),
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
        Box::pin(async {
            Err(ArtifactError::NotFound {
                code: finstack_ai_runtime::ARTIFACT_NOT_FOUND,
            })
        })
    }
}

#[tokio::test]
async fn oversized_output_requires_and_uses_exact_scoped_artifact_service() {
    let root = TempDir::new().expect("root");
    std::fs::write(root.path().join("large.txt"), "x".repeat(256)).expect("large");
    let limits = FileSystemLimits {
        inline_result_bytes: 32,
        ..FileSystemLimits::default()
    };
    let without_store = FileSystemToolset::try_new(root.path())
        .expect("filesystem")
        .try_with_limits(limits)
        .expect("limits");
    let error = invoke(&without_store, 0, serde_json::json!({"path":"large.txt"}))
        .await
        .expect_err("artifact required");
    assert_eq!(error.code(), FILESYSTEM_ARTIFACT_REQUIRED);

    let store = CaptureArtifactStore::default();
    let toolset = FileSystemToolset::try_new(root.path())
        .expect("filesystem")
        .try_with_limits(limits)
        .expect("limits")
        .with_artifact_store(Arc::new(store.clone()), Sensitivity::Confidential);
    let result = invoke(&toolset, 0, serde_json::json!({"path":"large.txt"}))
        .await
        .expect("artifact reference");
    assert!(result.output.as_str().contains("capture-blob"));
    let staged = store.staged.lock().expect("capture lock");
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].0.tenant_scope.as_ref(), "tenant-a");
    assert_eq!(staged[0].0.run_id, Some(context().run.locator.run_id));
    assert_eq!(staged[0].0.sensitivity, Sensitivity::Confidential);
    assert!(staged[0].1.len() > limits.inline_result_bytes);
    assert!(staged[0].2.attributes.as_str().contains("effect_id"));
}
