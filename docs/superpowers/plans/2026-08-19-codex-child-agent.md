# Codex Child Agent (External Peer Invoker) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A T1 native crate that lets a parent finstack agent delegate a coding task to a locally installed OpenAI Codex CLI as a `RemoteChildSession` child — `CodexChildInvoker` (an `AgentInvoker` over `codex exec --json`) plus a three-tool `CodexToolset` (`codex_start` / `codex_status` / `codex_cancel`).

**Architecture:** One new crate `extensions/interop/finstack-ai-codex-child` containing both the invoker and the toolset. The invoker spawns `codex exec --json` with sandbox/approval flags frozen at construction, supervises stdout JSONL into an in-memory run-state table, and implements attach/conflict semantics on `request_digest` like `RemoteChildInvoker`. The toolset mirrors `SubagentToolset` (frozen identity, locator + digest built in-toolset, no spawn authority). No kernel changes, no facade changes, no new provider.

**Tech Stack:** Rust workspace crate; `finstack-ai-kernel`, `finstack-ai-runtime` (`native-tokio`), `tokio` (`process`, `io-util`, `rt`, `sync`, `time`), `serde_json`, `thiserror`, `futures-util`; dev: `tempfile`, `tokio` macros; test double via `[[bin]] codex_fake` + `env!("CARGO_BIN_EXE_codex_fake")`.

**Spec:** `docs/superpowers/specs/2026-08-19-codex-child-agent.md` — read it first; it records the seam corrections (no `reconcile()` on `AgentInvoker`; `ExternalHandleRef` is immutable/precommitted; `finstack.peer.*` is minted here) and the frozen-approvals decision (D2). The ACP/Claude child variant is spec §8 and is **out of scope for this plan** (own crate, own plan, after this ships).

## Global Constraints

- Every source file starts with the standard extension lint header (copy verbatim from `extensions/toolsets/finstack-ai-tools-subagent/src/lib.rs:6-25`): `#![warn(missing_docs)]`, `#![forbid(unsafe_code)]`, `#![warn(clippy::float_cmp)]`, `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]`, `#![deny(clippy::panic)]`, `#![deny(clippy::unreachable)]`, plus the `#![cfg_attr(test, allow(...))]` relaxation and `#![doc(test(attr(allow(clippy::expect_used))))]`.
- `Cargo.toml` ends with `[lints] workspace = true`; package metadata uses `version.workspace = true` etc. (copy the subagent Cargo.toml header block).
- No `unwrap`/`expect`/`panic`/indexing-slicing outside `#[cfg(test)]`. Use `.get(..)`, `map_err`, and stable error-code strings.
- Checks: `cargo clippy -p finstack-ai-codex-child --all-targets --locked -- -D warnings`, `cargo nextest run -p finstack-ai-codex-child --locked`, and before finishing `mise run check-rust` + `mise run test-rust`.
- New `extensions/**` crates require a cargo-public-api baseline at `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-codex-child.txt` (Task 8).
- No network I/O; no reading `~/.codex`; binary path and workspace root are explicit constructor inputs.
- Commit after every task with a conventional message ending in the Claude co-author trailer.

---

### Task 1: Crate scaffold, workspace registration, and peer identity

**Files:**
- Create: `extensions/interop/finstack-ai-codex-child/Cargo.toml`
- Create: `extensions/interop/finstack-ai-codex-child/README.md`
- Create: `extensions/interop/finstack-ai-codex-child/src/lib.rs`
- Create: `extensions/interop/finstack-ai-codex-child/src/identity.rs`
- Create: `extensions/interop/finstack-ai-codex-child/src/tests.rs`
- Modify: `/Cargo.toml` (workspace `members` list — insert `"extensions/interop/finstack-ai-codex-child"` alphabetically next to the existing `"extensions/interop/finstack-ai-remote-child"` entry)

**Interfaces:**
- Produces: `pub const CODEX_PEER_AGENT_ID: &str = "finstack.peer.codex"`, `pub fn codex_agent_ref() -> Result<AgentRef, CodexChildError>`, `pub fn codex_route_ref() -> Result<RemoteRouteRef, CodexChildError>`, `pub enum CodexChildError { Configuration { reason: &'static str } }`, error-code consts `CODEX_CONFIGURATION_INVALID`, `CODEX_INVALID_ARGUMENTS`, `CODEX_CHILD_NOT_FOUND`. Tasks 2–7 use all of these.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "finstack-ai-codex-child"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Codex exec child-agent invoker and toolset for finstack-ai"
readme = "README.md"

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
futures-util = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["io-util", "process", "rt", "sync", "time"] }

[dev-dependencies]
tempfile = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[[bin]]
name = "codex_fake"
path = "src/bin/codex_fake.rs"

[lints]
workspace = true
```

(The `[[bin]]` target is filled in Task 5; declaring it now is fine once the file exists — if you prefer, add the `[[bin]]` block in Task 5 instead.)

- [ ] **Step 2: Register the workspace member**

Edit the root `/Cargo.toml` `members` array: add `"extensions/interop/finstack-ai-codex-child"` in alphabetical order. Do **not** add a `[workspace.dependencies]` entry — no other crate depends on this one yet.

- [ ] **Step 3: Write `README.md`**

```markdown
# finstack-ai-codex-child

Codex exec child-agent invoker and toolset. A parent finstack agent
delegates a coding task; a locally installed OpenAI Codex CLI runs it as a
spawned `codex exec --json` process using its own `codex login` credentials.
Finstack never reads `~/.codex`.

- `CodexChildInvoker` implements `AgentInvoker` for
  `ChildPlacement::RemoteChildSession` with the frozen peer identity
  `finstack.peer.codex`. Equal `request_digest` attaches; a differing digest
  is a conflict. Run state (thread id, last message, usage, exit) is held
  in memory; after a host restart, status is `unknown`.
- `CodexToolset` exposes `codex_start` / `codex_status` / `codex_cancel`.
  The prompt is the only model-supplied input; binary path, workspace root,
  and sandbox mode are frozen at construction.

This crate is a T1 native adapter. It is not isolated and is not compiled
into `wasm-host`. Codex's sandbox is Codex's own; the host chooses the
sandbox mode explicitly and should not also grant the parent an
unconstrained shell on the same tree.

Design: `docs/superpowers/specs/2026-08-19-codex-child-agent.md`.
```

- [ ] **Step 4: Write `src/lib.rs`**

```rust
//! Codex exec child-agent invoker and toolset.
//!
//! The crate holds no invocation authority beyond spawning the frozen
//! Codex binary. Child-run policy stays a runtime concern; policy, depth,
//! and budget failures surface as tool results.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
#![doc(test(attr(allow(clippy::expect_used))))]

use thiserror::Error;

mod identity;

#[cfg(test)]
mod tests;

pub use identity::{CODEX_PEER_AGENT_ID, codex_agent_ref, codex_route_ref};

/// Stable constructor failure code (missing binary, bad workspace, bad spec).
pub const CODEX_CONFIGURATION_INVALID: &str = "codex_configuration_invalid";
/// Tool arguments failed validation.
pub const CODEX_INVALID_ARGUMENTS: &str = "codex_invalid_arguments";
/// Named child is not in the in-process start table.
pub const CODEX_CHILD_NOT_FOUND: &str = "codex_child_not_found";

/// Construction failure for the invoker or toolset.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CodexChildError {
    /// Missing binary/workspace, or an invalid checked-in specification.
    #[error("{CODEX_CONFIGURATION_INVALID}: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}
```

- [ ] **Step 5: Write the failing identity tests in `src/tests.rs`**

```rust
use super::*;

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
```

(If `ExternalHandleRef::provider()` returns `&ComponentId`, `.to_string()` works; adjust the accessor call to the real signature in `crates/finstack-ai-kernel/src/primitives/handles.rs` if it differs.)

- [ ] **Step 6: Run to verify failure**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: FAIL to compile — `codex_agent_ref` not found.

- [ ] **Step 7: Write `src/identity.rs`**

```rust
//! Frozen peer identity for the Codex child agent.

use finstack_ai_kernel::{ComponentId, ComponentRef, Digest, ExternalHandleRef, RawJson, RemoteRouteRef, Version};
use finstack_ai_runtime::AgentRef;

use crate::CodexChildError;

/// Frozen allow-list identity for the Codex peer. Codex is not a
/// bundle-resolved finstack agent; the spec digest is a version marker,
/// not a kernel spec.
pub const CODEX_PEER_AGENT_ID: &str = "finstack.peer.codex";

const CODEX_ROUTE_LABEL: &str = "codex-exec";
const CODEX_SPEC_DOMAIN: &str = "codex-peer-spec";
const CODEX_SPEC_INPUT: &[u8] = b"codex-exec/v1";

/// Build the frozen [`AgentRef`] for the Codex peer.
///
/// # Errors
///
/// Returns [`CodexChildError::Configuration`] when the checked-in identity
/// cannot be constructed (never expected at runtime).
pub fn codex_agent_ref() -> Result<AgentRef, CodexChildError> {
    let id = finstack_ai_kernel::AgentId::parse(CODEX_PEER_AGENT_ID)
        .map_err(|_| configuration("peer_agent_id_invalid"))?;
    let spec_digest = Digest::domain_separated(CODEX_SPEC_DOMAIN, 1, CODEX_SPEC_INPUT)
        .map_err(|_| configuration("peer_spec_digest_failed"))?;
    Ok(AgentRef { id, bundle: None, spec_digest })
}

/// Build the stable [`RemoteRouteRef`] carried on every Codex child locator.
///
/// # Errors
///
/// Returns [`CodexChildError::Configuration`] when the checked-in route
/// cannot be constructed (never expected at runtime).
pub fn codex_route_ref() -> Result<RemoteRouteRef, CodexChildError> {
    let service = ComponentId::parse(CODEX_PEER_AGENT_ID)
        .map_err(|_| configuration("peer_component_id_invalid"))?;
    let metadata = RawJson::parse(b"{}").map_err(|_| configuration("route_metadata_invalid"))?;
    let route = ExternalHandleRef::try_new(service.clone(), CODEX_ROUTE_LABEL, metadata)
        .map_err(|_| configuration("route_handle_invalid"))?;
    Ok(RemoteRouteRef {
        service: ComponentRef::new(service, Some(Version { major: 1, minor: 0, patch: 0 })),
        route,
    })
}

pub(crate) fn configuration(reason: &'static str) -> CodexChildError {
    CodexChildError::Configuration { reason }
}
```

Check the exact import paths against `extensions/interop/finstack-ai-remote-child/src/route.rs:1-30` (it imports the same kernel types) and adjust `use` lines to match reality.

- [ ] **Step 8: Run tests to verify pass**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: PASS (2 tests).

- [ ] **Step 9: Lint and commit**

Run: `cargo clippy -p finstack-ai-codex-child --all-targets --locked -- -D warnings` then

```bash
git add extensions/interop/finstack-ai-codex-child Cargo.toml Cargo.lock
git commit -m "feat: scaffold finstack-ai-codex-child with frozen peer identity"
```

---

### Task 2: Exec config and fail-closed invoker construction

**Files:**
- Create: `extensions/interop/finstack-ai-codex-child/src/config.rs`
- Create: `extensions/interop/finstack-ai-codex-child/src/invoker.rs` (struct + `try_new` only)
- Modify: `extensions/interop/finstack-ai-codex-child/src/lib.rs` (add `mod config; mod invoker;` and re-exports)
- Test: `extensions/interop/finstack-ai-codex-child/src/tests.rs`

**Interfaces:**
- Produces: `pub struct CodexExecConfig { pub binary: PathBuf, pub workspace_root: PathBuf, pub sandbox: CodexSandboxMode, pub skip_git_repo_check: bool, pub extra_args: Vec<String> }`; `pub enum CodexSandboxMode { ReadOnly, WorkspaceWrite, DangerFullAccess }` with `pub const fn flag(self) -> &'static str`; `pub struct CodexChildInvoker` with `pub fn try_new(config: CodexExecConfig) -> Result<Self, CodexChildError>`. Tasks 5–7 consume all of these.

- [ ] **Step 1: Write failing construction tests (append to `src/tests.rs`)**

```rust
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
```

`tempfile` is already in dev-dependencies (Task 1). Note: `tempfile::tempdir` requires the crate name in `use` or fully-qualified calls as shown.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: FAIL to compile — `CodexExecConfig` not found.

- [ ] **Step 3: Write `src/config.rs`**

```rust
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
```

- [ ] **Step 4: Write `src/invoker.rs` (construction only)**

```rust
//! Codex exec child invoker.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::RunId;
use finstack_ai_runtime::ChildRunHandle;

use crate::config::CodexExecConfig;
use crate::identity::configuration;
use crate::CodexChildError;

pub(crate) const MAX_ACCEPTED: usize = 1_024;

/// One accepted child: precommitted identity plus live process state.
#[derive(Clone)]
pub(crate) struct CodexRun {
    pub(crate) request_digest: finstack_ai_kernel::Digest,
    pub(crate) handle: ChildRunHandle,
    pub(crate) state: Arc<Mutex<crate::state::RunState>>,
    pub(crate) child: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
}

/// `AgentInvoker` leaf that spawns `codex exec --json` with frozen flags.
pub struct CodexChildInvoker {
    pub(crate) config: CodexExecConfig,
    pub(crate) runs: Arc<Mutex<BTreeMap<RunId, CodexRun>>>,
}

impl CodexChildInvoker {
    /// Construct the invoker; fails closed on a missing binary or workspace.
    ///
    /// # Errors
    ///
    /// Returns [`CodexChildError::Configuration`] with reason
    /// `binary_missing`, `workspace_root_missing`, or `extra_args_invalid`.
    pub fn try_new(config: CodexExecConfig) -> Result<Self, CodexChildError> {
        if !config.binary.is_file() {
            return Err(configuration("binary_missing"));
        }
        if !config.workspace_root.is_dir() {
            return Err(configuration("workspace_root_missing"));
        }
        if config.extra_args.iter().any(|arg| arg.as_bytes().contains(&0)) {
            return Err(configuration("extra_args_invalid"));
        }
        Ok(Self {
            config,
            runs: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
}
```

Until Task 4 exists, stub the `state` field out: temporarily declare `CodexRun` without `state`/`child` (just `request_digest` + `handle`) and add the two fields in Tasks 4–5 — or create an empty `mod state` now. Choose the smaller diff: define `CodexRun` in Task 5 instead and keep only `CodexChildInvoker { config, runs: Arc<Mutex<BTreeMap<RunId, CodexRun>>> }` with `CodexRun` as a placeholder struct `pub(crate) struct CodexRun;` replaced in Task 5. Either way the tests in Step 1 only exercise `try_new`.

- [ ] **Step 5: Wire modules in `src/lib.rs`**

```rust
mod config;
mod invoker;

pub use config::{CodexExecConfig, CodexSandboxMode};
pub use invoker::CodexChildInvoker;
```

- [ ] **Step 6: Run tests to verify pass**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: PASS (6 tests).

- [ ] **Step 7: Lint and commit**

```bash
git add extensions/interop/finstack-ai-codex-child
git commit -m "feat: add CodexExecConfig and fail-closed invoker construction"
```

---

### Task 3: Tolerant JSONL event parser

**Files:**
- Create: `extensions/interop/finstack-ai-codex-child/src/events.rs`
- Modify: `extensions/interop/finstack-ai-codex-child/src/lib.rs` (`mod events;` + `pub use events::CodexUsage;`)
- Test: `extensions/interop/finstack-ai-codex-child/src/tests.rs`

**Interfaces:**
- Produces: `pub(crate) enum CodexEvent { ThreadStarted { thread_id: String }, AgentMessage { text: String }, TurnCompleted { usage: Option<CodexUsage> }, Failed { message: String }, Other }`; `pub struct CodexUsage { pub input_tokens: u64, pub cached_input_tokens: u64, pub output_tokens: u64 }`; `pub(crate) fn parse_event(line: &str) -> CodexEvent`. Task 4's reducer and Task 5's supervisor consume these.

- [ ] **Step 1: Write failing parser tests (append to `src/tests.rs`)**

```rust
use crate::events::{parse_event, CodexEvent};

#[test]
fn parses_thread_started() {
    let event = parse_event(r#"{"type":"thread.started","thread_id":"thread-1"}"#);
    assert_eq!(event, CodexEvent::ThreadStarted { thread_id: "thread-1".to_string() });
}

#[test]
fn parses_agent_message_item() {
    let event = parse_event(
        r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"done"}}"#,
    );
    assert_eq!(event, CodexEvent::AgentMessage { text: "done".to_string() });
}

#[test]
fn parses_turn_completed_usage() {
    let event = parse_event(
        r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#,
    );
    let CodexEvent::TurnCompleted { usage: Some(usage) } = event else {
        panic!("expected usage");
    };
    assert_eq!((usage.input_tokens, usage.cached_input_tokens, usage.output_tokens), (10, 2, 5));
}

#[test]
fn parses_failures() {
    assert_eq!(
        parse_event(r#"{"type":"turn.failed","error":{"message":"boom"}}"#),
        CodexEvent::Failed { message: "boom".to_string() },
    );
    assert_eq!(
        parse_event(r#"{"type":"error","message":"broke"}"#),
        CodexEvent::Failed { message: "broke".to_string() },
    );
}

#[test]
fn unknown_and_malformed_lines_are_other() {
    assert_eq!(parse_event(r#"{"type":"turn.started"}"#), CodexEvent::Other);
    assert_eq!(parse_event("not json"), CodexEvent::Other);
    assert_eq!(parse_event(""), CodexEvent::Other);
    assert_eq!(parse_event(r#"{"type":"item.completed","item":{"type":"command_execution"}}"#), CodexEvent::Other);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: FAIL to compile — `crate::events` not found.

- [ ] **Step 3: Write `src/events.rs`**

```rust
//! Tolerant parser for `codex exec --json` stdout lines.
//!
//! Unknown event types and malformed lines never fail a run; they parse to
//! [`CodexEvent::Other`]. Shapes were verified against the Codex CLI JSONL
//! contract; only the fields consumed here are assumed.

use serde::Serialize;

/// Token usage reported by `turn.completed`. Observational only (spec D8);
/// never charged against finstack budget in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct CodexUsage {
    /// Prompt tokens consumed by the turn.
    pub input_tokens: u64,
    /// Cached prompt tokens within `input_tokens`.
    pub cached_input_tokens: u64,
    /// Completion tokens produced by the turn.
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexEvent {
    ThreadStarted { thread_id: String },
    AgentMessage { text: String },
    TurnCompleted { usage: Option<CodexUsage> },
    Failed { message: String },
    Other,
}

pub(crate) fn parse_event(line: &str) -> CodexEvent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return CodexEvent::Other;
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("thread.started") => value
            .get("thread_id")
            .and_then(serde_json::Value::as_str)
            .map_or(CodexEvent::Other, |thread_id| CodexEvent::ThreadStarted {
                thread_id: thread_id.to_string(),
            }),
        Some("item.completed") => {
            let item = value.get("item");
            let is_message = item
                .and_then(|item| item.get("type"))
                .and_then(serde_json::Value::as_str)
                == Some("agent_message");
            let text = item
                .and_then(|item| item.get("text"))
                .and_then(serde_json::Value::as_str);
            match (is_message, text) {
                (true, Some(text)) => CodexEvent::AgentMessage { text: text.to_string() },
                _ => CodexEvent::Other,
            }
        }
        Some("turn.completed") => CodexEvent::TurnCompleted {
            usage: value.get("usage").map(|usage| CodexUsage {
                input_tokens: field_u64(usage, "input_tokens"),
                cached_input_tokens: field_u64(usage, "cached_input_tokens"),
                output_tokens: field_u64(usage, "output_tokens"),
            }),
        },
        Some("turn.failed") => CodexEvent::Failed {
            message: value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("codex turn failed")
                .to_string(),
        },
        Some("error") => CodexEvent::Failed {
            message: value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("codex reported an error")
                .to_string(),
        },
        _ => CodexEvent::Other,
    }
}

fn field_u64(value: &serde_json::Value, key: &str) -> u64 {
    value.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0)
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/interop/finstack-ai-codex-child
git commit -m "feat: parse codex exec JSONL events tolerantly"
```

---

### Task 4: Run-state reducer

**Files:**
- Create: `extensions/interop/finstack-ai-codex-child/src/state.rs`
- Modify: `extensions/interop/finstack-ai-codex-child/src/lib.rs` (`mod state;` + `pub use state::{CodexRunReport, CodexRunStatus};`)
- Test: `extensions/interop/finstack-ai-codex-child/src/tests.rs`

**Interfaces:**
- Consumes: `CodexEvent`, `CodexUsage` (Task 3).
- Produces: `pub enum CodexRunStatus { Running, Completed, Failed, Cancelled }`; `pub struct CodexRunReport { pub status: CodexRunStatus, pub thread_id: Option<String>, pub last_message: Option<String>, pub usage: Option<CodexUsage>, pub exit_code: Option<i32> }`; `pub(crate) struct RunState` with `apply(&mut self, event: CodexEvent)`, `record_exit(&mut self, code: Option<i32>)`, `mark_cancelled(&mut self)`, `report(&self) -> CodexRunReport`. Tasks 5–7 consume these.

- [ ] **Step 1: Write failing reducer tests (append to `src/tests.rs`)**

```rust
use crate::state::RunState;
use crate::CodexRunStatus;

#[test]
fn reduces_success_run() {
    let mut state = RunState::default();
    state.apply(parse_event(r#"{"type":"thread.started","thread_id":"t1"}"#));
    state.apply(parse_event(r#"{"type":"item.completed","item":{"type":"agent_message","text":"hi"}}"#));
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
    state.apply(parse_event(r#"{"type":"turn.failed","error":{"message":"boom"}}"#));
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
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: FAIL to compile — `crate::state` not found.

- [ ] **Step 3: Write `src/state.rs`**

```rust
//! In-memory run state reduced from Codex events and process exit.

use crate::events::{CodexEvent, CodexUsage};

/// Coarse child status surfaced through `codex_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexRunStatus {
    /// Process is still running.
    Running,
    /// Process exited zero without a reported failure.
    Completed,
    /// Process exited nonzero, or Codex reported `turn.failed` / `error`.
    Failed,
    /// A cancel was requested for this run; cancellation wins over exit.
    Cancelled,
}

/// Point-in-time status snapshot for one accepted child run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRunReport {
    /// Coarse status.
    pub status: CodexRunStatus,
    /// Codex thread id from `thread.started`, when seen.
    pub thread_id: Option<String>,
    /// Last `agent_message` text, when seen.
    pub last_message: Option<String>,
    /// Observational token usage from `turn.completed`, when seen.
    pub usage: Option<CodexUsage>,
    /// Process exit code, when the process has exited.
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RunState {
    thread_id: Option<String>,
    last_message: Option<String>,
    usage: Option<CodexUsage>,
    failure: Option<String>,
    exit_code: Option<i32>,
    cancelled: bool,
    exited: bool,
}

impl RunState {
    pub(crate) fn apply(&mut self, event: CodexEvent) {
        match event {
            CodexEvent::ThreadStarted { thread_id } => self.thread_id = Some(thread_id),
            CodexEvent::AgentMessage { text } => self.last_message = Some(text),
            CodexEvent::TurnCompleted { usage } => {
                if usage.is_some() {
                    self.usage = usage;
                }
            }
            CodexEvent::Failed { message } => self.failure = Some(message),
            CodexEvent::Other => {}
        }
    }

    pub(crate) fn record_exit(&mut self, code: Option<i32>) {
        self.exited = true;
        self.exit_code = code;
    }

    pub(crate) fn mark_cancelled(&mut self) {
        self.cancelled = true;
    }

    pub(crate) fn report(&self) -> CodexRunReport {
        let status = if self.cancelled {
            CodexRunStatus::Cancelled
        } else if !self.exited {
            CodexRunStatus::Running
        } else if self.failure.is_some() || self.exit_code != Some(0) {
            CodexRunStatus::Failed
        } else {
            CodexRunStatus::Completed
        };
        CodexRunReport {
            status,
            thread_id: self.thread_id.clone(),
            last_message: self.last_message.clone(),
            usage: self.usage,
            exit_code: self.exit_code,
        }
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/interop/finstack-ai-codex-child
git commit -m "feat: reduce codex events and exit status into run reports"
```

---

### Task 5: Fake Codex binary + `start_or_attach` with attach/conflict semantics

**Files:**
- Create: `extensions/interop/finstack-ai-codex-child/src/bin/codex_fake.rs`
- Modify: `extensions/interop/finstack-ai-codex-child/src/invoker.rs` (full `AgentInvoker` impl, supervisor task, `run_status`)
- Modify: `extensions/interop/finstack-ai-codex-child/Cargo.toml` (ensure the `[[bin]]` block from Task 1 is present)
- Test: `extensions/interop/finstack-ai-codex-child/tests/exec.rs`

**Interfaces:**
- Consumes: `CodexExecConfig`/`CodexSandboxMode` (Task 2), `parse_event` (Task 3), `RunState`/`CodexRunReport`/`CodexRunStatus` (Task 4), `codex_agent_ref`/`codex_route_ref` (Task 1); runtime types `AgentInvoker`, `ChildRunContext`, `ChildRunRequest`, `ChildRunHandle`, `AgentInvokeError`, `child_relation_digest` (see `crates/finstack-ai-runtime/src/services/agent_invoker.rs` and `services/composition.rs:368`).
- Produces: `impl AgentInvoker for CodexChildInvoker` (`start_or_attach` only in this task) and `pub fn run_status(&self, run_id: &RunId) -> Option<CodexRunReport>`. Task 6 adds `cancel`; Task 7 consumes both plus `run_status`.

- [ ] **Step 1: Write the fake binary `src/bin/codex_fake.rs`**

```rust
//! Deterministic Codex CLI stand-in for integration tests.
//!
//! Ignores real Codex flags; the test selects behavior with `--fake-mode
//! <success|fail|hang>` passed through `CodexExecConfig.extra_args`, so
//! parallel tests share no global state.

use std::io::Write;

fn main() {
    let mut mode = String::from("success");
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == "--fake-mode" {
            if let Some(value) = args.next() {
                mode = value;
            }
        }
    }
    let mut stdout = std::io::stdout();
    emit(&mut stdout, r#"{"type":"thread.started","thread_id":"thread-fake-1"}"#);
    match mode.as_str() {
        "fail" => {
            emit(&mut stdout, r#"{"type":"turn.failed","error":{"message":"fake failure"}}"#);
            std::process::exit(1);
        }
        "hang" => {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        _ => {
            emit(
                &mut stdout,
                r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"fake done"}}"#,
            );
            emit(
                &mut stdout,
                r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5}}"#,
            );
        }
    }
}

fn emit(stdout: &mut std::io::Stdout, line: &str) {
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}
```

- [ ] **Step 2: Write the failing integration tests `tests/exec.rs`**

First open `extensions/toolsets/finstack-ai-tools-subagent/src/tests.rs` and `crates/finstack-ai-test/tests/lanes/subagent.rs` and lift their context-construction helpers — how they build an `OperationLocator`, `EffectId`, and `AuthorizationContext` for a `ChildRunContext` (and later a `ToolCallContext`). Reproduce those helpers at the top of `tests/exec.rs` (adapting names, not inventing new APIs). Then:

```rust
use std::sync::Arc;

use finstack_ai_codex_child::{
    codex_agent_ref, codex_route_ref, CodexChildInvoker, CodexExecConfig, CodexRunStatus,
    CodexSandboxMode,
};
use finstack_ai_runtime::{AgentInvokeError, AgentInvoker, ChildRunRequest};
// ... plus the kernel/runtime imports the lifted helpers need.

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

// Build a ChildRunRequest the same way the toolset will (Task 7):
// agent = codex_agent_ref(), placement = RemoteChildSession,
// locator.remote = Some(codex_route_ref()), input = one TextBlock,
// request_digest = Digest::domain_separated("child-run-request", 1, canonical)
// over {agent_id, input, placement: "remote_child_session", run_id}
// — lift `child_locator` and `request_digest` shapes from
// extensions/toolsets/finstack-ai-tools-subagent/src/lib.rs:388-439.
fn codex_request(prompt: &str) -> ChildRunRequest { /* built from the lifted helpers */ }

async fn wait_until_settled(invoker: &CodexChildInvoker, run_id: &finstack_ai_kernel::RunId) -> finstack_ai_codex_child::CodexRunReport {
    for _ in 0..200 {
        if let Some(report) = invoker.run_status(run_id) {
            if report.status != CodexRunStatus::Running {
                return report;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("codex fake run never settled");
}

#[tokio::test]
async fn success_run_completes_with_message_and_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("success", dir.path());
    let request = codex_request("fix the failing test");
    let run_id = request.locator.operation.run_id;
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
    let run_id = request.locator.operation.run_id;
    invoker.start_or_attach(child_context(), request).await.expect("accepted");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Failed);
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

    let mut altered = request;
    altered.request_digest =
        finstack_ai_kernel::Digest::domain_separated("child-run-request", 1, b"different")
            .expect("digest");
    let error = invoker
        .start_or_attach(child_context(), altered)
        .await
        .expect_err("conflict");
    assert!(matches!(error, AgentInvokeError::Conflict { .. }));
}

#[tokio::test]
async fn wrong_placement_is_rejected() {
    // Build a request with ChildPlacement::IsolatedChildSession (remote: None)
    // and assert start_or_attach returns AgentInvokeError::InvalidRequest.
}
```

The `codex_request` / `child_context` helpers must be real code in the final file — the shapes to lift are fully spelled out in `extensions/toolsets/finstack-ai-tools-subagent/src/lib.rs:388-439` (locator + digest) and the runtime tests around `crates/finstack-ai-runtime/src/services/agent_invoker.rs:206` (context construction for `finstack.remote.worker`). If `AuthorizationContext` has no obvious test constructor, copy exactly what `crates/finstack-ai-test/tests/lanes/subagent.rs` does.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p finstack-ai-codex-child --test exec --locked`
Expected: FAIL to compile — no `AgentInvoker` impl / `run_status` on `CodexChildInvoker`.

- [ ] **Step 4: Implement spawn + supervision + `start_or_attach` + `run_status` in `src/invoker.rs`**

```rust
use std::process::Stdio;

use finstack_ai_kernel::{ChildPlacement, ContentBlock, Digest, RunId};
use finstack_ai_runtime::{
    child_relation_digest, AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunHandle,
    ChildRunLocator, ChildRunRequest, PortFuture,
};
use tokio::io::AsyncBufReadExt;

use crate::state::{CodexRunReport, RunState};

fn invalid(message: &'static str) -> AgentInvokeError {
    AgentInvokeError::InvalidRequest { message: Arc::from(message) }
}

fn unavailable(message: &'static str) -> AgentInvokeError {
    AgentInvokeError::Unavailable { message: Arc::from(message) }
}

impl CodexChildInvoker {
    /// Report point-in-time status for an accepted run.
    ///
    /// Returns `None` for runs this process never accepted (including all
    /// runs from before a host restart) — the toolset reports those as
    /// `unknown`.
    #[must_use]
    pub fn run_status(&self, run_id: &RunId) -> Option<CodexRunReport> {
        let runs = self.runs.lock().ok()?;
        let run = runs.get(run_id)?;
        let state = run.state.lock().ok()?;
        Some(state.report())
    }
}

fn prompt_text(input: &[ContentBlock]) -> Result<String, AgentInvokeError> {
    let text = input
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.trim().is_empty() {
        return Err(invalid("codex child input has no text"));
    }
    Ok(text)
}

async fn spawn_codex(
    config: &CodexExecConfig,
    prompt: &str,
) -> Result<(Arc<Mutex<RunState>>, Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>), AgentInvokeError> {
    let mut command = tokio::process::Command::new(&config.binary);
    command
        .arg("exec")
        .arg("--json")
        .arg("--sandbox")
        .arg(config.sandbox.flag())
        .arg("--cd")
        .arg(&config.workspace_root);
    if config.skip_git_repo_check {
        command.arg("--skip-git-repo-check");
    }
    for arg in &config.extra_args {
        command.arg(arg);
    }
    command.arg("--").arg(prompt);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| unavailable("codex binary failed to start"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| unavailable("codex stdout is unavailable"))?;
    let state = Arc::new(Mutex::new(RunState::default()));
    let slot = Arc::new(tokio::sync::Mutex::new(Some(child)));
    tokio::spawn(supervise(stdout, Arc::clone(&state), Arc::clone(&slot)));
    Ok((state, slot))
}

async fn supervise(
    stdout: tokio::process::ChildStdout,
    state: Arc<Mutex<RunState>>,
    slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
) {
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let event = crate::events::parse_event(&line);
        if let Ok(mut guard) = state.lock() {
            guard.apply(event);
        }
    }
    let child = slot.lock().await.take();
    let code = match child {
        Some(mut child) => child.wait().await.ok().and_then(|status| status.code()),
        None => None,
    };
    if let Ok(mut guard) = state.lock() {
        guard.record_exit(code);
    }
}

impl finstack_ai_runtime::PortObject for CodexChildInvoker {} // only if the codebase requires an explicit marker impl; check how RemoteChildInvoker satisfies PortObject and copy it.

impl AgentInvoker for CodexChildInvoker {
    fn start_or_attach(
        &self,
        ctx: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let config = self.config.clone();
        let runs = Arc::clone(&self.runs);
        Box::pin(async move {
            request.validate()?;
            if request.placement != ChildPlacement::RemoteChildSession {
                return Err(invalid("codex child invoker only accepts remote_child_session"));
            }
            if request.locator.remote.is_none() {
                return Err(invalid("codex child locator is missing a route"));
            }
            let run_id = request.locator.operation.run_id;
            {
                let guard = runs.lock().map_err(|_| unavailable("codex run table is poisoned"))?;
                if let Some(existing) = guard.get(&run_id) {
                    if existing.request_digest == request.request_digest {
                        return Ok(existing.handle.clone());
                    }
                    return Err(AgentInvokeError::Conflict {
                        existing: existing.request_digest,
                        submitted: request.request_digest,
                    });
                }
                if guard.len() >= MAX_ACCEPTED {
                    return Err(unavailable("codex run table is full"));
                }
            }
            let prompt = prompt_text(&request.input)?;
            let relation_digest = child_relation_digest(&ctx, &request)?;
            let handle = ChildRunHandle { locator: request.locator.clone(), relation_digest };
            let (state, slot) = spawn_codex(&config, &prompt).await?;
            let run = CodexRun {
                request_digest: request.request_digest,
                handle: handle.clone(),
                state,
                child: slot,
            };
            let mut guard = runs.lock().map_err(|_| unavailable("codex run table is poisoned"))?;
            // Re-check under the write lock: a racing equal request keeps
            // the first insertion (kill_on_drop reaps the duplicate child).
            if let Some(existing) = guard.get(&run_id) {
                if existing.request_digest == request.request_digest {
                    return Ok(existing.handle.clone());
                }
                return Err(AgentInvokeError::Conflict {
                    existing: existing.request_digest,
                    submitted: request.request_digest,
                });
            }
            guard.insert(run_id, run);
            Ok(handle)
        })
    }
}
```

Reality-check while implementing: (a) `child_relation_digest`'s exact error type — mirror how `extensions/interop/finstack-ai-remote-child/src/invoker.rs:67-153` calls it; (b) whether `PortObject` needs an explicit impl or is blanket-implemented — copy remote-child; (c) `RunId: Copy` — if not, `.clone()` where needed. Replace the placeholder `CodexRun` from Task 2 with the full four-field struct.

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test -p finstack-ai-codex-child --locked` (unit + exec integration)
Expected: PASS.

- [ ] **Step 6: Lint and commit**

Run: `cargo clippy -p finstack-ai-codex-child --all-targets --locked -- -D warnings` then

```bash
git add extensions/interop/finstack-ai-codex-child
git commit -m "feat: spawn and supervise codex exec with attach/conflict semantics"
```

---

### Task 6: Cancel

**Files:**
- Modify: `extensions/interop/finstack-ai-codex-child/src/invoker.rs`
- Test: `extensions/interop/finstack-ai-codex-child/tests/exec.rs`

**Interfaces:**
- Consumes: `CodexRun` table, `RunState::mark_cancelled` (Tasks 4–5).
- Produces: `fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>>` on the `AgentInvoker` impl. Task 7's `codex_cancel` consumes it.

- [ ] **Step 1: Write failing cancel tests (append to `tests/exec.rs`)**

```rust
#[tokio::test]
async fn cancel_kills_a_hanging_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("hang", dir.path());
    let request = codex_request("never finish");
    let run_id = request.locator.operation.run_id;
    let handle = invoker.start_or_attach(child_context(), request).await.expect("accepted");
    invoker.cancel(&handle.locator).await.expect("cancelled");
    let report = wait_until_settled(&invoker, &run_id).await;
    assert_eq!(report.status, CodexRunStatus::Cancelled);
}

#[tokio::test]
async fn cancel_of_unknown_locator_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = fake_invoker("success", dir.path());
    let request = codex_request("never started");
    let error = invoker.cancel(&request.locator).await.expect_err("unaccepted");
    assert!(matches!(error, AgentInvokeError::Unavailable { .. }));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-codex-child --test exec --locked`
Expected: `cancel_of_unknown_locator_fails_closed` may pass via the trait's default `Unavailable` — but `cancel_kills_a_hanging_run` FAILS (default `cancel` returns `Unavailable`).

- [ ] **Step 3: Implement `cancel` in the `AgentInvoker` impl**

```rust
    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let runs = Arc::clone(&self.runs);
        let run_id = locator.operation.run_id;
        Box::pin(async move {
            let run = {
                let guard = runs.lock().map_err(|_| unavailable("codex run table is poisoned"))?;
                guard.get(&run_id).cloned()
            };
            let Some(run) = run else {
                return Err(unavailable("codex child locator was never accepted"));
            };
            if let Ok(mut state) = run.state.lock() {
                state.mark_cancelled();
            }
            let mut slot = run.child.lock().await;
            if let Some(child) = slot.as_mut() {
                let _ = child.start_kill();
            }
            Ok(())
        })
    }
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/interop/finstack-ai-codex-child
git commit -m "feat: cancel codex child runs by terminating the process"
```

---

### Task 7: `CodexToolset` — codex_start / codex_status / codex_cancel

**Files:**
- Create: `extensions/interop/finstack-ai-codex-child/src/toolset.rs`
- Modify: `extensions/interop/finstack-ai-codex-child/src/lib.rs` (`mod toolset;` + `pub use toolset::CodexToolset;`)
- Test: `extensions/interop/finstack-ai-codex-child/tests/toolset.rs`

**Interfaces:**
- Consumes: `CodexChildInvoker` (`start_or_attach` via the `AgentInvoker` trait, `cancel`, `run_status`), `codex_agent_ref`, `codex_route_ref`, `CodexRunStatus`/`CodexRunReport`, error consts.
- Produces: `pub struct CodexToolset` with `pub fn try_new(invoker: Arc<CodexChildInvoker>) -> Result<Self, CodexChildError>` implementing `finstack_ai_runtime::Toolset`. Tool ids `finstack.tools.codex.{start,status,cancel}`, model names `codex_start` / `codex_status` / `codex_cancel`, descriptor name `"finstack-codex"`.

Structural template: `extensions/toolsets/finstack-ai-tools-subagent/src/lib.rs` — same `Toolset` impl shape, `verify_authority`, `ChildKey`-keyed table, `tool_spec` helper, `error_result`/`invoke_error_result`/`result_json`/`completed` helpers. Copy those helpers verbatim (lines 456–545), changing only the `SUBAGENT_*` codes to `CODEX_*`, the error type to `CodexChildError`, and the approval reason to `"codex child invocation edits the frozen workspace unless the host already authorized ChildRunPolicy::Allow"`.

- [ ] **Step 1: Write failing toolset tests `tests/toolset.rs`**

Reuse the lifted `ToolCallContext`/`ValidatedToolCall` test helpers (same sources as Task 5 Step 2; the subagent unit tests at `extensions/toolsets/finstack-ai-tools-subagent/src/tests.rs` show `context()` and `call(name, args)` builders).

```rust
#[tokio::test]
async fn start_then_status_reports_completion() {
    let dir = tempfile::tempdir().expect("tempdir");
    let invoker = Arc::new(fake_invoker("success", dir.path()));
    let toolset = CodexToolset::try_new(Arc::clone(&invoker)).expect("toolset");
    let output = call_tool(&toolset, "codex_start", r#"{"prompt":"fix the failing test"}"#).await;
    assert_eq!(output["status"], "accepted");
    let run_id = output["run_id"].as_str().expect("run_id").to_string();

    let settled = poll_status(&toolset, &run_id).await; // loop codex_status until != "running"
    assert_eq!(settled["status"], "completed");
    assert_eq!(settled["thread_id"], "thread-fake-1");
    assert_eq!(settled["last_message"], "fake done");
    assert_eq!(settled["usage"]["output_tokens"], 5);
}

#[tokio::test]
async fn status_of_unknown_run_is_child_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset = CodexToolset::try_new(Arc::new(fake_invoker("success", dir.path()))).expect("toolset");
    let output = call_tool(&toolset, "codex_status", r#"{"run_id":"not-a-run"}"#).await;
    assert_eq!(output["code"], "codex_child_not_found");
}

#[tokio::test]
async fn start_rejects_empty_prompt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset = CodexToolset::try_new(Arc::new(fake_invoker("success", dir.path()))).expect("toolset");
    let output = call_tool(&toolset, "codex_start", r#"{"prompt":"   "}"#).await;
    assert_eq!(output["code"], "codex_invalid_arguments");
}

#[tokio::test]
async fn cancel_stops_a_hanging_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toolset = CodexToolset::try_new(Arc::new(fake_invoker("hang", dir.path()))).expect("toolset");
    let output = call_tool(&toolset, "codex_start", r#"{"prompt":"never finish"}"#).await;
    let run_id = output["run_id"].as_str().expect("run_id").to_string();
    let cancel = call_tool(&toolset, "codex_cancel", &format!(r#"{{"run_id":"{run_id}"}}"#)).await;
    assert_eq!(cancel["cancelled"], true);
    let settled = poll_status(&toolset, &run_id).await;
    assert_eq!(settled["status"], "cancelled");
}
```

`call_tool` drives `Toolset::call` with the lifted context/call builders, collects the single `ToolStreamItem::Completed` from the stream, and parses `result.output` bytes as `serde_json::Value`. `poll_status` loops `codex_status` with 25 ms sleeps, up to 200 tries, until `status != "running"`.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-codex-child --test toolset --locked`
Expected: FAIL to compile — `CodexToolset` not found.

- [ ] **Step 3: Write `src/toolset.rs`**

Key content beyond the copied helpers (full file structure mirrors the subagent crate):

```rust
//! Codex toolset: start / status / cancel over the frozen exec invoker.

const START_ID: &str = "finstack.tools.codex.start";
const STATUS_ID: &str = "finstack.tools.codex.status";
const CANCEL_ID: &str = "finstack.tools.codex.cancel";
const START_NAME: &str = "codex_start";
const STATUS_NAME: &str = "codex_status";
const CANCEL_NAME: &str = "codex_cancel";
const MAX_MESSAGE_BYTES: usize = 4_096;

/// Codex toolset over one frozen [`CodexChildInvoker`]. The prompt is the
/// only model-supplied input; binary, workspace, and sandbox are frozen.
pub struct CodexToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    invoker: Arc<CodexChildInvoker>,
    agent: AgentRef,
    children: Arc<Mutex<BTreeMap<ChildKey, StartedChild>>>,
}

impl CodexToolset {
    /// Construct the toolset over a frozen invoker.
    ///
    /// # Errors
    ///
    /// Returns [`CodexChildError::Configuration`] when a checked-in tool
    /// specification or the peer identity cannot be built.
    pub fn try_new(invoker: Arc<CodexChildInvoker>) -> Result<Self, CodexChildError> {
        let agent = codex_agent_ref()?;
        let tools = Arc::from([
            tool_spec(START_ID, START_NAME,
                "Delegate one coding task to the Codex child agent in the frozen workspace.",
                br#"{"additionalProperties":false,"properties":{"prompt":{"type":"string"}},"required":["prompt"],"type":"object"}"#)?,
            tool_spec(STATUS_ID, STATUS_NAME,
                "Report status for one previously started Codex run without waiting.",
                br#"{"additionalProperties":false,"properties":{"run_id":{"type":"string"}},"required":["run_id"],"type":"object"}"#)?,
            tool_spec(CANCEL_ID, CANCEL_NAME,
                "Cancel a Codex child run started by this toolset.",
                br#"{"additionalProperties":false,"properties":{"run_id":{"type":"string"}},"required":["run_id"],"type":"object"}"#)?,
        ]);
        Ok(Self {
            descriptor: ToolsetDescriptor { name: Arc::from("finstack-codex"), metadata: Metadata::empty() },
            tools, invoker, agent,
            children: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
}
```

`start_child` differences from the subagent template:
- Arguments are `struct StartArguments { prompt: Arc<str> }` (`deny_unknown_fields`); reject a whitespace-only prompt with `CODEX_INVALID_ARGUMENTS`.
- Placement is always `ChildPlacement::RemoteChildSession`; the locator helper takes the route:

```rust
fn child_locator(parent: &OperationLocator, remote: RemoteRouteRef) -> Result<ChildRunLocator, IdGenerationError> {
    let generator = UuidV7Generator::new(SystemClock, OsRandomSource);
    let run_id = generator.generate::<RunTag>()?;
    let lane_id = generator.generate::<LaneTag>()?;
    let session_id = generator.generate::<SessionTag>()?;
    Ok(ChildRunLocator {
        operation: OperationLocator::try_new(parent.tenant_scope.as_ref(), session_id, lane_id, run_id)
            .map_err(|_| IdGenerationError::Source("codex child locator is invalid".into()))?,
        remote: Some(remote),
    })
}
```

- The request digest keeps the subagent canonical shape with the placement fixed:

```rust
fn request_digest(agent: &AgentRef, input: &[ContentBlock], locator: &ChildRunLocator) -> Result<Digest, ToolError> {
    let canonical = serde_json::to_vec(&serde_json::json!({
        "agent_id": agent.id.to_string(),
        "input": input.iter().map(content_text).collect::<Vec<_>>(),
        "placement": "remote_child_session",
        "run_id": locator.operation.run_id.to_string(),
    }))
    .map_err(|_| tool_error(CODEX_INVALID_ARGUMENTS, ErrorCategory::Internal, "codex request digest serialization failed"))?;
    Digest::domain_separated("child-run-request", 1, &canonical)
        .map_err(|_| tool_error(CODEX_INVALID_ARGUMENTS, ErrorCategory::Internal, "codex request digest failed"))
}
```

- Build `ChildRunRequest { agent: agent.clone(), input, placement: RemoteChildSession, locator, requested_deadline: ctx.run.deadline, requested_budget: BudgetRequest::default(), delegation_id: None, metadata: Metadata::empty(), request_digest }`, `request.validate()` → soft error, then `AgentInvoker::start_or_attach(invoker.as_ref(), context, request)` with `ChildRunContext { parent: ctx.run.locator.clone(), parent_effect_id: ctx.run.effect_id, authorization: ctx.run.authorization.clone() }`. Success output: `{"run_id","session_id","status":"accepted"}`.

`status_child` differences:
- Look up the child in the table (tenant-scoped, as subagent's `lookup_child`); miss → `CODEX_CHILD_NOT_FOUND`.
- Then `invoker.run_status(&child.handle.locator.operation.run_id)`:
  - `None` → `{"run_id": ..., "status": "unknown"}` (host restarted since start; not an error).
  - `Some(report)` → `{"run_id", "status": <running|completed|failed|cancelled>, "thread_id"?, "last_message"?, "usage"?, "exit_code"?}` with `last_message` truncated:

```rust
fn truncate_message(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or(text)
}
```

`cancel_child`: identical to the subagent template (`invoker.cancel(&child.handle.locator)`), output `{"run_id","cancelled":true,"placement":"remote_child_session"}`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-codex-child --locked`
Expected: PASS (all unit + both integration files).

- [ ] **Step 5: Lint and commit**

Run: `cargo clippy -p finstack-ai-codex-child --all-targets --locked -- -D warnings` then

```bash
git add extensions/interop/finstack-ai-codex-child
git commit -m "feat: add codex_start/codex_status/codex_cancel toolset"
```

---

### Task 8: Docs, public-API baseline, full verification

**Files:**
- Create: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-codex-child.txt`
- Modify: `docs/site/rust.md` (child-dispatch section)

**Interfaces:**
- Consumes: the finished public surface from Tasks 1–7.

- [ ] **Step 1: Update `docs/site/rust.md`**

In the child-dispatch paragraph that currently ends with "`RemoteChildSession` dispatch stays excluded.", append one sentence:

> `RemoteChildSession` dispatch through the facade stays excluded; external peer products attach through dedicated invokers instead (see `finstack-ai-codex-child`, which runs OpenAI Codex as a child via `codex exec --json` with construction-frozen sandbox flags).

- [ ] **Step 2: Generate the public-API baseline**

Run `uv run --no-project python scripts/compat/public_api.py --help` to find the write/update mode (the check mode is `scripts/compat/public_items.py --check`); regenerate baselines so `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-codex-child.txt` exists. Then verify:

Run: `mise run check-public-api`
Expected: PASS (no "missing baseline" for finstack-ai-codex-child).

- [ ] **Step 3: Full workspace verification**

```bash
mise run check-rust
```

```bash
mise run test-rust
```

Expected: both PASS. Fix anything they surface before committing.

- [ ] **Step 4: Optional manual smoke against the real CLI (not CI)**

If a logged-in `codex` binary is on this machine, verify the frozen flag set and JSONL shapes once: `codex exec --json --sandbox read-only --cd <some-repo> -- "summarize this repo"` and confirm `thread.started` / `item.completed(agent_message)` / `turn.completed` appear. If flag names differ in the installed version, fix `spawn_codex` and the spec table — the parser itself is tolerant.

- [ ] **Step 5: Commit**

```bash
git add docs/site/rust.md fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-codex-child.txt
git commit -m "docs: register codex child peer path and public-api baseline"
```

---

## Future work (not in this plan)

- **v2 — `codex app-server`:** long-lived JSON-RPC, approval bridging into finstack typed interactions, `codex exec resume` reconciliation by stored thread id. Same crate, same toolset verbs.
- **ACP child (Claude Code et al.):** `extensions/interop/finstack-ai-acp-child`, identity `finstack.peer.claude`, ACP JSON-RPC over stdio with `session/request_permission` answered from a frozen construction-time policy — fully mapped in spec §8. Copy this crate's shape; only extract a shared abstraction after both crates exist.
- **Budget mapping:** turn `CodexUsage` into a real charge (`BudgetPropagation::SharedScope` draw) once the host wants enforcement rather than observation.
