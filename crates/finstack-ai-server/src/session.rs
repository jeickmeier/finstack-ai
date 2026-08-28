//! In-process session replica: single-writer, reconnect plan, idempotency.

use std::collections::HashMap;
use std::sync::Mutex;

use finstack_ai_protocol::{
    RemoteCommand, RemoteCommandId, RemoteCommandPayload, RemoteCommandResult, RemoteDurableStep,
    RemoteEventView, RemoteLocator, RemoteSnapshot,
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
/// Produced by `SessionReplica::plan_reconnect` and returned by
/// [`crate::RemoteClient::reconnect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectView {
    /// Snapshot at `S`, or `None` for an explicit no-snapshot.
    pub snapshot: Option<RemoteSnapshot>,
    /// Sequence used for `NoSnapshot` when `snapshot` is `None`.
    pub snapshot_sequence: u64,
    /// Durable tail `S+1..=B`.
    pub tail: Vec<RemoteDurableStep>,
    /// Barrier sequence `B`.
    pub barrier: u64,
}

/// Per-session replica owned by the reference server.
#[derive(Debug)]
pub struct SessionReplica {
    session_id: String,
    tenant_scope: String,
    durable: Vec<RemoteDurableStep>,
    live: Vec<RemoteEventView>,
    last_transient_sequence: Option<u64>,
    writer: Option<u64>,
    barrier_released: bool,
    phase: ReplicaPhase,
    receipts: HashMap<RemoteCommandId, RemoteCommandResult>,
    receipt_cap: usize,
}

impl SessionReplica {
    /// Create an empty replica with validated identity and scope.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::SessionInvalid`] when `session_id` or
    /// `tenant_scope` is not a bounded semantic label.
    pub fn try_new(
        session_id: impl Into<String>,
        tenant_scope: impl Into<String>,
    ) -> Result<Self, ServerError> {
        let session_id = session_id.into();
        let tenant_scope = tenant_scope.into();
        if !finstack_ai_kernel::label_is_valid(&session_id)
            || !finstack_ai_kernel::label_is_valid(&tenant_scope)
        {
            return Err(ServerError::SessionInvalid);
        }
        Ok(Self {
            session_id,
            tenant_scope,
            durable: Vec::new(),
            live: Vec::new(),
            last_transient_sequence: None,
            writer: None,
            barrier_released: false,
            phase: ReplicaPhase::Idle,
            receipts: HashMap::new(),
            receipt_cap: DEFAULT_RECEIPT_CAP,
        })
    }

    /// Fail closed when a new command would exceed `cap` retained receipts.
    #[must_use]
    pub fn with_receipt_cap(mut self, cap: usize) -> Self {
        self.receipt_cap = cap;
        self
    }

    /// Append a durable event. Transient previous-connection progress is omitted.
    pub fn append_durable(&mut self, event: RemoteEventView) {
        let Some(sequence) = event.durable_sequence() else {
            return;
        };
        if self
            .durable
            .last_mut()
            .is_some_and(|step| step.sequence() == sequence)
        {
            let mut events = self
                .durable
                .pop()
                .map_or_else(Vec::new, RemoteDurableStep::into_events);
            events.push(event);
            if let Ok(step) = RemoteDurableStep::try_new(sequence, events) {
                self.durable.push(step);
            }
            return;
        }
        if let Ok(step) = RemoteDurableStep::try_new(sequence, vec![event]) {
            self.durable.push(step);
        }
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
        if event.durable_sequence().is_some()
            || event.event().session_id().to_string() != self.session_id
            || self
                .last_transient_sequence
                .is_some_and(|value| event.transient_sequence() <= value)
        {
            return Err(ServerError::Protocol(
                finstack_ai_protocol::ProtocolError::codec("invalid_live_event"),
            ));
        }
        self.last_transient_sequence = Some(event.transient_sequence());
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
        self.durable.last().map_or(0, RemoteDurableStep::sequence)
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
        if locator.session_id().to_string() != self.session_id {
            return Err(ServerError::UnknownLocator);
        }
        if auth.tenant_scope() != self.tenant_scope {
            return Err(ServerError::ScopeMismatch);
        }
        let head = self.head_sequence();
        let last_known = last_known.unwrap_or(0);
        // This reference replica has no authoritative kernel snapshot source.
        // It must never synthesize a projection that looks authoritative.
        let snapshot = None;
        let snapshot_sequence = last_known.min(head);
        let tail = self
            .durable
            .iter()
            .filter(|step| step.sequence() > snapshot_sequence)
            .cloned()
            .collect::<Vec<_>>();
        let barrier = tail
            .last()
            .map_or(snapshot_sequence, RemoteDurableStep::sequence);
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
    /// The replica is not a kernel execution authority. It validates the
    /// typed command payload and advances only its reference lifecycle; an
    /// authoritative kernel integration is responsible for publishing full
    /// durable events. Invalid transitions return `accepted = false`.
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
        if command.locator().session_id().to_string() != self.session_id {
            return Err(ServerError::UnknownLocator);
        }
        if let Some(existing) = self.receipts.get(&command.command_id()) {
            if existing.digest() == command.digest() {
                return Ok(existing.clone());
            }
            return Err(ServerError::IdempotencyConflict);
        }
        if self.receipts.len() >= self.receipt_cap {
            return Err(ServerError::ReceiptCap);
        }
        if command.expected_durable_sequence() != self.head_sequence() {
            let result = RemoteCommandResult::try_new(
                command.command_id(),
                command.digest(),
                self.head_sequence(),
                false,
                Some("stale_durable_cursor".into()),
            )?;
            self.receipts.insert(command.command_id(), result.clone());
            return Ok(result);
        }
        let (accepted, reason_code) = self.apply_payload(command.payload());
        let result = RemoteCommandResult::try_new(
            command.command_id(),
            command.digest(),
            self.head_sequence(),
            accepted,
            reason_code.map(str::to_owned),
        )?;
        self.receipts.insert(command.command_id(), result.clone());
        Ok(result)
    }

    fn apply_payload(&mut self, payload: &RemoteCommandPayload) -> (bool, Option<&'static str>) {
        match payload {
            RemoteCommandPayload::Start(_) => match self.phase {
                ReplicaPhase::Idle => {
                    self.phase = ReplicaPhase::Running;
                    self.append_command_event("run_accepted");
                    (true, None)
                }
                ReplicaPhase::Running => (false, Some("already_started")),
                ReplicaPhase::Terminal => (false, Some("run_terminal")),
            },
            RemoteCommandPayload::Cancel(_) => match self.phase {
                ReplicaPhase::Running => {
                    self.phase = ReplicaPhase::Terminal;
                    self.append_command_event("run_cancelled");
                    (true, None)
                }
                ReplicaPhase::Idle => (false, Some("not_running")),
                ReplicaPhase::Terminal => (false, Some("run_terminal")),
            },
            RemoteCommandPayload::Resolve(_) => match self.phase {
                ReplicaPhase::Running => {
                    self.append_command_event("interaction_resolved");
                    (true, None)
                }
                ReplicaPhase::Idle => (false, Some("not_running")),
                ReplicaPhase::Terminal => (false, Some("run_terminal")),
            },
            RemoteCommandPayload::Complete(_) => match self.phase {
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
        let _ = kind;
        if let Ok(step) = RemoteDurableStep::try_new(sequence, Vec::new()) {
            self.durable.push(step);
        }
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
    pub fn insert(&self, replica: SessionReplica) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(replica.session_id.clone(), replica);
    }

    /// Borrow a replica by public session id.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::UnknownLocator`] when the session is missing, or
    /// the error returned by `f`.
    ///
    pub fn with<R>(
        &self,
        session_id: &str,
        f: impl FnOnce(&mut SessionReplica) -> Result<R, ServerError>,
    ) -> Result<R, ServerError> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    use finstack_ai_kernel::{
        AcceptRun, BudgetPropagation, CancelRequested, CancellationInitiator,
        CancellationPropagation, DeadlinePropagation, Digest, Metadata, PrincipalPropagation,
        PrincipalRef, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
        RunSecurityContext,
    };
    use finstack_ai_protocol::{
        RemoteAgentRef, RemoteCommand, RemoteCommandPayload, RemoteLocator, RemoteStartRequest,
    };

    const SESSION_ID: &str = "01234567-89ab-7cde-89ab-0123456789ab";
    const LANE_ID: &str = "11234567-89ab-7cde-89ab-0123456789ab";
    const RUN_ID: &str = "21234567-89ab-7cde-89ab-0123456789ab";

    fn locator() -> RemoteLocator {
        RemoteLocator::try_new(
            SESSION_ID.parse().expect("session id"),
            Some(LANE_ID.parse().expect("lane id")),
            Some(RUN_ID.parse().expect("run id")),
        )
        .expect("locator")
    }

    fn auth() -> AuthContext {
        AuthContext::try_new("tenant-a", "loopback").expect("valid auth context")
    }

    fn start_payload() -> RemoteCommandPayload {
        let locator = locator();
        let run_id = locator.run_id().expect("run id");
        let spec_digest = Digest::raw_json(br#"{"agent":"fixture"}"#);
        let accepted = RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("root"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
                "loopback",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            spec_digest,
            None,
        )
        .expect("accepted");
        RemoteCommandPayload::Start(Box::new(
            RemoteStartRequest::try_new(
                AcceptRun {
                    session_id: locator.session_id(),
                    lane_id: locator.lane_id().expect("lane id"),
                    accepted,
                },
                RemoteAgentRef {
                    agent_id: finstack_ai_kernel::AgentId::parse("agent.fixture").expect("agent"),
                    bundle_id: None,
                    spec_digest,
                },
                Vec::new(),
                Metadata::empty(),
                None,
            )
            .expect("start"),
        ))
    }

    fn cancel_payload() -> RemoteCommandPayload {
        RemoteCommandPayload::Cancel(Box::new(CancelRequested {
            initiator: CancellationInitiator::RuntimeShutdown,
            reason: None,
        }))
    }

    fn command(id: &str, expected: u64, payload: RemoteCommandPayload) -> RemoteCommand {
        let ordinal = id.bytes().fold(0_u64, |value, byte| {
            value.wrapping_mul(31) + u64::from(byte)
        });
        let id = format!("0192e0f6-7c3a-7c11-8a4d-{ordinal:012x}");
        RemoteCommand::try_new(id, locator(), "tenant-a", expected, payload).expect("command")
    }

    #[test]
    fn replica_rejects_empty_or_malformed_identity() {
        for (session, tenant) in [
            ("", "tenant-a"),
            (SESSION_ID, ""),
            ("session\0id", "tenant-a"),
            (SESSION_ID, "tenant\0a"),
        ] {
            let error = SessionReplica::try_new(session, tenant).expect_err("invalid replica");
            assert!(matches!(error, ServerError::SessionInvalid));
            assert_eq!(error.code(), "session_invalid");
        }
    }

    #[test]
    fn receipt_map_fails_closed_at_cap() {
        let mut replica = SessionReplica::try_new(SESSION_ID, "tenant-a")
            .expect("valid session replica")
            .with_receipt_cap(1);
        replica
            .apply_command(&auth(), &command("cmd-1", 0, start_payload()))
            .expect("first");
        let err = replica
            .apply_command(&auth(), &command("cmd-2", 1, cancel_payload()))
            .expect_err("cap");
        assert!(matches!(err, ServerError::ReceiptCap));
        assert_eq!(err.code(), "receipt_cap");
        let replay = replica
            .apply_command(&auth(), &command("cmd-1", 0, start_payload()))
            .expect("replay still works at cap");
        assert!(replay.accepted());
    }

    #[test]
    fn receipt_conflict_is_constant_time_lookup() {
        let mut replica =
            SessionReplica::try_new(SESSION_ID, "tenant-a").expect("valid session replica");
        replica
            .apply_command(&auth(), &command("cmd-1", 0, start_payload()))
            .expect("start");
        let err = replica
            .apply_command(&auth(), &command("cmd-1", 1, cancel_payload()))
            .expect_err("conflict");
        assert!(matches!(err, ServerError::IdempotencyConflict));
        let replay = replica
            .apply_command(&auth(), &command("cmd-1", 0, start_payload()))
            .expect("replay");
        assert!(replay.accepted());
        assert_eq!(DEFAULT_RECEIPT_CAP, 1024);
    }

    #[test]
    fn apply_command_honors_payload_transitions() {
        let mut replica =
            SessionReplica::try_new(SESSION_ID, "tenant-a").expect("valid session replica");
        assert_eq!(replica.phase, ReplicaPhase::Idle);
        let start = replica
            .apply_command(&auth(), &command("c1", 0, start_payload()))
            .expect("start");
        assert!(start.accepted());
        assert_eq!(replica.phase, ReplicaPhase::Running);
        assert_eq!(replica.head_sequence(), 1);
        let start_again = replica
            .apply_command(&auth(), &command("c2", 1, start_payload()))
            .expect("already started");
        assert!(!start_again.accepted());
        assert_eq!(start_again.reason_code(), Some("already_started"));
        assert_eq!(replica.head_sequence(), 1);
        let cancel = replica
            .apply_command(&auth(), &command("c3", 1, cancel_payload()))
            .expect("cancel");
        assert!(cancel.accepted());
        assert_eq!(replica.phase, ReplicaPhase::Terminal);
        assert_eq!(replica.head_sequence(), 2);
        let cancel_again = replica
            .apply_command(&auth(), &command("c4", 2, cancel_payload()))
            .expect("terminal");
        assert!(!cancel_again.accepted());
        assert_eq!(cancel_again.reason_code(), Some("run_terminal"));
        assert_eq!(replica.head_sequence(), 2);
    }

    #[test]
    fn cancel_from_idle_is_rejected() {
        let mut replica =
            SessionReplica::try_new(SESSION_ID, "tenant-a").expect("valid session replica");
        let result = replica
            .apply_command(&auth(), &command("c1", 0, cancel_payload()))
            .expect("receipt");
        assert!(!result.accepted());
        assert_eq!(result.reason_code(), Some("not_running"));
        assert_eq!(replica.head_sequence(), 0);
        assert_eq!(replica.phase, ReplicaPhase::Idle);
    }
}
