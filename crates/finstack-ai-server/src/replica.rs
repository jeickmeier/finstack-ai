//! In-process session replica: single-writer, reconnect plan, idempotency.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use finstack_ai_kernel::Digest;
use finstack_ai_protocol::{
    RemoteCommand, RemoteCommandResult, RemoteEventView, RemoteLocator, RemoteSnapshot,
};

use crate::ServerError;
use crate::auth::AuthContext;

/// Credit window configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditLimits {
    /// Initial item credits.
    pub items: u32,
    /// Initial byte credits.
    pub bytes: u32,
    /// How long the server waits for an ack before disconnecting.
    pub ack_deadline: Duration,
}

impl Default for CreditLimits {
    fn default() -> Self {
        Self {
            items: 8,
            bytes: 64 * 1024,
            ack_deadline: Duration::from_millis(200),
        }
    }
}

/// Mutable credit window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditWindow {
    items: u32,
    bytes: u32,
    limits: CreditLimits,
}

impl CreditWindow {
    /// Open a window with `limits` already granted.
    #[must_use]
    pub const fn new(limits: CreditLimits) -> Self {
        Self {
            items: limits.items,
            bytes: limits.bytes,
            limits,
        }
    }

    /// Remaining item credits.
    #[must_use]
    pub const fn items(&self) -> u32 {
        self.items
    }

    /// Remaining byte credits.
    #[must_use]
    pub const fn bytes(&self) -> u32 {
        self.bytes
    }

    /// Configured limits.
    #[must_use]
    pub const fn limits(&self) -> CreditLimits {
        self.limits
    }

    /// Consume credits for one outbound batch.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::CreditTimeout`] when the window cannot cover the
    /// batch. The caller disconnects and the client resumes from its cursor.
    pub fn consume(&mut self, items: u32, bytes: u32) -> Result<(), ServerError> {
        if self.items < items || self.bytes < bytes {
            return Err(ServerError::CreditTimeout);
        }
        self.items -= items;
        self.bytes -= bytes;
        Ok(())
    }

    /// Restore credits from a client ack.
    pub fn ack(&mut self, items: u32, bytes: u32) {
        self.items = self.items.saturating_add(items).min(self.limits.items);
        self.bytes = self.bytes.saturating_add(bytes).min(self.limits.bytes);
    }
}

/// Authoritative reconnect plan. Live events are not included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectPlan {
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
    receipts: HashMap<(String, Digest), RemoteCommandResult>,
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
            receipts: HashMap::new(),
        }
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
    pub fn take_live(&mut self, n: usize) -> Vec<RemoteEventView> {
        let n = n.min(self.live.len());
        self.live.drain(..n).collect()
    }

    /// Current durable head sequence (`0` when empty).
    #[must_use]
    pub fn head_sequence(&self) -> u64 {
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
    pub fn plan_reconnect(
        &self,
        auth: &AuthContext,
        locator: &RemoteLocator,
        last_known: Option<u64>,
    ) -> Result<ReconnectPlan, ServerError> {
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
        Ok(ReconnectPlan {
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
    pub fn claim_writer(&mut self, connection_id: u64) -> Result<(), ServerError> {
        if self.writer.is_some() {
            return Err(ServerError::SessionBusy);
        }
        self.writer = Some(connection_id);
        self.barrier_released = false;
        Ok(())
    }

    /// Release writer ownership.
    pub fn release_writer(&mut self, connection_id: u64) {
        if self.writer == Some(connection_id) {
            self.writer = None;
        }
    }

    /// Apply or replay a command.
    ///
    /// # Errors
    ///
    /// Conflicting digest reuse fails closed.
    pub fn apply_command(
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
        let key = (command.command_id().to_owned(), command.digest());
        if let Some(existing) = self.receipts.get(&key) {
            return Ok(existing.clone());
        }
        if self
            .receipts
            .keys()
            .any(|(command_id, _)| command_id == command.command_id())
        {
            return Err(ServerError::IdempotencyConflict);
        }
        let result = RemoteCommandResult::new(command.command_id(), command.digest(), true, None);
        self.receipts.insert(key, result.clone());
        Ok(result)
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
