//! Runtime-driver contract for external workflow engines.
//!
//! This is not a seventh port. Workflow engines drive existing
//! [`CommitCoordinator`] / [`RunTaskOwner`] post-commit actions. The kernel
//! journal remains authoritative for agent-run semantics.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    ActiveToolCallStatus, CapabilityId, ComponentId, EffectId, EffectKind, EffectRequested,
    ExternalEffectCompletionCommand, ExternalHandleRef, InteractionId, InteractionRequest,
    InteractionResolutionCommand, KernelState, LaneId, OperationLocator, RunId, RunPhase,
    SessionId, Timestamp,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ApprovalGrantMode, CommitCoordinator, ContextProvider, EventHubConfig, ExternalClock,
    ExternalCompletionRouter, ExternalRouteError, ExternalRouteOutcome, IdGenerationError,
    InteractionRouter, JournalStore, LockedModelContextProfile, Model, ModelCapabilities,
    ModelTaskConfig, ModelWarmupContext, RandomSource, ReadyModel, ResolvedMiddlewareChain,
    ResolvedToolCatalog, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy, SecurityAuditGate,
    ToolSpec, ToolStreamLimits, ToolTaskConfig, model_retry_allowed, tool_retry_allowed,
};

/// Stable deny codes for [`retry_decision`].
pub const RETRY_LIMIT_REACHED: &str = "retry_limit_reached";
/// The outstanding effect is not safe to re-dispatch.
pub const RETRY_NOT_SAFE: &str = "retry_not_safe";
/// No outstanding effect matches the requested identity.
pub const EFFECT_NOT_OUTSTANDING: &str = "effect_not_outstanding";

/// Parked workflow wait reconstructed from authoritative kernel state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "Interaction carries the committed request envelope"
)]
pub enum WorkflowWait {
    /// Durable timer / sleep parked on a committed timer effect.
    Timer {
        /// Timer effect identity.
        effect_id: EffectId,
        /// Semantic due time.
        due_at: Timestamp,
    },
    /// Typed human or application interaction.
    Interaction {
        /// Interaction identity.
        interaction_id: InteractionId,
        /// Committed request envelope.
        request: InteractionRequest,
    },
    /// Externally completed deferred effect.
    DeferredEffect {
        /// Original effect identity.
        effect_id: EffectId,
        /// Non-secret external handle.
        handle: ExternalHandleRef,
    },
    /// Run reached a terminal phase.
    Terminal {
        /// Terminal phase when known.
        phase: Option<RunPhase>,
    },
}

/// Whether a workflow engine may re-dispatch one outstanding effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowRetryDecision {
    /// Kernel policy still allows a same-identity retry.
    Allow {
        /// Remaining kernel-allowed retries, or `None` when unlimited.
        remaining: Option<u32>,
    },
    /// Do not enqueue `ExecuteEffect`.
    Deny {
        /// Stable deny code.
        code: &'static str,
    },
}

/// Persistence handoff hint. Not kernel state and not a snapshot substitute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCheckpoint {
    /// Tenant captured from the session handle.
    pub tenant_scope: Arc<str>,
    /// Session identity.
    pub session_id: SessionId,
    /// Lane identity.
    pub lane_id: LaneId,
    /// Run identity.
    pub run_id: RunId,
    /// Last applied journal sequence observed after the wait commit.
    pub last_applied_seq: u64,
    /// Optional non-secret external handles.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub external_handles: BTreeMap<EffectId, ExternalHandleRef>,
}

/// Fail-closed workflow-driver errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkflowDriverError {
    /// Locator or tenant did not match the acquired session handle.
    #[error("unknown locator")]
    UnknownLocator,
    /// Journal recover failed.
    #[error("workflow recover failed: {code}")]
    Recover {
        /// Stable fault code.
        code: &'static str,
    },
    /// Owner spawn failed.
    #[error("workflow spawn failed: {code}")]
    Spawn {
        /// Stable fault code.
        code: &'static str,
    },
    /// Model/tool ports are required to continue a non-waiting run.
    #[error("workflow ports required")]
    PortsRequired,
    /// Audit gate is missing or unhealthy.
    #[error("workflow audit not ready")]
    AuditNotReady,
    /// Poll bound elapsed before a wait or terminal.
    #[error("workflow drive timeout")]
    DriveTimeout,
    /// Ingress router rejected the command.
    #[error(transparent)]
    Ingress(ExternalRouteError),
}

impl WorkflowDriverError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnknownLocator => "unknown_locator",
            Self::Recover { code } | Self::Spawn { code } => code,
            Self::PortsRequired => "ports_required",
            Self::AuditNotReady => "audit_not_ready",
            Self::DriveTimeout => "drive_timeout",
            Self::Ingress(_) => "ingress_rejected",
        }
    }
}

/// Classify a wait from recovered kernel state.
///
/// Returns `None` while the canonical model/tool loop is still in flight.
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::KernelState;
/// use finstack_ai_runtime::classify_wait;
///
/// assert!(classify_wait(&KernelState::default()).is_none());
/// ```
#[must_use]
pub fn classify_wait(state: &KernelState) -> Option<WorkflowWait> {
    if state.terminal.is_some()
        || matches!(
            state.phase,
            Some(RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled)
        )
    {
        return Some(WorkflowWait::Terminal { phase: state.phase });
    }
    if let Some(pending) = &state.pending_interaction {
        return Some(WorkflowWait::Interaction {
            interaction_id: pending.request.interaction_id(),
            request: pending.request.clone(),
        });
    }
    if let Some(timer) = &state.retry.pending {
        return Some(WorkflowWait::Timer {
            effect_id: timer.timer_effect_id,
            due_at: timer.due_at,
        });
    }
    if let Some(deferred) = state
        .pending_model_effect
        .as_ref()
        .and_then(|pending| pending.deferred.as_ref())
    {
        return Some(WorkflowWait::DeferredEffect {
            effect_id: deferred.effect_id,
            handle: deferred.handle.clone(),
        });
    }
    if let Some(batch) = &state.active_tool_batch {
        for call in batch.calls.iter() {
            if let ActiveToolCallStatus::Requested {
                deferred: Some(deferred),
                ..
            } = &call.status
            {
                return Some(WorkflowWait::DeferredEffect {
                    effect_id: deferred.effect_id,
                    handle: deferred.handle.clone(),
                });
            }
        }
    }
    None
}

/// Journal sequence always wins over a conflicting checkpoint hint.
///
/// # Examples
///
/// ```
/// use finstack_ai_runtime::resolve_checkpoint_sequence;
///
/// assert_eq!(resolve_checkpoint_sequence(5, Some(99)), 5);
/// assert_eq!(resolve_checkpoint_sequence(5, Some(5)), 5);
/// assert_eq!(resolve_checkpoint_sequence(5, None), 5);
/// ```
#[must_use]
pub const fn resolve_checkpoint_sequence(journal_seq: u64, hint: Option<u64>) -> u64 {
    let _ = hint;
    journal_seq
}

/// Ask whether a workflow engine may re-dispatch `effect_id`.
///
/// A [`WorkflowRetryDecision::Deny`] must not enqueue `ExecuteEffect`.
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::{EffectId, Id, KernelState};
/// use finstack_ai_runtime::{WorkflowRetryDecision, retry_decision};
///
/// let state = KernelState::default();
/// let effect_id = Id::from_bytes([
///     0, 0, 0, 0, 0, 0, 0x70, 0, 0x80, 0, 0, 0, 0, 0, 0, 1,
/// ]);
/// assert_eq!(
///     retry_decision(&state, effect_id, None, None),
///     WorkflowRetryDecision::Deny {
///         code: "effect_not_outstanding",
///     }
/// );
/// ```
#[must_use]
pub fn retry_decision(
    state: &KernelState,
    effect_id: EffectId,
    model: Option<&ModelCapabilities>,
    tool: Option<&ToolSpec>,
) -> WorkflowRetryDecision {
    let Some(requested) = outstanding_request(state, effect_id) else {
        return WorkflowRetryDecision::Deny {
            code: EFFECT_NOT_OUTSTANDING,
        };
    };
    let allowed = match requested.kind() {
        EffectKind::Model => {
            model.is_some_and(|capabilities| model_retry_allowed(requested, capabilities))
        }
        EffectKind::Tool => tool.is_some_and(|spec| tool_retry_allowed(requested, spec)),
        EffectKind::Context
        | EffectKind::Middleware
        | EffectKind::Interaction
        | EffectKind::Timer => false,
    };
    if !allowed {
        return WorkflowRetryDecision::Deny {
            code: RETRY_NOT_SAFE,
        };
    }
    let used = state.limit_usage.retries.max(state.retry.attempts);
    match state
        .accepted
        .as_ref()
        .and_then(|accepted| accepted.limits().max_retries)
    {
        Some(max) if used >= max => WorkflowRetryDecision::Deny {
            code: RETRY_LIMIT_REACHED,
        },
        Some(max) => WorkflowRetryDecision::Allow {
            remaining: Some(max.saturating_sub(used)),
        },
        None => WorkflowRetryDecision::Allow { remaining: None },
    }
}

fn outstanding_request(state: &KernelState, effect_id: EffectId) -> Option<&EffectRequested> {
    if let Some(pending) = &state.pending_model_effect
        && pending.requested.effect_id() == effect_id
    {
        return Some(&pending.requested);
    }
    if let Some(batch) = &state.active_tool_batch {
        for call in batch.calls.iter() {
            if let ActiveToolCallStatus::Requested { requested, .. } = &call.status
                && requested.effect_id() == effect_id
            {
                return Some(requested);
            }
        }
    }
    None
}

fn checkpoint_handles(state: &KernelState) -> BTreeMap<EffectId, ExternalHandleRef> {
    let mut handles = BTreeMap::new();
    if let Some(deferred) = state
        .pending_model_effect
        .as_ref()
        .and_then(|pending| pending.deferred.as_ref())
    {
        handles.insert(deferred.effect_id, deferred.handle.clone());
    }
    if let Some(batch) = &state.active_tool_batch {
        for call in batch.calls.iter() {
            if let ActiveToolCallStatus::Requested {
                deferred: Some(deferred),
                ..
            } = &call.status
            {
                handles.insert(deferred.effect_id, deferred.handle.clone());
            }
        }
    }
    handles
}

/// Seeded entropy for deterministic workflow-driver tests and examples.
#[derive(Debug, Clone)]
pub struct SeededRandom {
    next: Arc<AtomicU64>,
}

impl SeededRandom {
    /// Create a sequence source whose first block is derived from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            next: Arc::new(AtomicU64::new(seed)),
        }
    }
}

impl RandomSource for SeededRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        for chunk in buf.chunks_mut(8) {
            let value = self.next.fetch_add(1, Ordering::AcqRel).to_be_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
        Ok(())
    }
}

/// Native workflow driver over one recovered run.
///
/// The driver never plans model or tool batches. It spawns [`RunTaskOwner`]
/// and parks on journal-authoritative waits.
pub struct WorkflowSession {
    store: Arc<dyn JournalStore>,
    tenant_scope: Arc<str>,
    locator: OperationLocator,
    clock: ExternalClock,
    random: SeededRandom,
    audit: Arc<SecurityAuditGate>,
    model: Option<WorkflowModel>,
    catalog: Option<Arc<ResolvedToolCatalog>>,
    capability_owners: Option<Arc<BTreeMap<ComponentId, Arc<[CapabilityId]>>>>,
    middleware_chain: Option<Arc<ResolvedMiddlewareChain>>,
    context_providers: Option<Arc<[Arc<dyn ContextProvider>]>>,
    profile: Option<LockedModelContextProfile>,
    approval_grant: ApprovalGrantMode,
    owner: Option<RunTaskOwner>,
    last_state: KernelState,
    drive_timeout: Duration,
}

enum WorkflowModel {
    Unprepared(Arc<dyn Model>),
    Ready(Arc<ReadyModel>),
}

impl WorkflowSession {
    /// Attach to an already-accepted run without respawning it.
    ///
    /// Restore is [`JournalStore`] load plus [`CommitCoordinator::recover`].
    /// Tenant scope is taken from the acquired session handle.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowDriverError::UnknownLocator`] when the locator tenant
    /// does not match the session handle, and recover failures otherwise.
    pub async fn attach(
        store: Arc<dyn JournalStore>,
        locator: OperationLocator,
        clock: ExternalClock,
        random_seed: u64,
        audit: Arc<SecurityAuditGate>,
    ) -> Result<Self, WorkflowDriverError> {
        let coordinator = CommitCoordinator::recover(Arc::clone(&store), locator.session_id)
            .await
            .map_err(|_| WorkflowDriverError::UnknownLocator)?;
        let accepted = coordinator
            .state()
            .accepted
            .as_ref()
            .ok_or(WorkflowDriverError::UnknownLocator)?;
        if accepted.security().tenant_scope() != locator.tenant_scope.as_ref()
            || coordinator.state().session_id != Some(locator.session_id)
            || coordinator.state().lane_id != Some(locator.lane_id)
            || accepted.run_id() != locator.run_id
        {
            return Err(WorkflowDriverError::UnknownLocator);
        }
        Ok(Self {
            store,
            tenant_scope: Arc::from(accepted.security().tenant_scope()),
            locator,
            clock,
            random: SeededRandom::new(random_seed),
            audit,
            model: None,
            catalog: None,
            capability_owners: None,
            middleware_chain: None,
            context_providers: None,
            profile: None,
            approval_grant: ApprovalGrantMode::PerCall,
            owner: None,
            last_state: coordinator.state().clone(),
            drive_timeout: Duration::from_secs(2),
        })
    }

    /// Attach with an in-process no-op audit gate.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowDriverError::AuditNotReady`] when the gate cannot be
    /// enabled, and the same failures as [`Self::attach`].
    pub async fn trusted(
        store: Arc<dyn JournalStore>,
        locator: OperationLocator,
        clock: ExternalClock,
        random_seed: u64,
    ) -> Result<Self, WorkflowDriverError> {
        let audit = SecurityAuditGate::enable_noop()
            .await
            .map_err(|_| WorkflowDriverError::AuditNotReady)?;
        Self::attach(store, locator, clock, random_seed, audit).await
    }

    /// Bind model/tool ports used when the driver must spawn [`RunTaskOwner`].
    #[must_use]
    pub fn with_ports(
        mut self,
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        catalog: Option<Arc<ResolvedToolCatalog>>,
    ) -> Self {
        self.model = Some(WorkflowModel::Unprepared(model));
        self.profile = Some(profile);
        self.catalog = catalog;
        self
    }

    /// Bind a model that already completed construction warmup.
    #[must_use]
    pub fn with_ready_ports(
        mut self,
        model: Arc<ReadyModel>,
        profile: LockedModelContextProfile,
        catalog: Option<Arc<ResolvedToolCatalog>>,
    ) -> Self {
        self.model = Some(WorkflowModel::Ready(model));
        self.profile = Some(profile);
        self.catalog = catalog;
        self
    }

    /// Bind the paid-tool approval grant mode used when the driver respawns.
    #[must_use]
    pub fn with_approval_grant(mut self, mode: ApprovalGrantMode) -> Self {
        self.approval_grant = mode;
        self
    }

    /// Bind the lock-time capability ownership map used as a dispatch mask.
    #[must_use]
    pub fn with_capability_owners(
        mut self,
        owners: Arc<BTreeMap<ComponentId, Arc<[CapabilityId]>>>,
    ) -> Self {
        self.capability_owners = Some(owners);
        self
    }

    /// Bind the resolved middleware chain used when the driver respawns.
    ///
    /// [`CommitCoordinator::recover`] drops runtime ports. Without this bind,
    /// [`Self::respawn_owner`] passthroughs every stage fold.
    #[must_use]
    pub fn with_middleware_chain(mut self, chain: Arc<ResolvedMiddlewareChain>) -> Self {
        self.middleware_chain = Some(chain);
        self
    }

    /// Bind the resolved context providers used when the driver respawns.
    ///
    /// Peer of [`Self::with_middleware_chain`]. An absent list is a passthrough.
    #[must_use]
    pub fn with_context_providers(mut self, providers: Arc<[Arc<dyn ContextProvider>]>) -> Self {
        self.context_providers = Some(providers);
        self
    }

    /// Override the [`Self::drive_until_wait`] poll bound. Default is 2 s.
    #[must_use]
    pub const fn with_drive_timeout(mut self, timeout: Duration) -> Self {
        self.drive_timeout = timeout;
        self
    }

    /// Current [`Self::drive_until_wait`] poll bound.
    #[must_use]
    pub const fn drive_timeout(&self) -> Duration {
        self.drive_timeout
    }

    /// Durable locator captured from the session handle.
    #[must_use]
    pub fn locator(&self) -> &OperationLocator {
        &self.locator
    }

    /// Tenant scope captured from the session handle.
    #[must_use]
    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    /// Injected clock. Durable sleep advances only through this clock.
    #[must_use]
    pub fn clock(&self) -> &ExternalClock {
        &self.clock
    }

    /// Last recovered kernel state.
    #[must_use]
    pub const fn last_state(&self) -> &KernelState {
        &self.last_state
    }

    /// Drive existing post-commit actions until a wait or terminal.
    ///
    /// # Errors
    ///
    /// Returns recover, spawn, port, or poll-bound failures.
    pub async fn drive_until_wait(&mut self) -> Result<WorkflowWait, WorkflowDriverError> {
        tokio::time::timeout(self.drive_timeout, async {
            loop {
                self.refresh_state().await?;
                if let Some(wait) = classify_wait(&self.last_state) {
                    return Ok(wait);
                }
                if self.owner.is_none() {
                    self.spawn_owner().await?;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .map_err(|_| WorkflowDriverError::DriveTimeout)?
    }

    /// Reopen the journal without continuing the previous owner.
    ///
    /// `SessionRuntime::open` stays inspect-not-continue. Restore is journal
    /// load plus recover. A conflicting checkpoint sequence is ignored; the
    /// journal wins.
    ///
    /// # Errors
    ///
    /// Returns locator or recover failures.
    pub async fn resume(
        store: Arc<dyn JournalStore>,
        locator: OperationLocator,
        clock: ExternalClock,
        random_seed: u64,
        audit: Arc<SecurityAuditGate>,
        hint: Option<&WorkflowCheckpoint>,
    ) -> Result<Self, WorkflowDriverError> {
        let session = Self::attach(store, locator, clock, random_seed, audit).await?;
        let journal_seq = session.last_state.last_applied_sequence;
        let _ = resolve_checkpoint_sequence(
            journal_seq,
            hint.map(|checkpoint| checkpoint.last_applied_seq),
        );
        Ok(session)
    }

    /// Route one authenticated external completion through the existing ingress.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowDriverError::UnknownLocator`] when the command locator
    /// does not match the session handle, and ingress failures otherwise.
    pub async fn complete_external(
        &self,
        command: ExternalEffectCompletionCommand,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, WorkflowDriverError> {
        self.require_locator(&command.locator)?;
        let router =
            ExternalCompletionRouter::new(Arc::clone(&self.store), Arc::clone(&self.audit));
        Box::pin(router.route(command, submitted_at))
            .await
            .map_err(WorkflowDriverError::Ingress)
    }

    /// Route one authenticated interaction resolution through the existing ingress.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowDriverError::UnknownLocator`] when the command locator
    /// does not match the session handle, and ingress failures otherwise.
    pub async fn resolve_interaction(
        &self,
        command: InteractionResolutionCommand,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, WorkflowDriverError> {
        self.require_locator(&command.locator)?;
        let router = InteractionRouter::new(Arc::clone(&self.store), Arc::clone(&self.audit));
        router
            .route(command, submitted_at)
            .await
            .map_err(WorkflowDriverError::Ingress)
    }

    /// Kernel-authoritative retry decision for one outstanding effect.
    #[must_use]
    pub fn retry_decision(
        &self,
        effect_id: EffectId,
        model: Option<&ModelCapabilities>,
        tool: Option<&ToolSpec>,
    ) -> WorkflowRetryDecision {
        retry_decision(&self.last_state, effect_id, model, tool)
    }

    /// Persistence handoff written only after the wait-producing commit.
    ///
    /// # Errors
    ///
    /// Returns [`WorkflowDriverError::Recover`] when identities are missing.
    pub fn persist_handoff(&self) -> Result<WorkflowCheckpoint, WorkflowDriverError> {
        let session_id = self
            .last_state
            .session_id
            .ok_or(WorkflowDriverError::Recover {
                code: "missing_session",
            })?;
        let lane_id = self
            .last_state
            .lane_id
            .ok_or(WorkflowDriverError::Recover {
                code: "missing_lane",
            })?;
        let run_id = self
            .last_state
            .accepted
            .as_ref()
            .map_or(self.locator.run_id, finstack_ai_kernel::RunAccepted::run_id);
        Ok(WorkflowCheckpoint {
            tenant_scope: Arc::clone(&self.tenant_scope),
            session_id,
            lane_id,
            run_id,
            last_applied_seq: self.last_state.last_applied_sequence,
            external_handles: checkpoint_handles(&self.last_state),
        })
    }

    /// Drop the driving owner without touching the journal.
    pub fn abort_owner(&mut self) {
        self.owner = None;
    }

    fn require_locator(&self, locator: &OperationLocator) -> Result<(), WorkflowDriverError> {
        if locator.tenant_scope.as_ref() != self.tenant_scope.as_ref()
            || locator.session_id != self.locator.session_id
            || locator.lane_id != self.locator.lane_id
            || locator.run_id != self.locator.run_id
        {
            return Err(WorkflowDriverError::UnknownLocator);
        }
        Ok(())
    }

    async fn refresh_state(&mut self) -> Result<(), WorkflowDriverError> {
        let coordinator =
            CommitCoordinator::recover(Arc::clone(&self.store), self.locator.session_id)
                .await
                .map_err(|error| recover_error(&error))?;
        if coordinator
            .state()
            .accepted
            .as_ref()
            .is_none_or(|accepted| accepted.run_id() != self.locator.run_id)
        {
            return Err(WorkflowDriverError::UnknownLocator);
        }
        self.last_state = coordinator.state().clone();
        Ok(())
    }

    /// Spawn [`RunTaskOwner`] when the journal is still in the model/tool loop.
    ///
    /// # Errors
    ///
    /// Returns recover, spawn, or port failures.
    pub async fn ensure_owner(&mut self) -> Result<(), WorkflowDriverError> {
        self.refresh_state().await?;
        if self.owner.is_none() && classify_wait(&self.last_state).is_none() {
            self.spawn_owner().await?;
        }
        Ok(())
    }

    /// Respawn [`RunTaskOwner`] after a park, including journal-authoritative waits.
    ///
    /// Ports must already be bound with [`Self::with_ports`]. Bind
    /// [`Self::with_middleware_chain`] and [`Self::with_context_providers`]
    /// when the recovered run should keep those owners.
    ///
    /// # Errors
    ///
    /// Returns recover, spawn, or port failures.
    pub async fn respawn_owner(&mut self) -> Result<(), WorkflowDriverError> {
        self.refresh_state().await?;
        if self.owner.is_none() {
            self.spawn_owner().await?;
        }
        Ok(())
    }

    /// Whether this driver currently owns a live [`RunTaskOwner`].
    #[must_use]
    pub const fn owner_is_live(&self) -> bool {
        self.owner.is_some()
    }

    async fn spawn_owner(&mut self) -> Result<(), WorkflowDriverError> {
        let model = match self.model.as_ref() {
            Some(WorkflowModel::Ready(model)) => Arc::clone(model),
            Some(WorkflowModel::Unprepared(model)) => {
                let ready = ReadyModel::prepare_with_context(
                    Arc::clone(model),
                    ModelWarmupContext {
                        cancellation: crate::CancellationSignal::new(),
                        deadline: None,
                        metadata: crate::Metadata::empty(),
                    },
                )
                .await
                .map_err(|_| WorkflowDriverError::Spawn {
                    code: "model_warmup_failed",
                })?;
                let ready = Arc::new(ready);
                self.model = Some(WorkflowModel::Ready(Arc::clone(&ready)));
                ready
            }
            None => return Err(WorkflowDriverError::PortsRequired),
        };
        let profile = self
            .profile
            .clone()
            .ok_or(WorkflowDriverError::PortsRequired)?;
        let mut coordinator =
            CommitCoordinator::recover(Arc::clone(&self.store), self.locator.session_id)
                .await
                .map_err(|error| recover_error(&error))?;
        if coordinator
            .state()
            .accepted
            .as_ref()
            .is_none_or(|accepted| accepted.run_id() != self.locator.run_id)
        {
            return Err(WorkflowDriverError::UnknownLocator);
        }
        reinstall_runtime_ports(
            &mut coordinator,
            self.capability_owners.clone(),
            self.middleware_chain.clone(),
            self.context_providers.clone(),
        );
        let run_config = RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: Duration::from_millis(500),
            approval_grant: self.approval_grant,
        };
        let model_config = ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: crate::ModelStreamLimits::default(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        };
        let owner = if let Some(catalog) = self.catalog.clone() {
            RunTaskOwner::spawn_with_model_and_tools(
                coordinator,
                run_config,
                model_config,
                ToolTaskConfig {
                    job_capacity: 8,
                    result_capacity: 8,
                    global_max_concurrency: 2,
                    stream_limits: ToolStreamLimits::default(),
                },
                model,
                profile,
                catalog,
                self.clock.clone(),
                self.random.clone(),
            )
            .await
        } else {
            RunTaskOwner::spawn_with_model(
                coordinator,
                run_config,
                model_config,
                model,
                profile,
                self.clock.clone(),
                self.random.clone(),
            )
            .await
        }
        .map_err(|error| WorkflowDriverError::Spawn {
            code: spawn_code(&error),
        })?;
        self.owner = Some(owner);
        Ok(())
    }
}

fn recover_error(error: &crate::CommitCoordinatorError) -> WorkflowDriverError {
    WorkflowDriverError::Recover {
        code: error.stable_code(),
    }
}

fn reinstall_runtime_ports(
    coordinator: &mut CommitCoordinator,
    capability_owners: Option<Arc<BTreeMap<ComponentId, Arc<[CapabilityId]>>>>,
    middleware_chain: Option<Arc<ResolvedMiddlewareChain>>,
    context_providers: Option<Arc<[Arc<dyn ContextProvider>]>>,
) {
    if let Some(owners) = capability_owners {
        coordinator.install_capability_owners(owners);
    }
    if let Some(chain) = middleware_chain {
        coordinator.install_middleware_chain(chain);
    }
    if let Some(providers) = context_providers {
        coordinator.install_context_providers(providers);
    }
}

fn spawn_code(error: &crate::RunHandleError) -> &'static str {
    match error {
        crate::RunHandleError::InvalidConfiguration => "invalid_configuration",
        crate::RunHandleError::ShuttingDown => "shutting_down",
        crate::RunHandleError::Stopped => "stopped",
        crate::RunHandleError::Faulted { code } => code,
        crate::RunHandleError::IntakeClosed => "intake_closed",
        crate::RunHandleError::Coordinator(_) => "coordinator",
        crate::RunHandleError::Model { .. } | crate::RunHandleError::ModelSettlement { .. } => {
            "model"
        }
        crate::RunHandleError::Tool { .. } | crate::RunHandleError::ToolSettlement { .. } => "tool",
        crate::RunHandleError::InteractionSettlement { .. } => "interaction",
        crate::RunHandleError::Timer { .. } => "timer",
        crate::RunHandleError::CancellationSettlement { .. } => "cancellation",
        crate::RunHandleError::EventDelivery { .. } => "event_delivery",
        crate::RunHandleError::Middleware { .. } => "middleware",
    }
}

#[cfg(test)]
#[expect(
    clippy::field_reassign_with_default,
    reason = "tests assemble kernel state incrementally"
)]
mod tests;
