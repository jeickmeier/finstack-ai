//! In-process session writer, lane guard, and lineage fan-out (PR-047).
//!
//! This is composition over the existing journal store. It is not a seventh
//! port and does not add `KernelState` fields.

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex, Weak};

use finstack_ai_kernel::{
    AppendBatchId, CancellationInitiator, ChildPlacement, ConversationEntry, ConversationError,
    DeadlinePropagation, EntryId, KernelInput, LABEL_MAX_BYTES, LaneCreated, LaneId, LaneMoved,
    Message, Metadata, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft,
    RecordId, RunAccepted, RunId, SessionCreated, SessionId, SessionProjection, Timestamp,
    TransitionEnv,
};
use thiserror::Error;

use crate::coordinator::{CommitCoordinator, CommitCoordinatorError, project_loaded};
use crate::identity_map::{ExternalIdentityKey, ExternalIdentityMap, IdentityMapError};
use crate::journal::{JournalStore, LoadRequest};

/// In-process owner of one `(session_id, lane_id)` guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneOwner {
    /// Create, navigate, or conversation mutation in flight.
    Structural,
    /// One accepted non-terminal run.
    Active(RunId),
}

/// Inspect snapshot for one lane.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneInspect {
    /// Durable lane identity.
    pub lane_id: LaneId,
    /// Stable application name.
    pub name: Arc<str>,
    /// Current leaf, when any conversation entry exists.
    pub leaf_id: Option<EntryId>,
    /// Non-terminal operation on this lane, when busy.
    pub active_run_id: Option<RunId>,
    /// History ending at the current leaf.
    pub history: Vec<ConversationEntry>,
}

/// Identities required to create a session and its mandatory `main` lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCreateIds {
    /// New session identity.
    pub session_id: SessionId,
    /// Mandatory `main` lane identity.
    pub main_lane_id: LaneId,
    /// `SessionCreated` record identity.
    pub session_created_record_id: RecordId,
    /// `LaneCreated` record identity.
    pub lane_created_record_id: RecordId,
    /// Append batch identity.
    pub batch_id: AppendBatchId,
    /// Commit timestamp.
    pub now: Timestamp,
}

/// Identities required to create one additional lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneCreateIds {
    /// New lane identity.
    pub lane_id: LaneId,
    /// `LaneCreated` record identity.
    pub lane_created_record_id: RecordId,
    /// Optional `LaneMoved` record identity when forking.
    pub lane_moved_record_id: Option<RecordId>,
    /// Append batch identity.
    pub batch_id: AppendBatchId,
    /// Commit timestamp.
    pub now: Timestamp,
}

/// Identities required to append one conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneAppendIds {
    /// `ConversationEntry` record identity.
    pub entry_record_id: RecordId,
    /// `LaneMoved` record identity.
    pub lane_moved_record_id: RecordId,
    /// Append batch identity.
    pub batch_id: AppendBatchId,
}

/// Session-writer failures that fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionError {
    /// The requested session cannot be loaded.
    #[error("session recover failed: {code}")]
    Recover {
        /// Stable fault code.
        code: &'static str,
    },
    /// A structural or run commit failed.
    #[error("session commit failed: {code}")]
    Commit {
        /// Stable fault code.
        code: &'static str,
    },
    /// The named or identified lane does not exist.
    #[error("unknown lane")]
    UnknownLane,
    /// The fork or navigate target is not in the conversation tree.
    #[error("unknown entry")]
    UnknownEntry,
    /// A second owner tried to take `(session_id, lane_id)`.
    #[error("lane is busy")]
    LaneBusy,
    /// Lane name is empty, oversized, or a second `main`.
    #[error("invalid lane name")]
    InvalidLaneName,
    /// Another lane already uses this application name.
    #[error("duplicate lane name")]
    DuplicateLaneName,
    /// Shared session state is poisoned.
    #[error("session lock is poisoned")]
    Poisoned,
    /// Host identity map rejected the bind.
    #[error("identity map: {0}")]
    Identity(IdentityMapError),
    /// Conversation projection rejected the mutation.
    #[error("conversation: {0}")]
    Conversation(ConversationError),
}

impl SessionError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Recover { code } | Self::Commit { code } => code,
            Self::UnknownLane => "unknown_lane",
            Self::UnknownEntry => "unknown_entry",
            Self::LaneBusy => "lane_busy",
            Self::InvalidLaneName => "invalid_lane_name",
            Self::DuplicateLaneName => "duplicate_lane_name",
            Self::Poisoned => "session_lock_poisoned",
            Self::Identity(IdentityMapError::Conflict) => "identity_conflict",
            Self::Identity(IdentityMapError::InvalidKey { .. }) => "invalid_identity_key",
            Self::Conversation(_) => "conversation_invalid",
        }
    }

    fn recover(error: &CommitCoordinatorError) -> Self {
        Self::Recover {
            code: commit_code(error),
        }
    }

    fn commit(error: &CommitCoordinatorError) -> Self {
        Self::Commit {
            code: commit_code(error),
        }
    }
}

fn commit_code(error: &CommitCoordinatorError) -> &'static str {
    match error {
        CommitCoordinatorError::Decision { code }
        | CommitCoordinatorError::Faulted { code }
        | CommitCoordinatorError::BoundaryFault { code }
        | CommitCoordinatorError::EventDelivery { code } => code,
        CommitCoordinatorError::SidecarConflict => "sidecar_conflict",
        CommitCoordinatorError::AppendBatchIdCardinality => "append_batch_id_cardinality",
        CommitCoordinatorError::Store(_) => "store_failure",
        CommitCoordinatorError::ModelRequest { .. } => "model_request",
    }
}

struct SessionInner {
    projection: SessionProjection,
    guards: BTreeMap<LaneId, LaneOwner>,
}

/// Shared in-process writer for one session journal.
pub struct SessionRuntime {
    store: Arc<dyn JournalStore>,
    session_id: SessionId,
    tenant_scope: Arc<str>,
    inner: Mutex<SessionInner>,
}

impl SessionRuntime {
    /// Create a session and persist `SessionCreated` plus `LaneCreated("main")`.
    ///
    /// # Errors
    ///
    /// Returns a recover or commit failure when the journal cannot be written.
    pub async fn create(
        store: Arc<dyn JournalStore>,
        tenant_scope: impl Into<Arc<str>>,
        ids: SessionCreateIds,
    ) -> Result<Arc<Self>, SessionError> {
        let tenant_scope = tenant_scope.into();
        if let Some(existing) = interned(&store, ids.session_id) {
            return Ok(existing);
        }
        let loaded = store
            .load(LoadRequest {
                session_id: ids.session_id,
            })
            .await
            .map_err(|_| SessionError::Recover {
                code: "session_load_failed",
            })?;
        if loaded.head_sequence > 0 {
            return Self::open(store, ids.session_id, tenant_scope).await;
        }
        let mut coordinator = CommitCoordinator::new(Arc::clone(&store));
        coordinator
            .commit_session_records(
                ids.batch_id,
                vec![
                    session_draft(
                        ids.session_created_record_id,
                        ids.session_id,
                        ids.main_lane_id,
                        ids.now,
                        RecordBody::SessionCreated(SessionCreated::new(Metadata::empty())),
                    )?,
                    session_draft(
                        ids.lane_created_record_id,
                        ids.session_id,
                        ids.main_lane_id,
                        ids.now,
                        RecordBody::LaneCreated(
                            LaneCreated::try_new("main")
                                .map_err(|_| SessionError::InvalidLaneName)?,
                        ),
                    )?,
                ],
            )
            .await
            .map_err(|error| SessionError::commit(&error))?;
        Ok(intern(Self {
            store,
            session_id: ids.session_id,
            tenant_scope,
            inner: Mutex::new(SessionInner {
                projection: coordinator.session().clone(),
                guards: BTreeMap::new(),
            }),
        }))
    }

    /// Rebuild the projection without respawning non-terminal runs.
    ///
    /// Pre-046 journals without `SessionCreated` still open.
    ///
    /// # Errors
    ///
    /// Returns a recover failure when the journal cannot be loaded or projected.
    pub async fn open(
        store: Arc<dyn JournalStore>,
        session_id: SessionId,
        tenant_scope: impl Into<Arc<str>>,
    ) -> Result<Arc<Self>, SessionError> {
        if let Some(existing) = interned(&store, session_id) {
            return Ok(existing);
        }
        let loaded =
            store
                .load(LoadRequest { session_id })
                .await
                .map_err(|_| SessionError::Recover {
                    code: "session_load_failed",
                })?;
        let projection = project_loaded(&loaded).map_err(|code| SessionError::Recover { code })?;
        Ok(intern(Self {
            store,
            session_id,
            tenant_scope: tenant_scope.into(),
            inner: Mutex::new(SessionInner {
                projection,
                guards: BTreeMap::new(),
            }),
        }))
    }

    /// Return the interned writer for this store and session, when present.
    #[must_use]
    pub fn existing(store: &Arc<dyn JournalStore>, session_id: SessionId) -> Option<Arc<Self>> {
        interned(store, session_id)
    }

    /// Durable session identity.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Tenant scope captured by the host. Never grants authority.
    #[must_use]
    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    /// Journal store shared by every lane in this session.
    #[must_use]
    pub fn store(&self) -> Arc<dyn JournalStore> {
        Arc::clone(&self.store)
    }

    /// Clone the current in-process projection.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Poisoned`] when the lock is poisoned.
    pub fn projection(&self) -> Result<SessionProjection, SessionError> {
        Ok(self.lock()?.projection.clone())
    }

    /// Refresh the projection from the journal without taking a lane guard.
    ///
    /// # Errors
    ///
    /// Returns a recover failure when load or projection fails.
    pub async fn refresh(&self) -> Result<SessionProjection, SessionError> {
        let loaded = self
            .store
            .load(LoadRequest {
                session_id: self.session_id,
            })
            .await
            .map_err(|_| SessionError::Recover {
                code: "session_load_failed",
            })?;
        let projection = project_loaded(&loaded).map_err(|code| SessionError::Recover { code })?;
        self.lock()?.projection = projection.clone();
        Ok(projection)
    }

    /// Create a named lane, optionally forking from an existing entry.
    ///
    /// # Errors
    ///
    /// Returns a configuration or commit failure for duplicate names, a second
    /// `main`, an unknown fork target, or a store failure.
    pub async fn create_lane(
        &self,
        name: impl Into<Arc<str>>,
        fork: Option<EntryId>,
        ids: LaneCreateIds,
    ) -> Result<LaneId, SessionError> {
        let name = name.into();
        let created =
            LaneCreated::try_new(Arc::clone(&name)).map_err(|_| SessionError::InvalidLaneName)?;
        if name.len() > LABEL_MAX_BYTES {
            return Err(SessionError::InvalidLaneName);
        }
        {
            let inner = self.lock()?;
            if name.as_ref() == "main" {
                return Err(SessionError::InvalidLaneName);
            }
            if inner.projection.lane(name.as_ref()).is_some() {
                return Err(SessionError::DuplicateLaneName);
            }
            if let Some(entry_id) = fork
                && !inner.projection.entries().contains_key(&entry_id)
            {
                return Err(SessionError::UnknownEntry);
            }
        }
        self.acquire(ids.lane_id, LaneOwner::Structural)?;
        let result = self.commit_lane_created(created, fork, ids).await;
        self.release(ids.lane_id);
        result?;
        Ok(ids.lane_id)
    }

    /// Point one idle lane at an existing entry without copying.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::LaneBusy`], [`SessionError::UnknownLane`],
    /// [`SessionError::UnknownEntry`], or a commit failure.
    pub async fn navigate(
        &self,
        lane_id: LaneId,
        entry_id: EntryId,
        record_id: RecordId,
        batch_id: AppendBatchId,
        now: Timestamp,
    ) -> Result<(), SessionError> {
        {
            let inner = self.lock()?;
            if inner.projection.lane_by_id(lane_id).is_none() {
                return Err(SessionError::UnknownLane);
            }
            if !inner.projection.entries().contains_key(&entry_id) {
                return Err(SessionError::UnknownEntry);
            }
            if inner.projection.active_on_lane(lane_id).is_some() {
                return Err(SessionError::LaneBusy);
            }
        }
        self.acquire(lane_id, LaneOwner::Structural)?;
        let result = self
            .commit_records(
                batch_id,
                vec![session_draft(
                    record_id,
                    self.session_id,
                    lane_id,
                    now,
                    RecordBody::LaneMoved(LaneMoved::new(entry_id)),
                )?],
            )
            .await;
        self.release(lane_id);
        result
    }

    /// Append one message on an idle lane and move that lane's leaf.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane, unknown-lane, or commit failure.
    pub async fn append_message(
        &self,
        lane_id: LaneId,
        message: &Message,
        ids: LaneAppendIds,
    ) -> Result<EntryId, SessionError> {
        let parent_id = {
            let inner = self.lock()?;
            if inner.projection.lane_by_id(lane_id).is_none() {
                return Err(SessionError::UnknownLane);
            }
            if inner.projection.active_on_lane(lane_id).is_some() {
                return Err(SessionError::LaneBusy);
            }
            inner
                .projection
                .lane_by_id(lane_id)
                .and_then(|lane| lane.leaf_id)
        };
        self.acquire(lane_id, LaneOwner::Structural)?;
        let entry = ConversationEntry::from_message(message, parent_id, lane_id, 0)
            .map_err(SessionError::Conversation)?;
        let entry_id = entry.id();
        let result = self
            .commit_records(
                ids.batch_id,
                vec![
                    session_draft(
                        ids.entry_record_id,
                        self.session_id,
                        lane_id,
                        message.created_at(),
                        RecordBody::ConversationEntry(entry),
                    )?,
                    session_draft(
                        ids.lane_moved_record_id,
                        self.session_id,
                        lane_id,
                        message.created_at(),
                        RecordBody::LaneMoved(LaneMoved::new(entry_id)),
                    )?,
                ],
            )
            .await;
        self.release(lane_id);
        result?;
        Ok(entry_id)
    }

    /// Inspect one lane after refreshing the projection.
    ///
    /// History may include an open tool-call pair while a run is awaiting
    /// tools or interaction. Model-facing [`SessionProjection::history`]
    /// still rejects that incomplete pair.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownLane`] or a recover/history failure.
    pub async fn inspect(&self, lane_id: LaneId) -> Result<LaneInspect, SessionError> {
        let projection = self.refresh().await?;
        let lane = projection
            .lane_by_id(lane_id)
            .ok_or(SessionError::UnknownLane)?;
        let history = match lane.leaf_id {
            Some(leaf) => projection.walk(leaf).map_err(SessionError::Conversation)?,
            None => Vec::new(),
        };
        Ok(LaneInspect {
            lane_id,
            name: Arc::clone(&lane.name),
            leaf_id: lane.leaf_id,
            active_run_id: projection.active_on_lane(lane_id),
            history,
        })
    }

    /// Take the in-process lane guard for a new run.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::LaneBusy`] when the lane already has an owner or
    /// a non-terminal operation.
    pub fn try_acquire_run(&self, lane_id: LaneId, run_id: RunId) -> Result<(), SessionError> {
        let inner = self.lock()?;
        if inner.projection.active_on_lane(lane_id).is_some() || inner.guards.contains_key(&lane_id)
        {
            return Err(SessionError::LaneBusy);
        }
        drop(inner);
        self.acquire(lane_id, LaneOwner::Active(run_id))
    }

    /// Release a previously acquired lane guard.
    pub fn release(&self, lane_id: LaneId) {
        if let Ok(mut inner) = self.lock() {
            inner.guards.remove(&lane_id);
        }
    }

    /// Whether this process currently owns `(session_id, lane_id)`.
    #[must_use]
    pub fn is_guarded(&self, lane_id: LaneId) -> bool {
        self.lock()
            .ok()
            .is_some_and(|inner| inner.guards.contains_key(&lane_id))
    }

    /// Recover a one-run coordinator at the session head.
    ///
    /// # Errors
    ///
    /// Returns a recover failure when replay cannot be completed.
    pub async fn coordinator_for_run(
        &self,
        run_id: Option<RunId>,
    ) -> Result<CommitCoordinator, SessionError> {
        CommitCoordinator::recover_run(Arc::clone(&self.store), self.session_id, run_id)
            .await
            .map_err(|error| SessionError::recover(&error))
    }

    /// Accept one root or child run on an already-guarded lane.
    ///
    /// # Errors
    ///
    /// Returns a recover or decision failure. The caller keeps the Active guard.
    pub async fn accept_run(
        &self,
        lane_id: LaneId,
        accepted: RunAccepted,
        env: TransitionEnv,
    ) -> Result<CommitCoordinator, SessionError> {
        let run_id = accepted.run_id();
        let mut coordinator = self.coordinator_for_run(Some(run_id)).await?;
        coordinator
            .submit(
                env,
                KernelInput::AcceptRun(finstack_ai_kernel::AcceptRun {
                    session_id: self.session_id,
                    lane_id,
                    accepted,
                }),
            )
            .await
            .map_err(|error| SessionError::commit(&error))?;
        self.lock()?.projection = coordinator.session().clone();
        Ok(coordinator)
    }

    /// Cancel one run and fan out through restored child mappings.
    ///
    /// Isolated children are opened in the same store. Remote children are not
    /// dispatched. Detach-preauthorized children stay up because the child
    /// kernel rejects `ParentRun`.
    ///
    /// # Errors
    ///
    /// Returns a recover or commit failure. Unauthorized detach rejects are
    /// ignored so the child remains running.
    pub async fn cancel_run(
        &self,
        run_id: RunId,
        initiator: CancellationInitiator,
        next_env: &mut dyn FnMut() -> Result<TransitionEnv, SessionError>,
    ) -> Result<(), SessionError> {
        let projection = self.refresh().await?;
        if let Some(operation) = projection.operations().get(&run_id)
            && !operation.terminal
        {
            let mut coordinator = self.coordinator_for_run(Some(run_id)).await?;
            match coordinator
                .submit(
                    next_env()?,
                    KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
                        initiator: initiator.clone(),
                        reason: None,
                    }),
                )
                .await
            {
                Ok(_)
                | Err(CommitCoordinatorError::Decision {
                    code: "invalid_input_payload",
                }) => {}
                Err(error) => return Err(SessionError::commit(&error)),
            }
            self.lock()?.projection = coordinator.session().clone();
        }
        self.fan_out(run_id, &initiator, next_env).await
    }

    /// Bind this session's lane into a host-owned identity map.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownLane`] or an identity-map failure.
    pub fn bind_external_identity(
        &self,
        map: &dyn ExternalIdentityMap,
        key: ExternalIdentityKey,
        lane_id: LaneId,
    ) -> Result<(), SessionError> {
        if self.lock()?.projection.lane_by_id(lane_id).is_none() {
            return Err(SessionError::UnknownLane);
        }
        map.bind(key, self.session_id, lane_id)
            .map_err(SessionError::Identity)
    }

    async fn commit_lane_created(
        &self,
        created: LaneCreated,
        fork: Option<EntryId>,
        ids: LaneCreateIds,
    ) -> Result<(), SessionError> {
        let mut records = vec![session_draft(
            ids.lane_created_record_id,
            self.session_id,
            ids.lane_id,
            ids.now,
            RecordBody::LaneCreated(created),
        )?];
        if let Some(entry_id) = fork {
            let moved_id = ids.lane_moved_record_id.ok_or(SessionError::Commit {
                code: "lane_moved_record_id_missing",
            })?;
            records.push(session_draft(
                moved_id,
                self.session_id,
                ids.lane_id,
                ids.now,
                RecordBody::LaneMoved(LaneMoved::new(entry_id)),
            )?);
        }
        self.commit_records(ids.batch_id, records).await
    }

    async fn commit_records(
        &self,
        batch_id: AppendBatchId,
        records: Vec<RecordDraft>,
    ) -> Result<(), SessionError> {
        let mut coordinator = self.coordinator_for_run(None).await?;
        coordinator
            .commit_session_records(batch_id, records)
            .await
            .map_err(|error| SessionError::commit(&error))?;
        self.lock()?.projection = coordinator.session().clone();
        Ok(())
    }

    async fn fan_out(
        &self,
        parent_run_id: RunId,
        parent_initiator: &CancellationInitiator,
        next_env: &mut dyn FnMut() -> Result<TransitionEnv, SessionError>,
    ) -> Result<(), SessionError> {
        let projection = self.projection()?;
        let children: Vec<_> = projection
            .child_mappings()
            .iter()
            .filter(|((run_id, _), _)| *run_id == parent_run_id)
            .map(|(_, prepared)| prepared.clone())
            .collect();
        for prepared in children {
            let child_run = prepared.child.operation.run_id;
            let initiator =
                fanout_initiator(parent_run_id, parent_initiator, &projection, child_run);
            match prepared.placement {
                ChildPlacement::RemoteChildSession => {}
                ChildPlacement::CompatibleLaneInParentSession => {
                    Box::pin(self.cancel_run(child_run, initiator, next_env)).await?;
                }
                ChildPlacement::IsolatedChildSession => {
                    let child = Self::open(
                        Arc::clone(&self.store),
                        prepared.child.operation.session_id,
                        Arc::clone(&prepared.child.operation.tenant_scope),
                    )
                    .await?;
                    Box::pin(child.cancel_run(child_run, initiator, next_env)).await?;
                }
            }
        }
        Ok(())
    }

    fn acquire(&self, lane_id: LaneId, owner: LaneOwner) -> Result<(), SessionError> {
        let mut inner = self.lock()?;
        if inner.guards.contains_key(&lane_id) {
            return Err(SessionError::LaneBusy);
        }
        inner.guards.insert(lane_id, owner);
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, SessionInner>, SessionError> {
        self.inner.lock().map_err(|_| SessionError::Poisoned)
    }
}

fn fanout_initiator(
    parent_run_id: RunId,
    parent_initiator: &CancellationInitiator,
    projection: &SessionProjection,
    child_run: RunId,
) -> CancellationInitiator {
    let deadline = projection
        .operations()
        .get(&child_run)
        .is_some_and(|child| {
            matches!(parent_initiator, CancellationInitiator::Deadline)
                && child.propagation.deadline == DeadlinePropagation::MinimumOfParentAndChild
        });
    if deadline {
        CancellationInitiator::Deadline
    } else {
        CancellationInitiator::ParentRun { parent_run_id }
    }
}

fn session_draft(
    record_id: RecordId,
    session_id: SessionId,
    lane_id: LaneId,
    timestamp: Timestamp,
    body: RecordBody,
) -> Result<RecordDraft, SessionError> {
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        record_id,
        session_id,
        lane_id,
        None,
        timestamp,
        Vec::new(),
        body,
    )
    .map_err(|_| SessionError::Commit {
        code: "session_records_invalid",
    })
}

type InternKey = (usize, SessionId);

fn store_key(store: &Arc<dyn JournalStore>) -> usize {
    Arc::as_ptr(store).cast::<u8>() as usize
}

fn with_interns<R>(
    f: impl FnOnce(&mut BTreeMap<InternKey, Weak<SessionRuntime>>) -> R,
) -> Option<R> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static INTERNS: OnceLock<Mutex<BTreeMap<InternKey, Weak<SessionRuntime>>>> =
            OnceLock::new();
        INTERNS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .ok()
            .map(|mut map| f(&mut map))
    }
    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        thread_local! {
            static INTERNS: RefCell<BTreeMap<InternKey, Weak<SessionRuntime>>> =
                const { RefCell::new(BTreeMap::new()) };
        }
        INTERNS.with(|cell| cell.try_borrow_mut().ok().map(|mut map| f(&mut map)))
    }
}

fn interned(store: &Arc<dyn JournalStore>, session_id: SessionId) -> Option<Arc<SessionRuntime>> {
    with_interns(|map| {
        map.get(&(store_key(store), session_id))
            .and_then(Weak::upgrade)
    })
    .flatten()
}

fn intern(runtime: SessionRuntime) -> Arc<SessionRuntime> {
    let key = (store_key(&runtime.store), runtime.session_id);
    let runtime = Arc::new(runtime);
    let _ = with_interns(|map| {
        map.insert(key, Arc::downgrade(&runtime));
    });
    runtime
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_error_codes_are_stable() {
        assert_eq!(SessionError::LaneBusy.code(), "lane_busy");
        assert_eq!(SessionError::UnknownLane.code(), "unknown_lane");
        assert_eq!(
            SessionError::DuplicateLaneName.code(),
            "duplicate_lane_name"
        );
    }
}
