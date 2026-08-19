//! In-process session replica: single-writer, reconnect plan, idempotency.

use std::collections::HashMap;
use std::sync::Mutex;

use finstack_ai_protocol::{
    RemoteCommand, RemoteCommandOp, RemoteCommandResult, RemoteEventView, RemoteLocator,
    RemoteSnapshot,
};

use crate::ServerError;
use crate::auth::AuthContext;

/// Default retained command receipts per replica. New commands fail closed
/// when this ceiling would be exceeded.
pub(crate) const DEFAULT_RECEIPT_CAP: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplicaPhase {
    Idle,
    Running,
    Terminal,
}

/// Authoritative reconnect plan. Live events are not included.
///
/// Produced by [`SessionReplica::plan_reconnect`] and returned by
/// [`crate::RemoteClient::reconnect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectView {
    /// Snapshot at `S`, or `None` for an explicit no-snapshot.
    pub snapshot: Option<RemoteSnapshot>,
    /// Sequence used for `NoSnapshot` when `snapshot` is `None`.
    pub snapshot_sequence: u64,
    /// Durable tail `S+1..=B`.
    pub tail: Vec<RemoteEventView>,
    /// Barrier sequence `B`.
    pub barrier: u64,
}

/// Per-session replica owned by the reference server.
#[derive(Debug)]
pub struct SessionReplica {
    session_id: String,
    tenant_scope: String,
    durable: Vec<RemoteEventView>,
    live: Vec<RemoteEventView>,
    writer: Option<u64>,
    barrier_released: bool,
    phase: ReplicaPhase,
    receipts: HashMap<String, RemoteCommandResult>,
    receipt_cap: usize,
}

impl SessionReplica {
    /// Create an empty replica.
    #[must_use]
    pub fn new(session_id: impl Into<String>, tenant_scope: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            tenant_scope: tenant_scope.into(),
            durable: Vec::new(),
            live: Vec::new(),
            writer: None,
            barrier_released: false,
            phase: ReplicaPhase::Idle,
            receipts: HashMap::new(),
            receipt_cap: DEFAULT_RECEIPT_CAP,
        }
    }

    /// Fail closed when a new command would exceed `cap` retained receipts.
    #[must_use]
    pub fn with_receipt_cap(mut self, cap: usize) -> Self {
        self.receipt_cap = cap;
        self
    }

    /// Append a durable event. Transient previous-connection progress is omitted.
    pub fn append_durable(&mut self, event: RemoteEventView) {
        self.durable.push(event);
    }

    /// Queue a live event. Fails if the sync barrier has not been sent.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::LiveBeforeBarrier`] when called too early.
    pub fn queue_live(&mut self, event: RemoteEventView) -> Result<(), ServerError> {
        if !self.barrier_released {
            return Err(ServerError::LiveBeforeBarrier);
        }
        self.live.push(event);
        Ok(())
    }

    /// Mark the reconnect barrier as sent so live events may follow.
    pub fn release_barrier(&mut self) {
        self.barrier_released = true;
    }

    /// Drain up to `n` queued live events.
    pub(crate) fn take_live(&mut self, n: usize) -> Vec<RemoteEventView> {
        let n = n.min(self.live.len());
        self.live.drain(..n).collect()
    }

    /// Current durable head sequence (`0` when empty).
    #[must_use]
    pub(crate) fn head_sequence(&self) -> u64 {
        self.durable
            .last()
            .and_then(RemoteEventView::durable_sequence)
            .unwrap_or(0)
    }

    /// Build authenticate → open → snapshot → tail → barrier. No live events.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::UnknownLocator`] or [`ServerError::ScopeMismatch`].
    pub(crate) fn plan_reconnect(
        &self,
        auth: &AuthContext,
        locator: &RemoteLocator,
        last_known: Option<u64>,
    ) -> Result<ReconnectView, ServerError> {
        if locator.session_id() != self.session_id {
            return Err(ServerError::UnknownLocator);
        }
        if auth.tenant_scope() != self.tenant_scope {
            return Err(ServerError::UnknownLocator);
        }
        let head = self.head_sequence();
        let last_known = last_known.unwrap_or(0);
        let snapshot = if last_known == 0 && head > 0 {
            Some(RemoteSnapshot::new(
                self.session_id.clone(),
                last_known.max(1).min(head),
            ))
        } else if last_known == 0 {
            None
        } else {
            Some(RemoteSnapshot::new(
                self.session_id.clone(),
                last_known.min(head),
            ))
        };
        let snapshot_sequence = snapshot
            .as_ref()
            .map_or(last_known, RemoteSnapshot::sequence);
        let tail = self
            .durable
            .iter()
            .filter(|event| {
                event
                    .durable_sequence()
                    .is_some_and(|sequence| sequence > snapshot_sequence)
            })
            .cloned()
            .collect::<Vec<_>>();
        let barrier = tail
            .last()
            .and_then(RemoteEventView::durable_sequence)
            .unwrap_or(snapshot_sequence);
        Ok(ReconnectView {
            snapshot,
            snapshot_sequence,
            tail,
            barrier,
        })
    }

    /// Claim exclusive writer ownership.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::SessionBusy`] when another writer is active.
    pub(crate) fn claim_writer(&mut self, connection_id: u64) -> Result<(), ServerError> {
        if self.writer.is_some() {
            return Err(ServerError::SessionBusy);
        }
        self.writer = Some(connection_id);
        self.barrier_released = false;
        Ok(())
    }

    /// Release writer ownership.
    pub(crate) fn release_writer(&mut self, connection_id: u64) {
        if self.writer == Some(connection_id) {
            self.writer = None;
        }
    }

    /// Apply or replay a command.
    ///
    /// Receipts are indexed by `command_id`. Equal digest reuse returns the
    /// original receipt; a conflicting digest fails closed. A new command
    /// that would exceed [`DEFAULT_RECEIPT_CAP`] (or the cap set by
    /// [`SessionReplica::with_receipt_cap`]) fails closed without eviction.
    ///
    /// The replica is not a kernel execution authority. It records `op` as
    /// durable public events (`run_accepted`, `run_cancelled`,
    /// `interaction_resolved`, `run_completed`) and rejects invalid
    /// transitions with `accepted = false`.
    ///
    /// # Errors
    ///
    /// Conflicting digest reuse or a full receipt map fails closed.
    pub(crate) fn apply_command(
        &mut self,
        auth: &AuthContext,
        command: &RemoteCommand,
    ) -> Result<RemoteCommandResult, ServerError> {
        if command.tenant_scope() != auth.tenant_scope()
            || command.tenant_scope() != self.tenant_scope
        {
            return Err(ServerError::ScopeMismatch);
        }
        if command.locator().session_id() != self.session_id {
            return Err(ServerError::UnknownLocator);
        }
        if let Some(existing) = self.receipts.get(command.command_id()) {
            if existing.digest() == command.digest() {
                return Ok(existing.clone());
            }
            return Err(ServerError::IdempotencyConflict);
        }
        if self.receipts.len() >= self.receipt_cap {
            return Err(ServerError::ReceiptCap);
        }
        let (accepted, reason_code) = self.apply_op(command.op());
        let result = RemoteCommandResult::new(
            command.command_id(),
            command.digest(),
            accepted,
            reason_code.map(str::to_owned),
        );
        self.receipts
            .insert(command.command_id().to_owned(), result.clone());
        Ok(result)
    }

    fn apply_op(&mut self, op: RemoteCommandOp) -> (bool, Option<&'static str>) {
        match op {
            RemoteCommandOp::Start => match self.phase {
                ReplicaPhase::Idle => {
                    self.phase = ReplicaPhase::Running;
                    self.append_command_event("run_accepted");
                    (true, None)
                }
                ReplicaPhase::Running => (false, Some("already_started")),
                ReplicaPhase::Terminal => (false, Some("run_terminal")),
            },
            RemoteCommandOp::Cancel => match self.phase {
                ReplicaPhase::Running => {
                    self.phase = ReplicaPhase::Terminal;
                    self.append_command_event("run_cancelled");
                    (true, None)
                }
                ReplicaPhase::Idle => (false, Some("not_running")),
                ReplicaPhase::Terminal => (false, Some("run_terminal")),
            },
            RemoteCommandOp::Resolve => match self.phase {
                ReplicaPhase::Running => {
                    self.append_command_event("interaction_resolved");
                    (true, None)
                }
                ReplicaPhase::Idle => (false, Some("not_running")),
                ReplicaPhase::Terminal => (false, Some("run_terminal")),
            },
            RemoteCommandOp::Complete => match self.phase {
                ReplicaPhase::Running => {
                    self.phase = ReplicaPhase::Terminal;
                    self.append_command_event("run_completed");
                    (true, None)
                }
                ReplicaPhase::Idle => (false, Some("not_running")),
                ReplicaPhase::Terminal => (false, Some("run_terminal")),
            },
        }
    }

    fn append_command_event(&mut self, kind: &str) {
        let sequence = self.head_sequence().saturating_add(1);
        self.durable.push(RemoteEventView::new(
            format!("cmd-{sequence}"),
            kind,
            Some(sequence),
            sequence,
        ));
    }
}

/// Shared hub of replicas.
#[derive(Debug, Default)]
pub struct SessionHub {
    inner: Mutex<HashMap<String, SessionReplica>>,
}

impl SessionHub {
    /// Insert or replace a replica.
    ///
    /// # Panics
    ///
    /// Panics when the hub mutex is poisoned.
    pub fn insert(&self, replica: SessionReplica) {
        self.inner
            .lock()
            .expect("session hub")
            .insert(replica.session_id.clone(), replica);
    }

    /// Borrow a replica by public session id.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::UnknownLocator`] when the session is missing, or
    /// the error returned by `f`.
    ///
    /// # Panics
    ///
    /// Panics when the hub mutex is poisoned.
    pub fn with<R>(
        &self,
        session_id: &str,
        f: impl FnOnce(&mut SessionReplica) -> Result<R, ServerError>,
    ) -> Result<R, ServerError> {
        let mut guard = self.inner.lock().expect("session hub");
        let replica = guard
            .get_mut(session_id)
            .ok_or(ServerError::UnknownLocator)?;
        f(replica)
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthContext, DEFAULT_RECEIPT_CAP, ReplicaPhase, SessionReplica};
    use crate::ServerError;
    use finstack_ai_protocol::{RemoteCommand, RemoteCommandOp, RemoteLocator};

    fn auth() -> AuthContext {
        AuthContext::new("tenant-a", "loopback")
    }

    fn command(id: &str, op: RemoteCommandOp) -> RemoteCommand {
        RemoteCommand::try_new(id, RemoteLocator::new("sess-1", None, None), "tenant-a", op)
            .expect("command")
    }

    #[test]
    fn receipt_map_fails_closed_at_cap() {
        let mut replica = SessionReplica::new("sess-1", "tenant-a").with_receipt_cap(1);
        replica
            .apply_command(&auth(), &command("cmd-1", RemoteCommandOp::Start))
            .expect("first");
        let err = replica
            .apply_command(&auth(), &command("cmd-2", RemoteCommandOp::Complete))
            .expect_err("cap");
        assert!(matches!(err, ServerError::ReceiptCap));
        assert_eq!(err.code(), "receipt_cap");
        let replay = replica
            .apply_command(&auth(), &command("cmd-1", RemoteCommandOp::Start))
            .expect("replay still works at cap");
        assert!(replay.accepted());
    }

    #[test]
    fn receipt_conflict_is_constant_time_lookup() {
        let mut replica = SessionReplica::new("sess-1", "tenant-a");
        replica
            .apply_command(&auth(), &command("cmd-1", RemoteCommandOp::Start))
            .expect("start");
        let err = replica
            .apply_command(&auth(), &command("cmd-1", RemoteCommandOp::Cancel))
            .expect_err("conflict");
        assert!(matches!(err, ServerError::IdempotencyConflict));
        let replay = replica
            .apply_command(&auth(), &command("cmd-1", RemoteCommandOp::Start))
            .expect("replay");
        assert!(replay.accepted());
        assert_eq!(DEFAULT_RECEIPT_CAP, 1024);
    }

    #[test]
    fn apply_command_honors_op_transitions() {
        let mut replica = SessionReplica::new("sess-1", "tenant-a");
        assert_eq!(replica.phase, ReplicaPhase::Idle);
        let start = replica
            .apply_command(&auth(), &command("c1", RemoteCommandOp::Start))
            .expect("start");
        assert!(start.accepted());
        assert_eq!(replica.phase, ReplicaPhase::Running);
        assert_eq!(replica.head_sequence(), 1);
        let start_again = replica
            .apply_command(&auth(), &command("c2", RemoteCommandOp::Start))
            .expect("already started");
        assert!(!start_again.accepted());
        assert_eq!(start_again.reason_code(), Some("already_started"));
        assert_eq!(replica.head_sequence(), 1);
        let resolved = replica
            .apply_command(&auth(), &command("c3", RemoteCommandOp::Resolve))
            .expect("resolve");
        assert!(resolved.accepted());
        assert_eq!(replica.phase, ReplicaPhase::Running);
        let complete = replica
            .apply_command(&auth(), &command("c4", RemoteCommandOp::Complete))
            .expect("complete");
        assert!(complete.accepted());
        assert_eq!(replica.phase, ReplicaPhase::Terminal);
        assert_eq!(replica.head_sequence(), 3);
        let cancel = replica
            .apply_command(&auth(), &command("c5", RemoteCommandOp::Cancel))
            .expect("terminal");
        assert!(!cancel.accepted());
        assert_eq!(cancel.reason_code(), Some("run_terminal"));
        assert_eq!(replica.head_sequence(), 3);
    }

    #[test]
    fn cancel_from_idle_is_rejected() {
        let mut replica = SessionReplica::new("sess-1", "tenant-a");
        let result = replica
            .apply_command(&auth(), &command("c1", RemoteCommandOp::Cancel))
            .expect("receipt");
        assert!(!result.accepted());
        assert_eq!(result.reason_code(), Some("not_running"));
        assert_eq!(replica.head_sequence(), 0);
        assert_eq!(replica.phase, ReplicaPhase::Idle);
    }
}
