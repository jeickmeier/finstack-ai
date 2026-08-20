//! Frozen construction-time configuration for the Codex exec invoker.

use std::path::PathBuf;

/// Codex sandbox mode, mapped one-to-one onto `codex exec --sandbox`.
///
/// Approvals are frozen here (spec decision D2): `codex exec` is
/// non-interactive, so the host chooses the blast radius up front instead
/// of bridging per-command approvals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSandboxMode {
    /// Codex may read the workspace but not write or run mutating commands.
    ReadOnly,
    /// Codex may edit files and run commands inside the workspace root.
    WorkspaceWrite,
    /// No Codex sandbox. Only for hosts that provide their own isolation.
    DangerFullAccess,
}

impl CodexSandboxMode {
    /// The `--sandbox` flag value for this mode.
    #[must_use]
    pub const fn flag(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

/// Explicit, frozen invoker configuration. No env-var discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExecConfig {
    /// Absolute path to the `codex` binary. Auth stays inside Codex.
    pub binary: PathBuf,
    /// Workspace the child operates on (`codex exec --cd`).
    pub workspace_root: PathBuf,
    /// Sandbox mode; there is deliberately no default.
    pub sandbox: CodexSandboxMode,
    /// Pass `--skip-git-repo-check` (needed when the root is not a repo).
    pub skip_git_repo_check: bool,
    /// Extra frozen arguments inserted before the prompt separator.
    pub extra_args: Vec<String>,
}
