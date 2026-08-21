//! In-process session writer, lane guard, and lineage fan-out.
//!
//! This is composition over the existing journal store. It is not a seventh
//! port and does not add `KernelState` fields.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AppendBatchId, CancellationInitiator, ChildPlacement, ConversationEntry, ConversationError,
    DeadlinePropagation, EntryBody, EntryId, KernelInput, LABEL_MAX_BYTES, LaneCreated, LaneId,
    LaneMoved, Message, Metadata, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
    RecordDraft, RecordId, RunAccepted, RunId, SessionCreated, SessionId, SessionProjection,
    Timestamp, TransitionEnv,
};
use thiserror::Error;

use crate::coordinator::{CommitCoordinator, CommitCoordinatorError, project_loaded};
use crate::journal::{JournalStore, LoadRequest};
use crate::services::identity_map::{ExternalIdentityKey, ExternalIdentityMap, IdentityMapError};
use crate::services::session_intern::{self, InternDecision};
use crate::services::session_sync::StructuralGate;

/// In-process owner of one `(session_id, lane_id)` guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneOwner {
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
    /// Message text is empty or exceeds the content bound.
    #[error("invalid message text")]
    InvalidMessageText,
    /// Another lane already uses this application name.
    #[error("duplicate lane name")]
    DuplicateLaneName,
    /// Shared session state is poisoned.
    #[error("session lock is poisoned")]
    Poisoned,
    /// An interned runtime belongs to a different tenant scope.
    #[error("session tenant scope mismatch")]
    TenantScopeMismatch,
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
            Self::InvalidMessageText => "invalid_message_text",
            Self::DuplicateLaneName => "duplicate_lane_name",
            Self::Poisoned => "session_lock_poisoned",
            Self::TenantScopeMismatch => "tenant_scope_mismatch",
            Self::Identity(IdentityMapError::Conflict) => "identity_conflict",
            Self::Identity(IdentityMapError::InvalidKey { .. }) => "invalid_identity_key",
            Self::Conversation(_) => "conversation_invalid",
        }
    }

    fn recover(error: &CommitCoordinatorError) -> Self {
        Self::Recover {
            code: error.stable_code(),
        }
    }

    fn commit(error: &CommitCoordinatorError) -> Self {
        Self::Commit {
            code: error.stable_code(),
        }
    }
}

struct SessionInner {
    projection: SessionProjection,
    guards: BTreeMap<LaneId, LaneOwner>,
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    live_runs: BTreeMap<RunId, crate::RunHandle>,
    /// Structural-only coordinator at the live session head. Taken out for
    /// the duration of one structural commit so the mutex is not held across
    /// store I/O.
    head: Option<CommitCoordinator>,
}

/// Shared in-process writer for one session journal.
pub struct SessionRuntime {
    store: Arc<dyn JournalStore>,
    session_id: SessionId,
    tenant_scope: Arc<str>,
    inner: Mutex<SessionInner>,
    structural: StructuralGate,
}

impl SessionRuntime {
    /// Create a session and persist `SessionCreated` plus `LaneCreated("main")`.
    ///
    /// # Errors
    ///
    /// Returns a recover, commit, or intern-table poison failure.
    pub async fn create(
        store: Arc<dyn JournalStore>,
        tenant_scope: impl Into<Arc<str>>,
        ids: SessionCreateIds,
    ) -> Result<Arc<Self>, SessionError> {
        let tenant_scope = tenant_scope.into();
        loop {
            match session_intern::decide(&store, ids.session_id)? {
                InternDecision::Existing(existing) => {
                    existing.ensure_tenant_scope(&tenant_scope)?;
                    return Ok(existing);
                }
                InternDecision::Wait(follower) => follower.wait().await,
                InternDecision::Lead(leader) => {
                    let loaded = store
                        .load(LoadRequest {
                            session_id: ids.session_id,
                        })
                        .await
                        .map_err(|_| SessionError::Recover {
                            code: "session_load_failed",
                        })?;
                    if loaded.head_sequence > 0 {
                        let projection = project_loaded(&loaded)
                            .map_err(|code| SessionError::Recover { code })?;
                        let coordinator = CommitCoordinator::structural_from_loaded(
                            Arc::clone(&store),
                            &loaded,
                            projection.clone(),
                        )
                        .map_err(|error| SessionError::recover(&error))?;
                        return leader.complete(Self {
                            store,
                            session_id: ids.session_id,
                            tenant_scope,
                            inner: Mutex::new(SessionInner {
                                projection,
                                guards: BTreeMap::new(),
                                #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
                                live_runs: BTreeMap::new(),
                                head: Some(coordinator),
                            }),
                            structural: StructuralGate::default(),
                        });
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
                                    RecordBody::SessionCreated(SessionCreated::new(
                                        Metadata::empty(),
                                    )),
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
                    coordinator.mark_structural_head();
                    return leader.complete(Self {
                        store,
                        session_id: ids.session_id,
                        tenant_scope,
                        inner: Mutex::new(SessionInner {
                            projection: coordinator.session().clone(),
                            guards: BTreeMap::new(),
                            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
                            live_runs: BTreeMap::new(),
                            head: Some(coordinator),
                        }),
                        structural: StructuralGate::default(),
                    });
                }
            }
        }
    }

    /// Rebuild the projection without respawning non-terminal runs.
    ///
    /// Pre-046 journals without `SessionCreated` still open.
    ///
    /// # Errors
    ///
    /// Returns a recover or intern-table poison failure.
    pub async fn open(
        store: Arc<dyn JournalStore>,
        session_id: SessionId,
        tenant_scope: impl Into<Arc<str>>,
    ) -> Result<Arc<Self>, SessionError> {
        let tenant_scope = tenant_scope.into();
        loop {
            match session_intern::decide(&store, session_id)? {
                InternDecision::Existing(existing) => {
                    existing.ensure_tenant_scope(&tenant_scope)?;
                    return Ok(existing);
                }
                InternDecision::Wait(follower) => follower.wait().await,
                InternDecision::Lead(leader) => {
                    let loaded = store.load(LoadRequest { session_id }).await.map_err(|_| {
                        SessionError::Recover {
                            code: "session_load_failed",
                        }
                    })?;
                    let projection =
                        project_loaded(&loaded).map_err(|code| SessionError::Recover { code })?;
                    let coordinator = CommitCoordinator::structural_from_loaded(
                        Arc::clone(&store),
                        &loaded,
                        projection.clone(),
                    )
                    .map_err(|error| SessionError::recover(&error))?;
                    return leader.complete(Self {
                        store,
                        session_id,
                        tenant_scope,
                        inner: Mutex::new(SessionInner {
                            projection,
                            guards: BTreeMap::new(),
                            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
                            live_runs: BTreeMap::new(),
                            head: Some(coordinator),
                        }),
                        structural: StructuralGate::default(),
                    });
                }
            }
        }
    }

    /// Return the interned writer for this store and session, when present.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Poisoned`] when the intern table is unavailable.
    pub fn existing(
        store: &Arc<dyn JournalStore>,
        session_id: SessionId,
    ) -> Result<Option<Arc<Self>>, SessionError> {
        session_intern::existing(store, session_id)
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
        let _structural = self.structural.acquire().await?;
        self.refresh_locked().await
    }

    async fn refresh_locked(&self) -> Result<SessionProjection, SessionError> {
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
        let coordinator = CommitCoordinator::structural_from_loaded(
            Arc::clone(&self.store),
            &loaded,
            projection.clone(),
        )
        .map_err(|error| SessionError::recover(&error))?;
        let mut inner = self.lock()?;
        inner.projection = projection.clone();
        inner.head = Some(coordinator);
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
        let _structural = self.structural.acquire().await?;
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
        let _structural = self.structural.acquire().await?;
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
        let _structural = self.structural.acquire().await?;
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

    /// Atomically reserve an idle lane for `run_id` and append its user input.
    ///
    /// The returned messages are the complete durable history immediately
    /// preceding `message`. The active guard remains held on success and must
    /// be released with [`Self::release_run`].
    ///
    /// # Errors
    ///
    /// Returns a busy-lane, unknown-lane, invalid-history, or commit failure.
    pub async fn append_message_for_run(
        &self,
        lane_id: LaneId,
        run_id: RunId,
        message: &Message,
        ids: LaneAppendIds,
    ) -> Result<Vec<Message>, SessionError> {
        let _structural = self.structural.acquire().await?;
        let (parent_id, history) = {
            let mut inner = self.lock()?;
            let lane = inner
                .projection
                .lane_by_id(lane_id)
                .ok_or(SessionError::UnknownLane)?;
            if inner.projection.active_on_lane(lane_id).is_some()
                || inner.guards.contains_key(&lane_id)
            {
                return Err(SessionError::LaneBusy);
            }
            let history = match lane.leaf_id {
                Some(leaf_id) => inner
                    .projection
                    .history(leaf_id)
                    .map_err(SessionError::Conversation)?
                    .into_iter()
                    .map(|entry| match entry.body() {
                        EntryBody::Message(message) => message.clone(),
                    })
                    .collect(),
                None => Vec::new(),
            };
            let parent_id = lane.leaf_id;
            inner.guards.insert(lane_id, LaneOwner::Active(run_id));
            (parent_id, history)
        };
        let entry = match ConversationEntry::from_message(message, parent_id, lane_id, 0) {
            Ok(entry) => entry,
            Err(error) => {
                self.release_run(lane_id, run_id);
                return Err(SessionError::Conversation(error));
            }
        };
        let entry_id = entry.id();
        let records = match (|| -> Result<Vec<RecordDraft>, SessionError> {
            Ok(vec![
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
            ])
        })() {
            Ok(records) => records,
            Err(error) => {
                self.release_run(lane_id, run_id);
                return Err(error);
            }
        };
        let result = self.commit_records(ids.batch_id, records).await;
        if let Err(error) = result {
            self.release_run(lane_id, run_id);
            return Err(error);
        }
        Ok(history)
    }

    /// Append the completed assistant message while the matching run owns the lane.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane, unknown-lane, conversation, or commit failure.
    pub async fn append_message_from_run(
        &self,
        lane_id: LaneId,
        run_id: RunId,
        message: &Message,
        ids: LaneAppendIds,
    ) -> Result<EntryId, SessionError> {
        let _structural = self.structural.acquire().await?;
        let parent_id = {
            let inner = self.lock()?;
            let lane = inner
                .projection
                .lane_by_id(lane_id)
                .ok_or(SessionError::UnknownLane)?;
            if inner.guards.get(&lane_id) != Some(&LaneOwner::Active(run_id)) {
                return Err(SessionError::LaneBusy);
            }
            lane.leaf_id
        };
        let entry = ConversationEntry::from_message(message, parent_id, lane_id, 0)
            .map_err(SessionError::Conversation)?;
        let entry_id = entry.id();
        self.commit_records(
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
        .await?;
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

    /// Release a run guard only when it is still owned by `run_id`.
    pub fn release_run(&self, lane_id: LaneId, run_id: RunId) {
        if let Ok(mut inner) = self.lock()
            && inner.guards.get(&lane_id) == Some(&LaneOwner::Active(run_id))
        {
            inner.guards.remove(&lane_id);
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            inner.live_runs.remove(&run_id);
        }
    }

    /// Attach the live runtime handle for the run currently owning `lane_id`.
    ///
    /// This keeps authenticated lane cancellation on the owning coordinator,
    /// where committed cancellation can signal active effect drivers.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::LaneBusy`] when the guard no longer belongs to
    /// `run_id`.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub fn bind_run_handle(
        &self,
        lane_id: LaneId,
        run_id: RunId,
        handle: crate::RunHandle,
    ) -> Result<(), SessionError> {
        let mut inner = self.lock()?;
        if inner.guards.get(&lane_id) != Some(&LaneOwner::Active(run_id)) {
            return Err(SessionError::LaneBusy);
        }
        inner.live_runs.insert(run_id, handle);
        Ok(())
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
        self.sync_structural_head(&coordinator)?;
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
    pub async fn cancel_run<F>(
        &self,
        run_id: RunId,
        initiator: CancellationInitiator,
        next_env: &mut F,
    ) -> Result<(), SessionError>
    where
        F: FnMut() -> Result<TransitionEnv, SessionError> + Send,
    {
        let projection = self.refresh().await?;
        if let Some(operation) = projection.operations().get(&run_id)
            && !operation.terminal
        {
            let input = KernelInput::CancelRequested(finstack_ai_kernel::CancelRequested {
                initiator: initiator.clone(),
                reason: None,
            });
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            let live = self.lock()?.live_runs.get(&run_id).cloned();
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            if let Some(handle) = live {
                let outcome =
                    handle
                        .submit(next_env()?, input)
                        .await
                        .map_err(|_| SessionError::Commit {
                            code: "live_run_cancel_failed",
                        })?;
                if let Some(fault) = outcome.fault {
                    return Err(SessionError::Commit { code: fault.code });
                }
            } else {
                self.cancel_recovered(run_id, next_env()?, input).await?;
            }
            #[cfg(not(any(feature = "native-tokio", feature = "wasm-host")))]
            self.cancel_recovered(run_id, next_env()?, input).await?;
        }
        self.fan_out(run_id, &initiator, next_env).await
    }

    async fn cancel_recovered(
        &self,
        run_id: RunId,
        env: TransitionEnv,
        input: KernelInput,
    ) -> Result<(), SessionError> {
        let mut coordinator = self.coordinator_for_run(Some(run_id)).await?;
        match coordinator.submit(env, input).await {
            Ok(_)
            | Err(CommitCoordinatorError::Decision {
                code: "invalid_input_payload",
            }) => {}
            Err(error) => return Err(SessionError::commit(&error)),
        }
        self.sync_structural_head(&coordinator)
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
        let mut coordinator = self.take_structural_head().await?;
        let result = coordinator.commit_session_records(batch_id, records).await;
        match result {
            Ok(_) => self.put_structural_head(coordinator),
            Err(error) => Err(SessionError::commit(&error)),
        }
    }

    async fn take_structural_head(&self) -> Result<CommitCoordinator, SessionError> {
        if let Some(coordinator) = self.lock()?.head.take() {
            return Ok(coordinator);
        }
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
        CommitCoordinator::structural_from_loaded(Arc::clone(&self.store), &loaded, projection)
            .map_err(|error| SessionError::recover(&error))
    }

    fn put_structural_head(&self, coordinator: CommitCoordinator) -> Result<(), SessionError> {
        let mut inner = self.lock()?;
        inner.projection = coordinator.session().clone();
        inner.head = Some(coordinator);
        Ok(())
    }

    fn sync_structural_head(&self, foreign: &CommitCoordinator) -> Result<(), SessionError> {
        let mut inner = self.lock()?;
        inner.projection = foreign.session().clone();
        if let Some(head) = inner.head.as_mut() {
            head.adopt_live_session(
                foreign.session().clone(),
                foreign.state().last_applied_sequence,
                foreign.head_checksum(),
            )
            .map_err(|error| SessionError::recover(&error))?;
        }
        Ok(())
    }

    async fn fan_out<F>(
        &self,
        parent_run_id: RunId,
        parent_initiator: &CancellationInitiator,
        next_env: &mut F,
    ) -> Result<(), SessionError>
    where
        F: FnMut() -> Result<TransitionEnv, SessionError> + Send,
    {
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

    fn ensure_tenant_scope(&self, tenant_scope: &str) -> Result<(), SessionError> {
        if self.tenant_scope.as_ref() == tenant_scope {
            Ok(())
        } else {
            Err(SessionError::TenantScopeMismatch)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_error_codes_are_stable() {
        assert_eq!(SessionError::LaneBusy.code(), "lane_busy");
        assert_eq!(SessionError::UnknownLane.code(), "unknown_lane");
        assert_eq!(
            SessionError::InvalidMessageText.code(),
            "invalid_message_text"
        );
        assert_eq!(
            SessionError::DuplicateLaneName.code(),
            "duplicate_lane_name"
        );
        assert_eq!(SessionError::Poisoned.code(), "session_lock_poisoned");
    }

    struct UnavailableStore;

    impl JournalStore for UnavailableStore {
        fn append(
            &self,
            _request: finstack_ai_kernel::AppendRequest,
        ) -> crate::PortFuture<Result<finstack_ai_kernel::CommittedBatch, crate::StoreError>>
        {
            Box::pin(async {
                Err(crate::StoreError::Unavailable {
                    reason_code: "unavailable",
                })
            })
        }

        fn load(
            &self,
            _request: LoadRequest,
        ) -> crate::PortFuture<Result<crate::LoadedSession, crate::StoreError>> {
            Box::pin(async {
                Err(crate::StoreError::Unavailable {
                    reason_code: "unavailable",
                })
            })
        }

        fn write_snapshot(
            &self,
            _request: crate::SnapshotRequest,
        ) -> crate::PortFuture<Result<crate::SnapshotReceipt, crate::StoreError>> {
            Box::pin(async {
                Err(crate::StoreError::Unavailable {
                    reason_code: "unavailable",
                })
            })
        }

        fn health(&self) -> crate::PortFuture<Result<crate::StoreHealth, crate::StoreError>> {
            Box::pin(async {
                Err(crate::StoreError::Unavailable {
                    reason_code: "unavailable",
                })
            })
        }
    }

    #[test]
    fn intern_table_unavailable_does_not_yield_a_second_owner() {
        let store: Arc<dyn JournalStore> = Arc::new(UnavailableStore);
        let session_id = SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
        let _guard = session_intern::FailInterns::arm();
        assert!(matches!(
            SessionRuntime::existing(&store, session_id),
            Err(SessionError::Poisoned)
        ));
        assert!(matches!(
            SessionRuntime::existing(&store, session_id),
            Err(SessionError::Poisoned)
        ));
    }

    #[test]
    fn session_intern_allows_one_initialization_leader_per_store_and_session() {
        let store: Arc<dyn JournalStore> = Arc::new(UnavailableStore);
        let session_id = SessionId::parse("11234567-89ab-7cde-89ab-0123456789ab").expect("id");
        let leader = match session_intern::decide(&store, session_id).expect("first decision") {
            InternDecision::Lead(leader) => leader,
            InternDecision::Existing(_) | InternDecision::Wait(_) => {
                panic!("first decision must lead")
            }
        };
        assert!(matches!(
            session_intern::decide(&store, session_id).expect("second decision"),
            InternDecision::Wait(_)
        ));
        drop(leader);
        assert!(matches!(
            session_intern::decide(&store, session_id).expect("retry decision"),
            InternDecision::Lead(_)
        ));
    }
}
