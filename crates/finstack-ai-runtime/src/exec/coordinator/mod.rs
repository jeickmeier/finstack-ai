//! Authoritative decide, append, apply, and post-commit dispatch coordination.

mod dispatch;
mod recover;
mod session_commit;
mod submit;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{
    CapabilityId, CommittedBatch, ComponentId, Diagnostic, Digest, EffectId, EventId, Kernel,
    KernelState, ModelTextDelta, ProviderHeartbeat, ReasoningDelta, RunEvent, RunEventBody, RunId,
    Sensitivity, SessionProjection, Timestamp, ToolProgress,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
use finstack_ai_kernel::{Decision, KernelError, KernelInput, TransitionEnv};
#[cfg(all(test, not(any(feature = "native-tokio", feature = "wasm-host"))))]
use finstack_ai_kernel::{KernelInput, TransitionEnv};
use thiserror::Error;

use crate::{
    ContextProvider, JournalStore, ModelProgress, ResolvedMiddlewareChain, SnapshotSchedule,
    StoreError,
};

pub(crate) use dispatch::PostCommitDispatcher;
#[cfg(feature = "native-tokio")]
pub(crate) use dispatch::TimerDispatchSeed;
#[cfg(any(feature = "native-tokio", feature = "wasm-host", test))]
pub(crate) use dispatch::cancel_registered_effect;
#[cfg(any(feature = "native-tokio", feature = "wasm-host", test))]
#[allow(
    unused_imports,
    reason = "wasm host-task dispatcher consumes the context seed"
)]
pub(crate) use dispatch::{
    ContextDispatchSeed, DispatchError, ModelDispatchSeed, RuntimeDispatch, ToolDispatchSeed,
};
pub(crate) use recover::project_loaded;

/// Stable run-local fault state owned by a commit coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunFault {
    /// Stable non-secret fault code.
    pub code: &'static str,
}

/// Result of one accepted coordinator submission.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    /// Atomic committed batch, absent for an empty duplicate decision.
    pub committed: Option<CommittedBatch>,
    /// Events derived transactionally from the committed batch.
    pub events: Arc<[RunEvent]>,
    /// Non-semantic diagnostics returned by the pure decision.
    pub diagnostics: Arc<[Diagnostic]>,
    /// Number of post-commit actions successfully dispatched.
    pub dispatched_actions: usize,
    /// Post-commit fault, when the durable boundary succeeded but dispatch could not.
    pub fault: Option<RunFault>,
}

/// Commit-loop failures that do not have a fully applied outcome to return.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommitCoordinatorError {
    /// A durable composition identity already exists with different content.
    #[error("durable composition sidecar conflicts with existing identity")]
    SidecarConflict,
    /// Run was already faulted by an uncertain prior boundary.
    #[error("run faulted: {code}")]
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
    /// Pure kernel decision rejected the normalized input.
    #[error("kernel decision rejected input: {code}")]
    Decision {
        /// Stable kernel error code.
        code: &'static str,
    },
    /// A configured effect driver rejected a request before any append.
    #[error("model request rejected before commit: {code}")]
    ModelRequest {
        /// Stable adapter error code.
        code: Arc<str>,
    },
    /// The environment did not provide exactly one append identity.
    #[error("non-empty decision requires exactly one append batch id")]
    AppendBatchIdCardinality,
    /// A definite pre-commit store failure occurred.
    #[error(transparent)]
    Store(StoreError),
    /// Store/replay/apply uncertainty faulted the run.
    #[error("commit boundary faulted: {code}")]
    BoundaryFault {
        /// Stable fault code.
        code: &'static str,
    },
    /// The installed runtime event hub failed after apply and before dispatch.
    #[error("event delivery faulted: {code}")]
    EventDelivery {
        /// Stable event-delivery error code.
        code: &'static str,
    },
}

impl CommitCoordinatorError {
    /// Stable fault code for session commit and workflow recover.
    #[must_use]
    pub(crate) fn stable_code(&self) -> &'static str {
        match self {
            Self::Decision { code }
            | Self::Faulted { code }
            | Self::BoundaryFault { code }
            | Self::EventDelivery { code } => code,
            Self::SidecarConflict => "sidecar_conflict",
            Self::AppendBatchIdCardinality => "append_batch_id_cardinality",
            Self::Store(_) => "store_failure",
            Self::ModelRequest { .. } => "model_request",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplayScope {
    Primary,
    StructuralOnly,
    Run(RunId),
}

/// One-run coordinator for the authoritative commit-before-effect path.
pub struct CommitCoordinator {
    kernel: Kernel,
    session: SessionProjection,
    store: Arc<dyn JournalStore>,
    next_transient_sequence: u64,
    pending_timer_scheduled_at: Option<Timestamp>,
    snapshot_schedule: SnapshotSchedule,
    last_snapshot_sequence: Option<u64>,
    head_checksum: Option<Digest>,
    fault: Option<RunFault>,
    last_store_reason: Option<Arc<str>>,
    dispatcher: Option<Arc<dyn PostCommitDispatcher>>,
    #[cfg(feature = "native-tokio")]
    manual_drive: Option<crate::native::manual_drive::ManualDriveGate>,
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    event_publisher: Option<Arc<dyn crate::event_hub::RuntimeEventPublisher>>,
    replay_scope: ReplayScope,
    middleware_chain: Option<Arc<ResolvedMiddlewareChain>>,
    capability_owners: Option<Arc<BTreeMap<ComponentId, CapabilityId>>>,
    context_providers: Option<Arc<[Arc<dyn ContextProvider>]>>,
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    context_projection: Option<
        std::collections::BTreeMap<
            finstack_ai_kernel::EntryId,
            (bool, finstack_ai_kernel::Sensitivity),
        >,
    >,
    last_model_continuation: Option<finstack_ai_kernel::RawJson>,
}

impl CommitCoordinator {
    /// Construct an empty coordinator over a direct journal-store handle.
    #[must_use]
    pub fn new(store: Arc<dyn JournalStore>) -> Self {
        Self {
            kernel: Kernel::default(),
            session: SessionProjection::default(),
            store,
            next_transient_sequence: 0,
            pending_timer_scheduled_at: None,
            snapshot_schedule: SnapshotSchedule::default(),
            last_snapshot_sequence: None,
            head_checksum: None,
            fault: None,
            last_store_reason: None,
            dispatcher: None,
            #[cfg(feature = "native-tokio")]
            manual_drive: None,
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            event_publisher: None,
            replay_scope: ReplayScope::Primary,
            middleware_chain: None,
            capability_owners: None,
            context_providers: None,
            #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
            context_projection: None,
            last_model_continuation: None,
        }
    }

    /// Replace the default snapshot write policy.
    #[must_use]
    pub fn with_snapshot_schedule(mut self, schedule: SnapshotSchedule) -> Self {
        self.snapshot_schedule = schedule;
        self
    }

    /// Opaque provider continuation from the last successful model settlement.
    #[must_use]
    pub const fn last_model_continuation(&self) -> Option<&finstack_ai_kernel::RawJson> {
        self.last_model_continuation.as_ref()
    }

    /// Borrow replay-derived semantic state.
    #[must_use]
    pub const fn state(&self) -> &KernelState {
        self.kernel.state()
    }

    /// Journal store used to commit and recover this coordinator.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    #[must_use]
    pub(crate) fn journal_store(&self) -> &Arc<dyn JournalStore> {
        &self.store
    }

    /// Borrow the rebuilt session projection. Not part of `kernel-state`.
    #[must_use]
    pub const fn session(&self) -> &SessionProjection {
        &self.session
    }

    /// Checksum of the last applied envelope, when the journal is non-empty.
    #[must_use]
    pub const fn head_checksum(&self) -> Option<Digest> {
        self.head_checksum
    }

    /// Current run-local runtime fault.
    #[must_use]
    pub const fn fault(&self) -> Option<RunFault> {
        self.fault
    }

    /// Redacted store `Display` retained from the last integrity/corruption fault.
    ///
    /// The public boundary code stays `store_integrity_uncertain`.
    #[must_use]
    pub fn last_store_reason(&self) -> Option<&str> {
        self.last_store_reason.as_deref()
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn classify(
        &self,
        env: &TransitionEnv,
        input: KernelInput,
    ) -> Result<Decision, KernelError> {
        self.kernel.decide(env, input)
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn install_dispatcher(&mut self, dispatcher: Arc<dyn PostCommitDispatcher>) {
        self.dispatcher = Some(dispatcher);
    }

    /// Pause every committed execute/cancel action immediately before external dispatch.
    ///
    /// Test harness only. This deterministic native mode preserves normal
    /// validation, append, apply, event publication, and dispatch precondition
    /// checks, then stalls each post-commit action with no timeout until the
    /// exclusive controller releases it. Do not use in production.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ManualDriveError::ZeroCapacity`] when `capacity` is zero.
    #[doc(hidden)]
    #[cfg(feature = "native-tokio")]
    pub fn enable_manual_drive(
        &mut self,
        capacity: usize,
    ) -> Result<crate::ManualDriveController, crate::ManualDriveError> {
        let (gate, controller) = crate::native::manual_drive::manual_drive(capacity)?;
        self.manual_drive = Some(gate);
        Ok(controller)
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn install_event_publisher(
        &mut self,
        publisher: Arc<dyn crate::event_hub::RuntimeEventPublisher>,
    ) {
        self.event_publisher = Some(publisher);
    }

    /// Install the resolved middleware chain the runtime worker drives stage
    /// boundaries through.
    ///
    /// Additive public peer of `install_dispatcher` and
    /// `install_event_publisher`, both `pub(crate)` and therefore
    /// unreachable from the facade crate. This one must be `pub`: the chain
    /// lives in the facade (`ResolvedRunPlan::middleware_chain`) but
    /// `RunTaskConfig` is `Copy` and cannot itself carry an `Arc`, so the
    /// facade hands the chain over by calling this directly on the
    /// coordinator it already owns, once, before spawning the run worker.
    /// Unconditional (not feature-gated): both the native-tokio and
    /// wasm-host workers read it back through [`Self::middleware_chain`].
    pub fn install_middleware_chain(&mut self, chain: Arc<ResolvedMiddlewareChain>) {
        self.middleware_chain = Some(chain);
    }

    /// The middleware chain installed by [`Self::install_middleware_chain`],
    /// if any.
    ///
    /// `None` means no chain was installed for this run. Every stage-driver
    /// hook must treat an absent chain as an exact passthrough: proceed
    /// exactly as if no middleware were configured.
    #[must_use]
    pub fn middleware_chain(&self) -> Option<&Arc<ResolvedMiddlewareChain>> {
        self.middleware_chain.as_ref()
    }

    /// Install the lock-time capability-to-component ownership map.
    pub fn install_capability_owners(&mut self, owners: Arc<BTreeMap<ComponentId, CapabilityId>>) {
        self.capability_owners = Some(owners);
    }

    /// Whether a lock-time component is live under the current kernel mask.
    #[must_use]
    pub fn component_is_active(&self, component: &ComponentId) -> bool {
        let Some(owners) = self.capability_owners.as_ref() else {
            return true;
        };
        let Some(owner) = owners.get(component) else {
            return true;
        };
        self.state()
            .active_capabilities
            .iter()
            .any(|item| &item.capability_id == owner)
    }

    /// Install the resolved `ContextProvider` list the runtime drives at
    /// `prepare_context` / `before_model`.
    ///
    /// Additive public peer of [`Self::install_middleware_chain`]. The facade
    /// hands over `ResolvedRunPlan::context_providers` once, before spawning
    /// the run worker. An absent or empty list is a passthrough: no provider
    /// is invoked and source-entry `protected` still follows the structural
    /// projection (system/developer and the trailing current user).
    pub fn install_context_providers(&mut self, providers: Arc<[Arc<dyn ContextProvider>]>) {
        self.context_providers = Some(providers);
    }

    /// The context providers installed by [`Self::install_context_providers`].
    #[must_use]
    pub fn context_providers(&self) -> Option<&Arc<[Arc<dyn ContextProvider>]>> {
        self.context_providers.as_ref()
    }

    /// Store the authoritative `protected` projection for `BeforeModel`.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn store_context_projection(
        &mut self,
        projection: std::collections::BTreeMap<
            finstack_ai_kernel::EntryId,
            (bool, finstack_ai_kernel::Sensitivity),
        >,
    ) {
        self.context_projection = Some(projection);
    }

    /// Authoritative `protected` / sensitivity map from the last context collect.
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    #[must_use]
    pub(crate) fn context_projection(
        &self,
    ) -> Option<
        &std::collections::BTreeMap<
            finstack_ai_kernel::EntryId,
            (bool, finstack_ai_kernel::Sensitivity),
        >,
    > {
        self.context_projection.as_ref()
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) async fn publish_events(
        &mut self,
        events: Arc<[RunEvent]>,
    ) -> Result<(), CommitCoordinatorError> {
        let Some(publisher) = self.event_publisher.clone() else {
            return Ok(());
        };
        publisher.publish(events).await.map_err(|error| {
            self.fault = Some(RunFault { code: error.code });
            CommitCoordinatorError::EventDelivery { code: error.code }
        })
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn materialize_model_progress(
        &mut self,
        progress: &ModelProgress,
        event_id: EventId,
        provider: &str,
        now: Timestamp,
    ) -> Result<RunEvent, &'static str> {
        let pending = self
            .kernel
            .state()
            .pending_model_effect
            .as_ref()
            .ok_or("model_progress_without_pending_effect")?;
        let state = self.kernel.state();
        let accepted = state
            .accepted
            .as_ref()
            .ok_or("model_progress_without_accepted_run")?;
        let session_id = state.session_id.ok_or("model_progress_without_session")?;
        let lane_id = state.lane_id.ok_or("model_progress_without_lane")?;
        let effect_id = pending.requested.effect_id();
        let transient_sequence = self.next_transient_sequence;
        let body = match progress {
            ModelProgress::Text(text) => RunEventBody::ModelTextDelta(
                ModelTextDelta::try_new(text).map_err(|_| "model_progress_invalid")?,
            ),
            ModelProgress::Reasoning(text) => RunEventBody::ReasoningDelta(
                ReasoningDelta::try_new(text).map_err(|_| "model_progress_invalid")?,
            ),
            ModelProgress::Heartbeat(metadata) => RunEventBody::ProviderHeartbeat(
                ProviderHeartbeat::try_new(provider, Some(metadata.as_str()))
                    .map_err(|_| "model_progress_invalid")?,
            ),
        };
        let event = RunEvent::try_transient(
            finstack_ai_kernel::RUN_EVENT_SCHEMA_VERSION,
            finstack_ai_kernel::RUN_EVENT_KIND_VERSION,
            event_id,
            session_id,
            lane_id,
            accepted.run_id(),
            Some(pending.turn_id),
            Some(pending.model_request_id),
            None,
            Some(effect_id),
            None,
            transient_sequence,
            now,
            Sensitivity::Confidential,
            body,
        )
        .map_err(|_| "model_progress_invalid")?;
        self.next_transient_sequence = self
            .next_transient_sequence
            .checked_add(1)
            .ok_or("transient_event_sequence_exhausted")?;
        Ok(event)
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn materialize_tool_progress(
        &mut self,
        progress: &ToolProgress,
        event_id: EventId,
        effect_id: EffectId,
        now: Timestamp,
    ) -> Result<RunEvent, &'static str> {
        let state = self.kernel.state();
        let accepted = state
            .accepted
            .as_ref()
            .ok_or("tool_progress_without_accepted_run")?;
        let session_id = state.session_id.ok_or("tool_progress_without_session")?;
        let lane_id = state.lane_id.ok_or("tool_progress_without_lane")?;
        let batch = state
            .active_tool_batch
            .as_ref()
            .ok_or("tool_progress_without_active_batch")?;
        let call = batch
            .calls
            .iter()
            .find(|call| call.assigned.effect_id == effect_id)
            .ok_or("tool_progress_without_active_call")?;
        let run_id = accepted.run_id();
        let turn_id = batch.opened.turn_id;
        let tool_batch_id = batch.opened.tool_batch_id;
        let tool_call_id = *call.assigned.plan.call().tool_call_id();
        let transient_sequence = self.next_transient_sequence;
        let event = RunEvent::try_transient(
            finstack_ai_kernel::RUN_EVENT_SCHEMA_VERSION,
            finstack_ai_kernel::RUN_EVENT_KIND_VERSION,
            event_id,
            session_id,
            lane_id,
            run_id,
            Some(turn_id),
            None,
            Some(tool_batch_id),
            Some(effect_id),
            Some(tool_call_id),
            transient_sequence,
            now,
            Sensitivity::Confidential,
            RunEventBody::ToolProgress(progress.clone()),
        )
        .map_err(|_| "tool_progress_invalid")?;
        self.next_transient_sequence = self
            .next_transient_sequence
            .checked_add(1)
            .ok_or("transient_event_sequence_exhausted")?;
        Ok(event)
    }

    #[cfg(test)]
    fn with_test_dispatcher(
        store: Arc<dyn JournalStore>,
        dispatcher: Arc<dyn PostCommitDispatcher>,
    ) -> Self {
        let mut coordinator = Self::new(store);
        coordinator.install_dispatcher(dispatcher);
        coordinator
    }
}
