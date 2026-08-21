//! Public Session and Lane handles over [`SessionRuntime`].

#[cfg(feature = "native-tokio")]
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AuthorizationEvidence, EntryId, IdTag, LaneId, Message, MessageRole, PrincipalRef, ProviderIds,
    SessionId, TextBlock, Timestamp,
};
use finstack_ai_runtime::{
    ExternalIdentityKey, ExternalIdentityMap, JournalStore, LaneAppendIds, LaneCreateIds,
    LaneInspect, SessionCreateIds, SessionError, SessionRuntime, UuidV7Generator,
};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver::{
    InstalledClock as AgentClock, InstalledRandom as AgentRandom,
};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::{OsRandomSource as AgentRandom, SystemClock as AgentClock};

/// Live handle for one journaled session.
#[derive(Clone)]
#[expect(
    clippy::struct_field_names,
    reason = "session_id is the shared semantic field name"
)]
pub struct Session {
    store: Arc<dyn JournalStore>,
    session_id: SessionId,
    tenant_scope: Arc<str>,
    runtime: Arc<Mutex<Option<Arc<SessionRuntime>>>>,
    #[cfg(feature = "native-tokio")]
    live: Arc<Mutex<BTreeMap<LaneId, crate::agent::LaneLive>>>,
}

impl Session {
    /// Create a session and persist `SessionCreated` plus `LaneCreated("main")`.
    ///
    /// # Arguments
    ///
    /// * `store` - Journal that will own the new session envelopes.
    /// * `tenant_scope` - Host-captured tenant scope stored on every record.
    ///
    /// # Errors
    ///
    /// Returns a configuration or store failure.
    pub async fn create(
        store: Arc<dyn JournalStore>,
        tenant_scope: impl Into<Arc<str>>,
    ) -> Result<Self, SessionError> {
        let tenant_scope = tenant_scope.into();
        let runtime = SessionRuntime::create(
            Arc::clone(&store),
            Arc::clone(&tenant_scope),
            generated_create_ids()?,
        )
        .await?;
        Ok(Self::from_runtime(runtime))
    }

    /// Open an existing session without respawning non-terminal runs.
    ///
    /// This is inspect-not-continue. Resume belongs to an explicit workflow
    /// driver when one is composed.
    ///
    /// # Arguments
    ///
    /// * `store` - Journal that already contains `session_id`.
    /// * `session_id` - Durable session identity to rebuild.
    /// * `tenant_scope` - Host-captured tenant scope; mismatch fails closed.
    ///
    /// # Errors
    ///
    /// Returns a recover failure when the journal cannot be loaded.
    pub async fn open(
        store: Arc<dyn JournalStore>,
        session_id: SessionId,
        tenant_scope: impl Into<Arc<str>>,
    ) -> Result<Self, SessionError> {
        Ok(Self::from_runtime(
            SessionRuntime::open(store, session_id, tenant_scope).await?,
        ))
    }

    /// Construct a live handle that opens the runtime on first mutation.
    ///
    /// Intern-table poison is treated as absence so construction stays
    /// infallible. The first mutation fail-closes through
    /// [`SessionRuntime::open`].
    ///
    /// # Arguments
    ///
    /// * `store` - Journal backing this session.
    /// * `session_id` - Durable session identity.
    /// * `tenant_scope` - Host-captured tenant scope.
    #[must_use]
    pub(crate) fn pending(
        store: Arc<dyn JournalStore>,
        session_id: SessionId,
        tenant_scope: impl Into<Arc<str>>,
    ) -> Self {
        let tenant_scope = tenant_scope.into();
        let runtime = SessionRuntime::existing(&store, session_id)
            .ok()
            .flatten()
            .filter(|runtime| runtime.tenant_scope() == tenant_scope.as_ref());
        Self {
            store,
            session_id,
            tenant_scope,
            runtime: Arc::new(Mutex::new(runtime)),
            #[cfg(feature = "native-tokio")]
            live: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Wrap an already-constructed runtime.
    ///
    /// # Arguments
    ///
    /// * `runtime` - Session runtime recovered or created by the caller.
    #[must_use]
    pub fn from_runtime(runtime: Arc<SessionRuntime>) -> Self {
        Self {
            store: runtime.store(),
            session_id: runtime.session_id(),
            tenant_scope: Arc::from(runtime.tenant_scope()),
            runtime: Arc::new(Mutex::new(Some(runtime))),
            #[cfg(feature = "native-tokio")]
            live: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Session-scoped in-process run table used by [`crate::Lane::suspend`].
    #[cfg(feature = "native-tokio")]
    pub(crate) fn live_lanes(&self) -> &Mutex<BTreeMap<LaneId, crate::agent::LaneLive>> {
        &self.live
    }

    /// Durable session identity.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Tenant scope captured by the host.
    #[must_use]
    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    async fn ensure(&self) -> Result<Arc<SessionRuntime>, SessionError> {
        if let Some(runtime) = self
            .runtime
            .lock()
            .map_err(|_| SessionError::Poisoned)?
            .clone()
        {
            return Ok(runtime);
        }
        let runtime = SessionRuntime::open(
            Arc::clone(&self.store),
            self.session_id,
            Arc::clone(&self.tenant_scope),
        )
        .await?;
        *self.runtime.lock().map_err(|_| SessionError::Poisoned)? = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    /// Create a named lane, optionally forking from an existing entry.
    ///
    /// # Errors
    ///
    /// Returns a configuration or commit failure.
    pub async fn create_lane(
        &self,
        name: impl Into<Arc<str>>,
        fork: Option<EntryId>,
    ) -> Result<Lane, SessionError> {
        let ids = generated_lane_ids(fork.is_some())?;
        let lane_id = self.ensure().await?.create_lane(name, fork, ids).await?;
        Ok(Lane {
            session: self.clone(),
            lane_id,
        })
    }

    /// List restored lanes after refreshing the projection.
    ///
    /// # Errors
    ///
    /// Returns a recover failure when the journal cannot be loaded.
    pub async fn list_lanes(&self) -> Result<Vec<Lane>, SessionError> {
        let projection = self.ensure().await?.refresh().await?;
        Ok(projection
            .lanes()
            .keys()
            .copied()
            .map(|lane_id| Lane {
                session: self.clone(),
                lane_id,
            })
            .collect())
    }

    /// Look up one lane by application name.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownLane`] when the name is missing.
    pub async fn lane(&self, name: &str) -> Result<Lane, SessionError> {
        let projection = self.ensure().await?.refresh().await?;
        let (lane_id, _) = projection.lane(name).ok_or(SessionError::UnknownLane)?;
        Ok(Lane {
            session: self.clone(),
            lane_id,
        })
    }

    /// Look up one lane by durable identity.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownLane`] when the identity is missing.
    pub async fn lane_by_id(&self, lane_id: LaneId) -> Result<Lane, SessionError> {
        let projection = self.ensure().await?.refresh().await?;
        if projection.lane_by_id(lane_id).is_none() {
            return Err(SessionError::UnknownLane);
        }
        Ok(Lane {
            session: self.clone(),
            lane_id,
        })
    }

    /// Bind one of this session's lanes into a host-owned identity map.
    ///
    /// # Errors
    ///
    /// Returns an unknown-lane or identity-map failure.
    pub fn bind_external_identity(
        &self,
        map: &dyn ExternalIdentityMap,
        key: ExternalIdentityKey,
        lane_id: LaneId,
    ) -> Result<(), SessionError> {
        let runtime = self
            .runtime
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .ok_or(SessionError::UnknownLane)?;
        runtime.bind_external_identity(map, key, lane_id)
    }

    /// Resolve a host-owned external identity key.
    ///
    /// The map is host-owned and is not read from the journal.
    #[must_use]
    pub fn resolve_external_identity(
        map: &dyn ExternalIdentityMap,
        key: &ExternalIdentityKey,
    ) -> Option<(SessionId, LaneId)> {
        map.resolve(key)
    }

    pub(crate) async fn runtime(&self) -> Result<Arc<SessionRuntime>, SessionError> {
        self.ensure().await
    }

    #[cfg(feature = "native-tokio")]
    pub(crate) fn journal_store(&self) -> Arc<dyn JournalStore> {
        Arc::clone(&self.store)
    }
}

/// Live handle for one lane in a session.
#[derive(Clone)]
pub struct Lane {
    session: Session,
    lane_id: LaneId,
}

impl Lane {
    /// Session that owns this lane.
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Durable lane identity.
    #[must_use]
    pub const fn lane_id(&self) -> LaneId {
        self.lane_id
    }

    /// Point this idle lane at an existing entry without copying.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane, unknown-entry, or commit failure.
    pub async fn navigate(&self, entry_id: EntryId) -> Result<(), SessionError> {
        let now = generated_timestamp()?;
        self.session
            .ensure()
            .await?
            .navigate(self.lane_id, entry_id, generate()?, generate()?, now)
            .await
    }

    /// Inspect name, leaf, active run, and history.
    ///
    /// # Errors
    ///
    /// Returns an unknown-lane or recover failure.
    pub async fn inspect(&self) -> Result<LaneInspect, SessionError> {
        self.session.ensure().await?.inspect(self.lane_id).await
    }

    /// Cancel the active run on this lane and fan out through child mappings.
    ///
    /// # Arguments
    ///
    /// * `principal` - Authenticated principal requesting cancellation.
    /// * `authorization` - Exact authorization decision for that principal.
    ///
    /// # Errors
    ///
    /// Returns a recover or commit failure.
    pub async fn cancel(
        &self,
        principal: PrincipalRef,
        authorization: AuthorizationEvidence,
    ) -> Result<(), SessionError> {
        let runtime = self.session.ensure().await?;
        let projection = runtime.refresh().await?;
        let Some(run_id) = projection.active_on_lane(self.lane_id) else {
            return Ok(());
        };
        let initiator = finstack_ai_kernel::CancellationInitiator::Principal {
            principal,
            authorization,
        };
        #[cfg(feature = "native-tokio")]
        if let Some(run) = crate::agent::live_run(self)? {
            return run
                .cancel_with_initiator(initiator)
                .await
                .map_err(|error| SessionError::Commit { code: error.code() });
        }
        runtime
            .cancel_run(run_id, initiator, &mut generated_env)
            .await
    }

    /// Append one user text message on this idle lane.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane or commit failure.
    pub async fn append_text(&self, text: &str) -> Result<EntryId, SessionError> {
        let now = generated_timestamp()?;
        let message = Message::try_new(
            generate()?,
            MessageRole::User,
            vec![finstack_ai_kernel::ContentBlock::Text(
                TextBlock::try_new(text).map_err(|_| SessionError::InvalidMessageText)?,
            )],
            now,
            None,
            ProviderIds::empty(),
            finstack_ai_kernel::Metadata::empty(),
        )
        .map_err(|_| SessionError::Commit {
            code: "message_invalid",
        })?;
        self.session
            .ensure()
            .await?
            .append_message(
                self.lane_id,
                &message,
                LaneAppendIds {
                    entry_record_id: generate()?,
                    lane_moved_record_id: generate()?,
                    batch_id: generate()?,
                },
            )
            .await
    }
}

fn generated_create_ids() -> Result<SessionCreateIds, SessionError> {
    Ok(SessionCreateIds {
        session_id: generate()?,
        main_lane_id: generate()?,
        session_created_record_id: generate()?,
        lane_created_record_id: generate()?,
        batch_id: generate()?,
        now: generated_timestamp()?,
    })
}

fn generated_lane_ids(fork: bool) -> Result<LaneCreateIds, SessionError> {
    Ok(LaneCreateIds {
        lane_id: generate()?,
        lane_created_record_id: generate()?,
        lane_moved_record_id: fork.then(generate).transpose()?,
        batch_id: generate()?,
        now: generated_timestamp()?,
    })
}

fn generated_timestamp() -> Result<Timestamp, SessionError> {
    finstack_ai_runtime::Clock::now(&AgentClock).map_err(|_| SessionError::Commit {
        code: "clock_unavailable",
    })
}

fn generate<T: IdTag>() -> Result<finstack_ai_kernel::Id<T>, SessionError> {
    UuidV7Generator::new(AgentClock, AgentRandom)
        .generate()
        .map_err(|_| SessionError::Commit {
            code: "id_generation_failed",
        })
}

fn generated_env() -> Result<finstack_ai_kernel::TransitionEnv, SessionError> {
    use finstack_ai_kernel::{
        AllocatedIds, AppendBatchTag, CancellationRequestTag, RecordTag, TransitionEnv,
    };
    Ok(TransitionEnv {
        now: generated_timestamp()?,
        ids: AllocatedIds::try_new(
            vec![generate::<RecordTag>()?],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![generate::<AppendBatchTag>()?],
            vec![generate::<CancellationRequestTag>()?],
        )
        .map_err(|_| SessionError::Commit {
            code: "allocated_ids_invalid",
        })?,
    })
}
