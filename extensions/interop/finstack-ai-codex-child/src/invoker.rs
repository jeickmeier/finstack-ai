//! Codex exec child invoker.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ChildPlacement, ChildRunLocator, ContentBlock, RunId};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunHandle, ChildRunRequest, PortFuture,
    child_relation_digest,
};
use tokio::io::AsyncBufReadExt;

use crate::CodexChildError;
use crate::config::CodexExecConfig;
use crate::identity::configuration;
use crate::state::{CodexRunReport, RunState};

/// Upper bound on accepted runs held in the in-process table.
const MAX_ACCEPTED: usize = 1_024;

type ChildSlot = Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>;

/// One accepted child's live process state.
#[derive(Debug, Clone)]
pub(crate) struct CodexRun {
    request_digest: finstack_ai_kernel::Digest,
    handle: ChildRunHandle,
    state: Arc<Mutex<RunState>>,
    // Killed by `cancel`. The supervisor task shares this slot and takes
    // the child out of it once stdout reaches EOF, so `kill_on_drop` only
    // fires when both references are gone (host shutdown).
    child: ChildSlot,
}

/// `AgentInvoker` leaf that spawns `codex exec --json` with frozen flags.
#[derive(Debug)]
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
        if config
            .extra_args
            .iter()
            .any(|arg| arg.as_bytes().contains(&0))
        {
            return Err(configuration("extra_args_invalid"));
        }
        Ok(Self {
            config,
            runs: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

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

fn invalid(message: &'static str) -> AgentInvokeError {
    AgentInvokeError::InvalidRequest {
        message: Arc::from(message),
    }
}

fn unavailable(message: &'static str) -> AgentInvokeError {
    AgentInvokeError::Unavailable {
        message: Arc::from(message),
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

fn spawn_codex(
    config: &CodexExecConfig,
    prompt: &str,
) -> Result<(Arc<Mutex<RunState>>, ChildSlot), AgentInvokeError> {
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
    let slot: ChildSlot = Arc::new(tokio::sync::Mutex::new(Some(child)));
    tokio::spawn(supervise(stdout, Arc::clone(&state), Arc::clone(&slot)));
    Ok((state, slot))
}

async fn supervise(
    stdout: tokio::process::ChildStdout,
    state: Arc<Mutex<RunState>>,
    slot: ChildSlot,
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
                return Err(invalid(
                    "codex child invoker only accepts remote_child_session",
                ));
            }
            if request.locator.remote.is_none() {
                return Err(invalid("codex child locator is missing a route"));
            }
            let run_id = request.locator.operation.run_id;
            let prompt = prompt_text(&request.input)?;
            let relation_digest = child_relation_digest(&ctx, &request).map_err(|error| {
                AgentInvokeError::InvalidRequest {
                    message: Arc::from(error.to_string()),
                }
            })?;
            let handle = ChildRunHandle {
                locator: request.locator.clone(),
                relation_digest,
            };
            // Attach check, spawn, and insert share one critical section.
            // `spawn_codex` never awaits, so holding the run-table lock
            // across it keeps the future `Send` and makes a racing equal
            // request wait rather than spawn a duplicate child that would
            // then have to be killed (the supervisor task holds a second
            // strong reference to the child, so dropping the losing
            // `CodexRun` would not run `kill_on_drop`).
            let mut guard = runs
                .lock()
                .map_err(|_| unavailable("codex run table is poisoned"))?;
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
            let (state, slot) = spawn_codex(&config, &prompt)?;
            guard.insert(
                run_id,
                CodexRun {
                    request_digest: request.request_digest,
                    handle: handle.clone(),
                    state,
                    child: slot,
                },
            );
            drop(guard);
            Ok(handle)
        })
    }

    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let runs = Arc::clone(&self.runs);
        let run_id = locator.operation.run_id;
        Box::pin(async move {
            let run = {
                let guard = runs
                    .lock()
                    .map_err(|_| unavailable("codex run table is poisoned"))?;
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
}
