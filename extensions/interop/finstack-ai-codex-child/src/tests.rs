use super::*;
use std::path::PathBuf;

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
    assert_eq!(error, CodexChildError::Configuration { reason: "binary_missing" });
}

#[test]
fn try_new_rejects_missing_workspace_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    config.workspace_root = PathBuf::from("/nonexistent/workspace");
    let error = CodexChildInvoker::try_new(config).expect_err("must fail closed");
    assert_eq!(error, CodexChildError::Configuration { reason: "workspace_root_missing" });
}

#[test]
fn try_new_rejects_nul_in_extra_args() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut config = valid_config(dir.path());
    config.extra_args = vec!["bad\0arg".to_string()];
    let error = CodexChildInvoker::try_new(config).expect_err("must fail closed");
    assert_eq!(error, CodexChildError::Configuration { reason: "extra_args_invalid" });
}

#[test]
fn try_new_accepts_valid_config_and_maps_sandbox_flags() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = CodexChildInvoker::try_new(valid_config(dir.path()));
    assert!(invoker.is_ok());
    assert_eq!(CodexSandboxMode::ReadOnly.flag(), "read-only");
    assert_eq!(CodexSandboxMode::WorkspaceWrite.flag(), "workspace-write");
    assert_eq!(CodexSandboxMode::DangerFullAccess.flag(), "danger-full-access");
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
