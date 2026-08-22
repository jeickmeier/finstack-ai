//! Codex exec child invoker.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ChildPlacement, ChildRunLocator, ContentBlock, RunId};
use finstack_ai_runtime::child::{
    AgentInvokeError, AgentInvoker, ChildRunContext, ChildRunHandle, ChildRunRequest,
    child_relation_digest,
};
use finstack_ai_runtime::ports::PortFuture;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::CodexChildError;
use crate::config::CodexExecConfig;
use crate::identity::{codex_agent_ref, codex_route_ref, configuration};
use crate::state::{CodexRunReport, CodexRunStatus, RunState};

/// Upper bound on accepted runs held in the in-process table.
pub(crate) const MAX_ACCEPTED: usize = 1_024;
const MAX_JSONL_LINE_BYTES: usize = 64 * 1_024;

type ChildSlot = Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>;

/// One accepted child's live process state.
#[derive(Debug, Clone)]
pub(crate) struct CodexRun {
    pub(crate) parent: finstack_ai_kernel::OperationLocator,
    pub(crate) request_digest: finstack_ai_kernel::Digest,
    pub(crate) handle: ChildRunHandle,
    pub(crate) state: Arc<Mutex<RunState>>,
    // Killed by `cancel`. The supervisor task shares this slot and takes
    // the child out of it once stdout reaches EOF, so `kill_on_drop` only
    // fires when both references are gone (host shutdown).
    pub(crate) child: ChildSlot,
}

#[derive(Debug, Clone)]
pub(crate) struct EvictedRun {
    pub(crate) request_digest: finstack_ai_kernel::Digest,
    pub(crate) handle: ChildRunHandle,
    pub(crate) parent: finstack_ai_kernel::OperationLocator,
}

/// `AgentInvoker` leaf that spawns `codex exec --json` with frozen flags.
#[derive(Debug)]
pub struct CodexChildInvoker {
    pub(crate) config: CodexExecConfig,
    pub(crate) runs: Arc<Mutex<BTreeMap<RunId, CodexRun>>>,
    // Tombstones for runs evicted by `make_room`, keyed by run id with the
    // accepted request digest. They keep `start_or_attach` idempotent: a
    // replayed equal request attaches (without a second spawn) and a
    // differing digest still conflicts, even after the run's state was
    // evicted. Bounded to `max_accepted` entries, oldest dropped first.
    pub(crate) evicted: Arc<Mutex<BTreeMap<RunId, EvictedRun>>>,
    pub(crate) max_accepted: usize,
}

impl CodexChildInvoker {
    /// Construct the invoker; fails closed on a missing binary or workspace.
    ///
    /// # Errors
    ///
    /// Returns [`CodexChildError::Configuration`] with reason
    /// `binary_missing`, `workspace_root_missing`, or `extra_args_invalid`.
    pub fn try_new(config: CodexExecConfig) -> Result<Self, CodexChildError> {
        Self::try_new_with_cap(config, MAX_ACCEPTED)
    }

    /// Same as [`CodexChildInvoker::try_new`] with an explicit run-table
    /// capacity. Internal so tests can exercise the eviction path without
    /// spawning 1 024 processes.
    pub(crate) fn try_new_with_cap(
        config: CodexExecConfig,
        max_accepted: usize,
    ) -> Result<Self, CodexChildError> {
        if !config.binary.is_absolute() || !config.binary.is_file() {
            return Err(configuration("binary_missing"));
        }
        if !config.workspace_root.is_absolute() || !config.workspace_root.is_dir() {
            return Err(configuration("workspace_root_missing"));
        }
        if config
            .extra_args
            .iter()
            .any(|arg| arg.as_bytes().contains(&0) || reserved_extra_arg(arg))
        {
            return Err(configuration("extra_args_invalid"));
        }
        Ok(Self {
            config,
            runs: Arc::new(Mutex::new(BTreeMap::new())),
            evicted: Arc::new(Mutex::new(BTreeMap::new())),
            max_accepted,
        })
    }

    /// Report point-in-time status for an accepted run.
    ///
    /// Returns `None` for runs this process never accepted — all runs from
    /// before a host restart, and settled runs evicted from a full table by
    /// [`make_room`] — the toolset reports those as `unknown`.
    #[must_use]
    pub fn run_status(&self, locator: &ChildRunLocator) -> Option<CodexRunReport> {
        let runs = self.runs.lock().ok()?;
        let run = runs.get(&locator.operation.run_id)?;
        if run.handle.locator != *locator {
            return None;
        }
        let state = run.state.lock().ok()?;
        Some(state.report())
    }

    /// Resolve an accepted child owned by the exact parent operation.
    #[must_use]
    pub fn accepted_locator(
        &self,
        parent: &finstack_ai_kernel::OperationLocator,
        run_id: &RunId,
    ) -> Option<ChildRunLocator> {
        if let Ok(runs) = self.runs.lock()
            && let Some(run) = runs.get(run_id)
            && run.parent == *parent
        {
            return Some(run.handle.locator.clone());
        }
        let evicted = self.evicted.lock().ok()?;
        let run = evicted.get(run_id)?;
        (run.parent == *parent).then(|| run.handle.locator.clone())
    }
}

fn reserved_extra_arg(arg: &str) -> bool {
    matches!(
        arg,
        "exec"
            | "--json"
            | "--sandbox"
            | "--cd"
            | "-C"
            | "--skip-git-repo-check"
            | "--dangerously-bypass-approvals-and-sandbox"
            | "--full-auto"
            | "--"
    ) || arg.starts_with("--sandbox=")
        || arg.starts_with("--cd=")
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

/// Make room for one more run, evicting settled entries first.
///
/// Terminal runs (completed, failed, cancelled) are pure history: the
/// process is gone and `run_status` only reports a snapshot, so dropping
/// them costs nothing but the ability to re-read that snapshot — the
/// alternative is a host that permanently answers `Unavailable`.
///
/// A run whose state lock is poisoned is kept: a poisoned lock means we
/// cannot tell whether the child is still alive, and evicting it would
/// drop the only handle `cancel` can use to kill a possibly-running
/// process. Keeping it costs one slot; evicting it could leak a process.
///
/// Every evicted run leaves a tombstone (run id → request digest) so a
/// replayed equal request still attaches instead of spawning a duplicate
/// child; the tombstone map is itself bounded to `cap` entries.
///
/// Called with the run-table lock held; it never awaits.
fn make_room(
    runs: &mut BTreeMap<RunId, CodexRun>,
    evicted: &mut BTreeMap<RunId, EvictedRun>,
    cap: usize,
) -> bool {
    if runs.len() < cap {
        return true;
    }
    runs.retain(|run_id, run| match run.state.lock() {
        Ok(state) => {
            let keep = state.report().status == CodexRunStatus::Running;
            if !keep {
                evicted.insert(
                    *run_id,
                    EvictedRun {
                        request_digest: run.request_digest,
                        handle: run.handle.clone(),
                        parent: run.parent.clone(),
                    },
                );
            }
            keep
        }
        Err(_) => true,
    });
    while evicted.len() > cap {
        evicted.pop_first();
    }
    runs.len() < cap
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
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| unavailable("codex binary failed to start"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| unavailable("codex stdout is unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| unavailable("codex stderr is unavailable"))?;
    let state = Arc::new(Mutex::new(RunState::default()));
    let slot: ChildSlot = Arc::new(tokio::sync::Mutex::new(Some(child)));
    tokio::spawn(supervise(
        stdout,
        stderr,
        Arc::clone(&state),
        Arc::clone(&slot),
    ));
    Ok((state, slot))
}

async fn drain_stdout(stdout: tokio::process::ChildStdout, state: &Mutex<RunState>) {
    let mut reader = tokio::io::BufReader::new(stdout);
    while let Ok(Some((line, oversized))) = read_bounded_line(&mut reader).await {
        let event = if oversized {
            crate::events::CodexEvent::Failed {
                message: "codex emitted an oversized JSONL event".to_owned(),
            }
        } else {
            crate::events::parse_event(&line)
        };
        if let Ok(mut guard) = state.lock() {
            guard.apply(event);
        }
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr, state: &Mutex<RunState>) {
    let mut reader = tokio::io::BufReader::new(stderr);
    while let Ok(Some((line, oversized))) = read_bounded_line(&mut reader).await {
        if let Ok(mut guard) = state.lock() {
            guard.append_stderr(&line);
            if oversized {
                guard.append_stderr("[oversized stderr line drained]");
            }
        }
    }
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<(String, bool)>> {
    let mut retained = Vec::with_capacity(1_024);
    let mut oversized = false;
    let mut saw_bytes = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return if saw_bytes {
                Ok(Some((
                    String::from_utf8_lossy(&retained).into_owned(),
                    oversized,
                )))
            } else {
                Ok(None)
            };
        }
        saw_bytes = true;
        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let chunk = &available[..consumed];
        let content = chunk.strip_suffix(b"\n").unwrap_or(chunk);
        let remaining = MAX_JSONL_LINE_BYTES.saturating_sub(retained.len());
        retained.extend_from_slice(&content[..content.len().min(remaining)]);
        if content.len() > remaining {
            oversized = true;
        }
        let finished = consumed < available.len() || chunk.ends_with(b"\n");
        reader.consume(consumed);
        if finished {
            return Ok(Some((
                String::from_utf8_lossy(&retained).into_owned(),
                oversized,
            )));
        }
    }
}

async fn supervise(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    state: Arc<Mutex<RunState>>,
    slot: ChildSlot,
) {
    // Both pipes are drained concurrently: a child that fills the stderr
    // pipe buffer would otherwise block on write and never finish stdout.
    tokio::join!(
        drain_stdout(stdout, state.as_ref()),
        drain_stderr(stderr, state.as_ref()),
    );
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
        let evicted = Arc::clone(&self.evicted);
        let max_accepted = self.max_accepted;
        Box::pin(async move {
            request.validate()?;
            let expected_agent =
                codex_agent_ref().map_err(|_| invalid("codex peer identity is unavailable"))?;
            if request.agent != expected_agent {
                return Err(invalid("codex child agent identity does not match"));
            }
            if request.placement != ChildPlacement::RemoteChildSession {
                return Err(invalid(
                    "codex child invoker only accepts remote_child_session",
                ));
            }
            let expected_route =
                codex_route_ref().map_err(|_| invalid("codex peer route is unavailable"))?;
            if request.locator.remote.as_ref() != Some(&expected_route) {
                return Err(invalid("codex child locator route does not match"));
            }
            if request.locator.operation.tenant_scope != ctx.parent.tenant_scope {
                return Err(invalid("codex child tenant does not match parent"));
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
                if existing.handle.locator != request.locator || existing.parent != ctx.parent {
                    return Err(invalid("codex run id is bound to a different locator"));
                }
                if existing.request_digest == request.request_digest {
                    return Ok(existing.handle.clone());
                }
                return Err(AgentInvokeError::Conflict {
                    existing: existing.request_digest,
                    submitted: request.request_digest,
                });
            }
            // The evicted-tombstone map is locked strictly after the run
            // table and released before any spawn; neither lock is held
            // across an await.
            let mut evicted_guard = evicted
                .lock()
                .map_err(|_| unavailable("codex eviction table is poisoned"))?;
            if let Some(prior) = evicted_guard.get(&run_id) {
                if prior.handle.locator != request.locator || prior.parent != ctx.parent {
                    return Err(invalid("codex run id is bound to a different locator"));
                }
                if prior.request_digest == request.request_digest {
                    // The run settled and was evicted; the equal replay
                    // attaches to that acceptance instead of spawning a
                    // second child. State is gone, so status is `unknown`.
                    return Ok(handle);
                }
                return Err(AgentInvokeError::Conflict {
                    existing: prior.request_digest,
                    submitted: request.request_digest,
                });
            }
            if !make_room(&mut guard, &mut evicted_guard, max_accepted) {
                return Err(unavailable("codex run table is full"));
            }
            drop(evicted_guard);
            let (state, slot) = spawn_codex(&config, &prompt)?;
            guard.insert(
                run_id,
                CodexRun {
                    parent: ctx.parent,
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
        let locator = locator.clone();
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
            if run.handle.locator != locator {
                return Err(invalid(
                    "codex child locator does not match accepted locator",
                ));
            }
            let mut slot = run.child.lock().await;
            if let Some(child) = slot.as_mut() {
                child
                    .start_kill()
                    .map_err(|_| unavailable("codex child kill request failed"))?;
                if let Ok(mut state) = run.state.lock() {
                    state.mark_cancelled();
                }
                return Ok(());
            }
            let terminal = run
                .state
                .lock()
                .is_ok_and(|state| state.report().status != CodexRunStatus::Running);
            if terminal {
                Ok(())
            } else {
                Err(unavailable("codex child process handle is unavailable"))
            }
        })
    }
}
