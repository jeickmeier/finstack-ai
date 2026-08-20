use super::*;
use crate::CodexRunStatus;
use crate::events::{CodexEvent, parse_event};
use crate::invoker::CodexRun;
use crate::state::RunState;
use finstack_ai_kernel::{
    BudgetRequest, ChildPlacement, ChildRunLocator, ContentBlock, Digest, EffectId, LaneId,
    Metadata, OperationLocator, PrincipalRef, RunId, SessionId, TextBlock,
};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, AuthorizationContext, ChildRunContext, ChildRunHandle,
    ChildRunRequest,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn valid_config(dir: &std::path::Path) -> CodexExecConfig {
    let binary = dir.join("codex");
    std::fs::write(&binary, b"#!/bin/sh\n").expect("write fake binary");
    CodexExecConfig {
        binary,
        workspace_root: dir.to_path_buf(),
        sandbox: CodexSandboxMode::WorkspaceWrite,
        skip_git_repo_check: true,
        extra_args: Vec::new(),
    }
}

#[test]
fn try_new_rejects_missing_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    config.binary = PathBuf::from("/nonexistent/codex-binary");
    let error = CodexChildInvoker::try_new(config).expect_err("must fail closed");
    assert_eq!(
        error,
        CodexChildError::Configuration {
            reason: "binary_missing"
        }
    );
}

#[test]
fn try_new_rejects_missing_workspace_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    config.workspace_root = PathBuf::from("/nonexistent/workspace");
    let error = CodexChildInvoker::try_new(config).expect_err("must fail closed");
    assert_eq!(
        error,
        CodexChildError::Configuration {
            reason: "workspace_root_missing"
        }
    );
}

#[test]
fn try_new_rejects_nul_in_extra_args() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    config.extra_args = vec!["bad\0arg".to_string()];
    let error = CodexChildInvoker::try_new(config).expect_err("must fail closed");
    assert_eq!(
        error,
        CodexChildError::Configuration {
            reason: "extra_args_invalid"
        }
    );
}

#[test]
fn try_new_accepts_valid_config_and_maps_sandbox_flags() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = CodexChildInvoker::try_new(valid_config(dir.path()));
    assert!(invoker.is_ok());
    assert_eq!(CodexSandboxMode::ReadOnly.flag(), "read-only");
    assert_eq!(CodexSandboxMode::WorkspaceWrite.flag(), "workspace-write");
    assert_eq!(
        CodexSandboxMode::DangerFullAccess.flag(),
        "danger-full-access"
    );
}

#[test]
fn parses_thread_started() {
    let event = parse_event(r#"{"type":"thread.started","thread_id":"thread-1"}"#);
    assert_eq!(
        event,
        CodexEvent::ThreadStarted {
            thread_id: "thread-1".to_string()
        }
    );
}

#[test]
fn parses_agent_message_item() {
    let event = parse_event(
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"done"}}"#,
    );
    assert_eq!(
        event,
        CodexEvent::AgentMessage {
            text: "done".to_string()
        }
    );
}

#[test]
fn parses_turn_completed_usage() {
    let event = parse_event(
        r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#,
    );
    let CodexEvent::TurnCompleted { usage: Some(usage) } = event else {
        panic!("expected usage");
    };
    assert_eq!(
        (
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.output_tokens
        ),
        (10, 2, 5)
    );
}

#[test]
fn parses_failures() {
    assert_eq!(
        parse_event(r#"{"type":"turn.failed","error":{"message":"boom"}}"#),
        CodexEvent::Failed {
            message: "boom".to_string()
        },
    );
    assert_eq!(
        parse_event(r#"{"type":"error","message":"broke"}"#),
        CodexEvent::Failed {
            message: "broke".to_string()
        },
    );
}

#[test]
fn unknown_and_malformed_lines_are_other() {
    assert_eq!(parse_event(r#"{"type":"turn.started"}"#), CodexEvent::Other);
    assert_eq!(parse_event("not json"), CodexEvent::Other);
    assert_eq!(parse_event(""), CodexEvent::Other);
    assert_eq!(
        parse_event(r#"{"type":"item.completed","item":{"type":"command_execution"}}"#),
        CodexEvent::Other
    );
}

#[test]
fn reduces_success_run() {
    let mut state = RunState::default();
    state.apply(parse_event(r#"{"type":"thread.started","thread_id":"t1"}"#));
    state.apply(parse_event(
        r#"{"type":"item.completed","item":{"type":"agent_message","text":"hi"}}"#,
    ));
    state.apply(parse_event(r#"{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}"#));
    assert_eq!(state.report().status, CodexRunStatus::Running);
    state.record_exit(Some(0));
    let report = state.report();
    assert_eq!(report.status, CodexRunStatus::Completed);
    assert_eq!(report.thread_id.as_deref(), Some("t1"));
    assert_eq!(report.last_message.as_deref(), Some("hi"));
    assert_eq!(report.exit_code, Some(0));
}

#[test]
fn nonzero_exit_or_failure_event_is_failed() {
    let mut state = RunState::default();
    state.record_exit(Some(1));
    assert_eq!(state.report().status, CodexRunStatus::Failed);

    let mut state = RunState::default();
    state.apply(parse_event(
        r#"{"type":"turn.failed","error":{"message":"boom"}}"#,
    ));
    state.record_exit(Some(0));
    assert_eq!(state.report().status, CodexRunStatus::Failed);
}

#[test]
fn cancelled_wins_over_exit_status() {
    let mut state = RunState::default();
    state.mark_cancelled();
    state.record_exit(Some(137));
    assert_eq!(state.report().status, CodexRunStatus::Cancelled);
}

#[test]
fn cancel_after_exit_does_not_rewrite_terminal_status() {
    let mut state = RunState::default();
    state.record_exit(Some(0));
    state.mark_cancelled();
    assert_eq!(state.report().status, CodexRunStatus::Completed);

    let mut failed = RunState::default();
    failed.record_exit(Some(1));
    failed.mark_cancelled();
    assert_eq!(failed.report().status, CodexRunStatus::Failed);
}

#[test]
fn stderr_tail_is_bounded_and_only_reported_on_failure() {
    const LINE: &str = "é-noisy-line-of-child-stderr-output-padding";
    let mut state = RunState::default();
    let mut written = String::new();
    for _ in 0..600 {
        state.append_stderr(LINE);
        written.push_str(LINE);
        written.push('\n');
    }
    state.record_exit(Some(0));
    assert_eq!(state.report().stderr_tail, None);

    state.record_exit(Some(1));
    let tail = state.report().stderr_tail.expect("failed run has a tail");
    assert!(tail.len() <= 4_096, "tail is {} bytes", tail.len());
    // The retained window is an exact suffix of what the child wrote:
    // truncation snapped forward to a char boundary, so no `é` was split.
    assert!(written.ends_with(&tail));
    assert!(tail.len() > 4_000, "tail is {} bytes", tail.len());
}

fn test_locator(run_byte: u8) -> ChildRunLocator {
    let operation = OperationLocator::try_new(
        "tenant-a",
        SessionId::from_bytes([1; 16]),
        LaneId::from_bytes([2; 16]),
        RunId::from_bytes([run_byte; 16]),
    )
    .expect("locator");
    ChildRunLocator {
        operation,
        remote: Some(codex_route_ref().expect("route")),
    }
}

fn test_context() -> ChildRunContext {
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

fn test_request(run_byte: u8) -> ChildRunRequest {
    let locator = test_locator(run_byte);
    let input: Arc<[ContentBlock]> = Arc::from([ContentBlock::Text(
        TextBlock::try_new("evict me").expect("text"),
    )]);
    ChildRunRequest {
        agent: codex_agent_ref().expect("agent"),
        input,
        placement: ChildPlacement::RemoteChildSession,
        locator,
        requested_deadline: None,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::domain_separated("child-run-request", 1, b"unit-test")
            .expect("digest"),
    }
}

/// Fill the run table with synthetic entries in a chosen state. The entries
/// hold no child process, which is exactly what a settled run looks like
/// after its supervisor finished.
fn fill_runs(invoker: &CodexChildInvoker, count: u8, settled: bool) {
    let mut guard = invoker.runs.lock().expect("run table");
    for index in 0..count {
        let mut state = RunState::default();
        if settled {
            state.record_exit(Some(0));
        }
        let locator = test_locator(200 + index);
        guard.insert(
            locator.operation.run_id,
            CodexRun {
                request_digest: Digest::domain_separated("child-run-request", 1, b"filler")
                    .expect("digest"),
                handle: ChildRunHandle {
                    locator,
                    relation_digest: Digest::domain_separated("child-relation", 1, b"filler")
                        .expect("digest"),
                },
                state: Arc::new(Mutex::new(state)),
                child: Arc::new(tokio::sync::Mutex::new(None)),
            },
        );
    }
}

/// A capacity-bound host that never evicts would answer `Unavailable`
/// forever once 1 024 runs settled; settled runs are pure history and get
/// evicted to make room.
#[tokio::test]
async fn full_table_of_settled_runs_evicts_and_accepts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    // A real, immediately-exiting executable: the test only needs the
    // spawn to succeed, not Codex semantics.
    config.binary = PathBuf::from("/bin/echo");
    let invoker = CodexChildInvoker::try_new_with_cap(config, 2).expect("invoker");
    fill_runs(&invoker, 2, true);

    invoker
        .start_or_attach(test_context(), test_request(9))
        .await
        .expect("settled runs make room");
    let guard = invoker.runs.lock().expect("run table");
    assert_eq!(guard.len(), 1, "both settled fillers were evicted");
}

/// Eviction leaves tombstones, so `start_or_attach` stays idempotent: an
/// equal replay of an evicted run attaches without spawning a second
/// child, and a differing digest still conflicts.
#[tokio::test]
async fn evicted_run_replay_attaches_and_conflicts_without_respawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    config.binary = PathBuf::from("/bin/echo");
    let invoker = CodexChildInvoker::try_new_with_cap(config, 2).expect("invoker");
    fill_runs(&invoker, 2, true);

    invoker
        .start_or_attach(test_context(), test_request(9))
        .await
        .expect("settled runs make room");
    assert_eq!(invoker.runs.lock().expect("run table").len(), 1);

    let mut replay = test_request(200);
    replay.request_digest =
        Digest::domain_separated("child-run-request", 1, b"filler").expect("digest");
    invoker
        .start_or_attach(test_context(), replay)
        .await
        .expect("equal replay of the evicted run attaches");
    assert_eq!(
        invoker.runs.lock().expect("run table").len(),
        1,
        "the attach did not spawn or insert a second run"
    );

    let error = invoker
        .start_or_attach(test_context(), test_request(201))
        .await
        .expect_err("differing digest for an evicted run conflicts");
    assert!(matches!(error, AgentInvokeError::Conflict { .. }));
}

/// Running entries are never evicted, so a table genuinely full of live
/// children still fails closed.
#[tokio::test]
async fn full_table_of_running_runs_stays_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    config.binary = PathBuf::from("/bin/echo");
    let invoker = CodexChildInvoker::try_new_with_cap(config, 2).expect("invoker");
    fill_runs(&invoker, 2, false);

    let error = invoker
        .start_or_attach(test_context(), test_request(9))
        .await
        .expect_err("table is full of running children");
    assert!(matches!(error, AgentInvokeError::Unavailable { .. }));
    assert_eq!(invoker.runs.lock().expect("run table").len(), 2);
}

#[test]
fn codex_agent_ref_is_stable() {
    let first = codex_agent_ref().expect("agent ref");
    let second = codex_agent_ref().expect("agent ref");
    assert_eq!(first.id.to_string(), "finstack.peer.codex");
    assert!(first.bundle.is_none());
    assert_eq!(first.spec_digest, second.spec_digest);
}

#[test]
fn codex_route_ref_targets_the_peer_component() {
    let route = codex_route_ref().expect("route ref");
    assert_eq!(route.route.handle(), "codex-exec");
    assert_eq!(route.route.provider().to_string(), "finstack.peer.codex");
}
