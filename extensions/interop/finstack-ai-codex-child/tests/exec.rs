//! Integration tests for spawning and supervising `codex exec` runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use finstack_ai_codex_child::{
    CodexChildInvoker, CodexExecConfig, CodexRunReport, CodexRunStatus, CodexSandboxMode,
    codex_agent_ref, codex_route_ref,
};
use finstack_ai_kernel::{
    BudgetRequest, ChildPlacement, ChildRunLocator, ContentBlock, EffectId, LaneId, Metadata,
    OperationLocator, PrincipalRef, RunId, SessionId, TextBlock,
};
use finstack_ai_runtime::child::{
    AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunRequest,
};
use finstack_ai_runtime::ports::model::AuthorizationContext;

fn fake_invoker(mode: &str, workspace: &std::path::Path) -> CodexChildInvoker {
    CodexChildInvoker::try_new(CodexExecConfig {
        binary: std::path::PathBuf::from(env!("CARGO_BIN_EXE_codex_fake")),
        workspace_root: workspace.to_path_buf(),
        sandbox: CodexSandboxMode::WorkspaceWrite,
        skip_git_repo_check: true,
        extra_args: vec!["--fake-mode".to_string(), mode.to_string()],
    })
    .expect("invoker")
}

fn child_context() -> ChildRunContext {
    ChildRunContext {
        parent: OperationLocator::try_new(
            "tenant-a",
            SessionId::from_bytes([4; 16]),
            LaneId::from_bytes([5; 16]),
            RunId::from_bytes([6; 16]),
        )
        .expect("parent"),
        parent_effect_id: EffectId::from_bytes([7; 16]),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("loopback", "tester", Some("tenant-a"))
                .expect("principal"),
            authentication_method: Arc::from("loopback"),
            assurance_level: Arc::from("low"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("1"),
            decision_id: Arc::from("d1"),
        },
    }
}

fn child_locator(placement: ChildPlacement) -> ChildRunLocator {
    let operation = OperationLocator::try_new(
        "tenant-a",
        SessionId::from_bytes([1; 16]),
        LaneId::from_bytes([2; 16]),
        RunId::from_bytes([3; 16]),
    )
    .expect("locator");
    let remote = match placement {
        ChildPlacement::RemoteChildSession => Some(codex_route_ref().expect("route")),
        _ => None,
    };
    ChildRunLocator { operation, remote }
}

fn request_for(prompt: &str, placement: ChildPlacement) -> ChildRunRequest {
    let locator = child_locator(placement);
    let input: Arc<[ContentBlock]> = Arc::from([ContentBlock::Text(
        TextBlock::try_new(prompt).expect("text"),
    )]);
    ChildRunRequest::try_new(
        codex_agent_ref().expect("agent"),
        input,
        placement,
        locator,
        None,
        BudgetRequest::default(),
        None,
        Metadata::empty(),
    )
    .expect("valid child request")
}

fn codex_request(prompt: &str) -> ChildRunRequest {
    request_for(prompt, ChildPlacement::RemoteChildSession)
}

async fn wait_until_settled(invoker: &CodexChildInvoker, run_id: &RunId) -> CodexRunReport {
    let start = Instant::now();
    loop {
        let locator = invoker.accepted_locator(&child_context().parent, run_id);
        if let Some(report) = locator
            .as_ref()
            .and_then(|locator| invoker.run_status(locator))
            && report.status != CodexRunStatus::Running
        {
            return report;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "codex fake run never settled"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn success_run_completes_with_message_and_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("success", dir.path());
    let request = codex_request("fix the failing test");
    let run_id = request.locator().operation.run_id;
    let handle = invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted");
    assert_eq!(handle.locator.operation.run_id, run_id);
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Completed);
    assert_eq!(report.thread_id.as_deref(), Some("thread-fake-1"));
    assert_eq!(report.last_message.as_deref(), Some("fake done"));
    assert_eq!(report.usage.map(|usage| usage.output_tokens), Some(5));
    assert_eq!(report.exit_code, Some(0));
}

#[tokio::test]
async fn failing_run_reports_failed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("fail", dir.path());
    let request = codex_request("do the impossible");
    let run_id = request.locator().operation.run_id;
    invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Failed);
    assert_eq!(report.stderr_tail, None, "quiet failure has no tail");
}

/// A real Codex failure often says nothing on stdout; the bounded stderr
/// tail is what makes it diagnosable.
#[tokio::test]
async fn failing_run_surfaces_the_stderr_tail() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("fail-noisy", dir.path());
    let request = codex_request("do the impossible loudly");
    let run_id = request.locator().operation.run_id;
    invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Failed);
    let tail = report.stderr_tail.expect("stderr tail");
    assert!(tail.contains("sandbox denied write"), "tail was {tail:?}");
}

/// NFR-4: the prompt never reaches argv, so a prompt that looks like a flag
/// is still a prompt. `codex_fake` exits nonzero if the stdin prompt also
/// appears in argv, so a completed run echoing the text proves it.
#[tokio::test]
async fn flag_shaped_prompt_is_never_parsed_as_a_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("echo", dir.path());
    let request = codex_request("--dangerously-bypass-approvals-and-sandbox");
    let run_id = request.locator().operation.run_id;
    invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Completed);
    assert_eq!(
        report.last_message.as_deref(),
        Some("stdin:--dangerously-bypass-approvals-and-sandbox")
    );
}

/// The prompt is written to the child's stdin, not placed in argv: the fake
/// labels the channel it read the prompt from and refuses argv duplicates.
#[tokio::test]
async fn prompt_is_delivered_via_stdin_not_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("echo", dir.path());
    let prompt = "refactor the parser\n\nkeep the tests green";
    let request = codex_request(prompt);
    let run_id = request.locator().operation.run_id;
    invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Completed);
    assert_eq!(
        report.last_message.as_deref(),
        Some(format!("stdin:{prompt}").as_str())
    );
}

/// A prompt over the 256 KiB cap is rejected before any process is spawned.
#[tokio::test]
async fn oversized_prompt_is_rejected_before_spawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("echo", dir.path());
    let prompt = "x".repeat(256 * 1_024 + 1);
    let request = codex_request(&prompt);
    let run_id = request.locator().operation.run_id;
    let error = invoker
        .start_or_attach(child_context(), request)
        .await
        .expect_err("rejected");
    assert!(matches!(error, AgentInvokeError::InvalidRequest { .. }));
    assert!(error.to_string().contains("256 KiB"), "error was {error}");
    assert!(
        invoker
            .accepted_locator(&child_context().parent, &run_id)
            .is_none(),
        "nothing was accepted or spawned"
    );

    // Exactly at the cap is still accepted and delivered intact over stdin,
    // well past the pipe buffer size.
    let request = codex_request(&"y".repeat(256 * 1_024));
    let run_id = request.locator().operation.run_id;
    invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted at the cap");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Completed);
    assert_eq!(report.last_message.as_deref(), Some("stdin-len:262144"));
}

/// Cancel kills the child's whole process group, not just the leader: a
/// grandchild the fake spawned (standing in for a command Codex ran) must
/// be gone afterwards. On the old leader-only `start_kill` path the
/// orphaned `sleep` would survive.
#[cfg(unix)]
#[tokio::test]
async fn cancel_terminates_process_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("hang-grandchild", dir.path());
    let request = codex_request("spawn a long command");
    let run_id = request.locator().operation.run_id;
    let handle = invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted");

    let start = Instant::now();
    let grandchild_pid = loop {
        let message = invoker
            .accepted_locator(&child_context().parent, &run_id)
            .and_then(|locator| invoker.run_status(&locator))
            .and_then(|report| report.last_message);
        if let Some(pid) = message {
            break pid;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "fake never reported its grandchild"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(process_is_alive(&grandchild_pid), "grandchild is running");

    invoker.cancel(&handle.locator).await.expect("cancelled");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Cancelled);

    // The grandchild is reparented and reaped asynchronously after the
    // group kill, so poll briefly instead of asserting once.
    let start = Instant::now();
    while process_is_alive(&grandchild_pid) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "grandchild {grandchild_pid} survived the group kill"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `true` while `pid` names a live (non-zombie) process.
#[cfg(unix)]
fn process_is_alive(pid: &str) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .expect("ps");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

#[tokio::test]
async fn equal_digest_attaches_and_different_digest_conflicts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("success", dir.path());
    let request = codex_request("task one");
    let first = invoker
        .start_or_attach(child_context(), request.clone())
        .await
        .expect("accepted");
    let second = invoker
        .start_or_attach(child_context(), request.clone())
        .await
        .expect("attached");
    assert_eq!(first, second);

    let altered = codex_request("task two");
    let error = invoker
        .start_or_attach(child_context(), altered)
        .await
        .expect_err("conflict");
    assert!(matches!(error, AgentInvokeError::Conflict { .. }));
}

#[tokio::test]
async fn wrong_placement_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("success", dir.path());
    let request = request_for("wrong placement", ChildPlacement::IsolatedChildSession);
    let error = invoker
        .start_or_attach(child_context(), request)
        .await
        .expect_err("rejected");
    assert!(matches!(error, AgentInvokeError::InvalidRequest { .. }));
}

/// Two equal requests racing on the same run id must produce exactly one
/// accepted run: the attach check and the spawn share one critical section,
/// so the loser never spawns a duplicate Codex process.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_equal_requests_accept_one_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = Arc::new(fake_invoker("hang", dir.path()));
    let request = codex_request("race me");
    let run_id = request.locator().operation.run_id;
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let invoker = Arc::clone(&invoker);
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            invoker
                .start_or_attach(child_context(), request)
                .await
                .expect("accepted")
        }));
    }
    let mut handles = Vec::new();
    for task in tasks {
        handles.push(task.await.expect("join"));
    }
    for handle in &handles {
        assert_eq!(handle, handles.first().expect("first"));
    }
    // A single accepted run means a single supervised process; cancelling
    // it settles that one run.
    invoker
        .cancel(&handles.first().expect("first").locator)
        .await
        .expect("cancelled");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Cancelled);
}

#[tokio::test]
async fn cancel_kills_a_hanging_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("hang", dir.path());
    let request = codex_request("never finish");
    let run_id = request.locator().operation.run_id;
    let handle = invoker
        .start_or_attach(child_context(), request)
        .await
        .expect("accepted");
    invoker.cancel(&handle.locator).await.expect("cancelled");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Cancelled);
}

#[tokio::test]
async fn cancel_of_unknown_locator_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("success", dir.path());
    let request = codex_request("never started");
    let error = invoker
        .cancel(request.locator())
        .await
        .expect_err("unaccepted");
    assert!(matches!(error, AgentInvokeError::Unavailable { .. }));
}
