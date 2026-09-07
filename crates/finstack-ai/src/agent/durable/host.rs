//! Embedded host composition and lifecycle; stage decisions live in `drive`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, ComponentId, ComponentRef, KernelInput, KernelState, OperationLocator, RunId,
    Timestamp,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ids::ExternalClock;
use finstack_ai_runtime::ports::journal::{JournalStore, StoreLimits};
use finstack_ai_runtime::workflow::{WorkflowWait, classify_wait};
use finstack_ai_store_sqlite::{SqliteDurability, SqliteJournalStore, SqliteStoreConfig};
use finstack_ai_workflow_hitl::{
    HitlInboxStore, HitlLifecycle, HitlRouter, InteractionRow, ResolutionInput, SqliteHitlStore,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, PortsFactory, RecoveryStore, SqliteWorkerStore, TickReport,
    WakeIndexStore, WakeReason, WakeRow, WorkerBuilder, WorkflowExecution, WorkflowWorker,
};

use super::super::prepare::{NativeIds, append_lane_input, create_session_runtime};
use super::super::{Agent, AgentRunRequest, PREVIEW_ENGINE_VERSION};
use super::DurableHostError;
use super::definition::Definition;
use super::descriptor::Descriptor;

/// Construction boundary for a host-owned journal and registered definitions.
pub struct DurableHostBuilder {
    tenant_scope: Arc<str>,
    journal: Arc<dyn JournalStore>,
    adapters: Arc<SqliteWorkerStore>,
    hitl: Arc<SqliteHitlStore>,
    definitions: BTreeMap<Arc<str>, Arc<Definition>>,
    worker_id: Arc<str>,
    drive_timeout: Duration,
    lease_ttl_ms: u64,
}

impl DurableHostBuilder {
    /// Open durable `SQLite` journal, worker and interaction tables on `path`.
    /// Registered agents execute against this host journal. Artifact stores
    /// and credentials remain supplied by the registered application.
    ///
    /// # Errors
    /// Returns invalid tenant, schema, durability or storage errors.
    pub fn try_open(
        tenant_scope: &str,
        path: impl AsRef<Path>,
        limits: StoreLimits,
    ) -> Result<Self, DurableHostError> {
        if tenant_scope.is_empty() || tenant_scope.as_bytes().contains(&0) {
            return Err(DurableHostError::new("durable_tenant_invalid"));
        }
        let journal: Arc<dyn JournalStore> = Arc::new(
            SqliteJournalStore::try_open(SqliteStoreConfig::new(
                path.as_ref(),
                SqliteDurability::Durable,
                limits,
            ))
            .map_err(|error| DurableHostError::new(error.code()))?,
        );
        Ok(Self {
            tenant_scope: Arc::from(tenant_scope),
            journal,
            adapters: Arc::new(SqliteWorkerStore::try_open(path.as_ref())?),
            hitl: Arc::new(SqliteHitlStore::try_open(path.as_ref())?),
            definitions: BTreeMap::new(),
            worker_id: Arc::from(format!("host-{}", std::process::id())),
            drive_timeout: Duration::from_secs(30),
            lease_ttl_ms: 60_000,
        })
    }

    /// Host journal handle for applications using explicit bundle resolution.
    /// Registering a resolved agent already bound to this handle preserves it.
    #[must_use]
    pub fn journal_store(&self) -> Arc<dyn JournalStore> {
        Arc::clone(&self.journal)
    }

    /// Set the process lease identity. Each simultaneously live host must use
    /// a distinct nonempty identity.
    #[must_use]
    pub fn worker_id(mut self, worker_id: impl Into<Arc<str>>) -> Self {
        self.worker_id = worker_id.into();
        self
    }

    /// Bound one leased execution segment. Defaults to thirty seconds.
    /// The budget must be positive and shorter than the lease TTL.
    #[must_use]
    pub const fn drive_timeout(mut self, timeout: Duration) -> Self {
        self.drive_timeout = timeout;
        self
    }

    /// Lease TTL in milliseconds. Defaults to sixty seconds.
    #[must_use]
    pub const fn lease_ttl_ms(mut self, ttl: u64) -> Self {
        self.lease_ttl_ms = ttl;
        self
    }

    /// Resolve an application definition against the host-owned journal.
    /// Its other component handles, media/artifact configuration and model
    /// credentials are preserved. Workflow kinds are immutable within a host.
    ///
    /// # Errors
    /// Returns duplicate/invalid kinds, unbuildable definitions or lock errors.
    pub async fn register(
        mut self,
        workflow_kind: &str,
        agent: Agent,
    ) -> Result<Self, DurableHostError> {
        if workflow_kind.is_empty()
            || workflow_kind.len() > 128
            || workflow_kind.as_bytes().contains(&0)
        {
            return Err(DurableHostError::new("durable_workflow_kind_invalid"));
        }
        if self.definitions.contains_key(workflow_kind) {
            return Err(DurableHostError::new("durable_workflow_kind_duplicate"));
        }
        let resolved = if Arc::ptr_eq(agent.resolved.run_plan().store().handle(), &self.journal) {
            agent
        } else {
            let mut builder = agent
                .rebuild
                .as_ref()
                .ok_or_else(|| DurableHostError::new("durable_definition_not_rebuildable"))?
                .as_ref()
                .clone();
            builder.store = (
                ComponentRef::new(
                    ComponentId::parse("finstack.host.journal")
                        .map_err(|_| DurableHostError::new("durable_store_identity"))?,
                    Some(PREVIEW_ENGINE_VERSION),
                ),
                Arc::clone(&self.journal),
            );
            let mut rebuilt = builder.build().await?;
            rebuilt
                .structured_output
                .clone_from(&agent.structured_output);
            rebuilt.history_cache_policy = agent.history_cache_policy;
            rebuilt
        };
        let kind: Arc<str> = Arc::from(workflow_kind);
        let definition = Definition {
            kind: Arc::clone(&kind),
            agent: resolved,
            journal: Arc::clone(&self.journal),
            recovery: Arc::clone(&self.adapters) as Arc<dyn RecoveryStore>,
        };
        self.definitions.insert(kind, Arc::new(definition));
        Ok(self)
    }

    /// Finish construction without driving accepted work.
    ///
    /// # Errors
    /// Returns invalid worker configuration or an empty definition catalogue.
    pub fn build(self) -> Result<DurableHost, DurableHostError> {
        if self.definitions.is_empty() {
            return Err(DurableHostError::new("durable_definitions_empty"));
        }
        let clock = ExternalClock::new(NativeIds::now()?);
        let mut worker = WorkerBuilder::new(
            Arc::clone(&self.journal),
            Arc::new(MemoryCronStore::new()),
            Arc::clone(&self.adapters) as Arc<dyn WakeIndexStore>,
            Arc::clone(&self.adapters) as Arc<dyn FireStore>,
            Arc::clone(&self.adapters) as Arc<dyn InboxStore>,
        )
        .drive_timeout(self.drive_timeout)
        .lease_ttl_ms(self.lease_ttl_ms)
        .tenant_scope(&self.tenant_scope)
        .worker_id(&self.worker_id)
        .clock(clock.clone())
        .system_clock()
        .interaction_lifecycle(Arc::new(HitlLifecycle::new(
            Arc::clone(&self.hitl) as Arc<dyn HitlInboxStore>
        )));
        for (kind, definition) in &self.definitions {
            worker = worker
                .register_ports(kind, Arc::clone(definition) as Arc<dyn PortsFactory>)
                .register_execution(kind, Arc::clone(definition) as Arc<dyn WorkflowExecution>);
        }
        let worker = Arc::new(worker.build()?);
        let router = HitlRouter::new(
            Arc::clone(&self.hitl) as Arc<dyn HitlInboxStore>,
            Arc::clone(&worker),
            Arc::clone(&self.adapters) as Arc<dyn WakeIndexStore>,
        );
        Ok(DurableHost {
            tenant_scope: self.tenant_scope,
            journal: self.journal,
            adapters: self.adapters,
            definitions: self.definitions,
            worker,
            router: Arc::new(router),
            hitl: self.hitl,
            clock,
            closed: Arc::new(AtomicBool::new(false)),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            recovery_cursor: Arc::new(std::sync::Mutex::new(None)),
        })
    }
}

/// Journal-derived inspection of an admitted run.
#[derive(Debug, Clone)]
pub struct DurableInspection {
    /// Exact original execution identity.
    pub locator: OperationLocator,
    /// Application definition required to continue it.
    pub workflow_kind: Arc<str>,
    /// Authoritative replay state including authority, limits and effect state.
    pub state: KernelState,
    /// Next wait when the run is parked or terminal.
    pub wait: Option<WorkflowWait>,
}

/// Embedded host. Explicit ticks own and join all local execution.
///
/// A fresh process opens the same path, registers the same definitions, and
/// ticks to recover admitted work. No original request needs to be retained.
#[derive(Clone)]
pub struct DurableHost {
    tenant_scope: Arc<str>,
    journal: Arc<dyn JournalStore>,
    adapters: Arc<SqliteWorkerStore>,
    definitions: BTreeMap<Arc<str>, Arc<Definition>>,
    worker: Arc<WorkflowWorker>,
    router: Arc<HitlRouter>,
    hitl: Arc<SqliteHitlStore>,
    clock: ExternalClock,
    closed: Arc<AtomicBool>,
    gate: Arc<tokio::sync::Mutex<()>>,
    recovery_cursor: Arc<std::sync::Mutex<Option<RunId>>>,
}

impl DurableHost {
    fn ensure_open(&self) -> Result<(), DurableHostError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(DurableHostError::new("durable_host_closed"));
        }
        Ok(())
    }
    fn now(&self) -> Result<Timestamp, DurableHostError> {
        let now = NativeIds::now()?;
        self.clock.set(now);
        Ok(now)
    }
    fn definition(&self, kind: &str) -> Result<&Arc<Definition>, DurableHostError> {
        self.definitions
            .get(kind)
            .ok_or_else(|| DurableHostError::new("durable_workflow_kind_unknown"))
    }

    /// Persist context and immutable recovery inputs, accept once, and enqueue.
    /// No model or tool dispatch occurs until a subsequent `tick`.
    ///
    /// # Errors
    /// Returns tenant, definition, admission, descriptor or storage failures.
    pub async fn start(
        &self,
        workflow_kind: &str,
        request: AgentRunRequest,
    ) -> Result<OperationLocator, DurableHostError> {
        Box::pin(self.start_inner(workflow_kind, request)).await
    }

    async fn start_inner(
        &self,
        workflow_kind: &str,
        request: AgentRunRequest,
    ) -> Result<OperationLocator, DurableHostError> {
        let _guard = self.gate.lock().await;
        self.ensure_open()?;
        if request.security.tenant_scope() != self.tenant_scope.as_ref() {
            return Err(DurableHostError::new("durable_tenant_mismatch"));
        }
        if request.capability.is_some() {
            return Err(DurableHostError::new(
                "durable_select_registered_definition",
            ));
        }
        let definition = self.definition(workflow_kind)?;
        let prepared = definition.agent.prepare(request)?;
        let runtime = create_session_runtime(
            Arc::clone(&self.journal),
            &self.tenant_scope,
            prepared.session_id,
            prepared.lane_id,
        )
        .await?;
        let context = append_lane_input(
            &runtime,
            prepared.lane_id,
            prepared.locator.run_id,
            &prepared.request.input,
            &prepared.request.attachments,
        )
        .await?;
        let descriptor = Descriptor {
            output_schema: definition
                .agent
                .structured_output
                .as_ref()
                .map(|output| output.schema_ref.clone()),
            version: 1,
            locator: prepared.locator.clone(),
            workflow_kind: Arc::clone(&definition.kind),
            lock_fingerprint: prepared.accepted.resolved_agent_lock_digest(),
            model: prepared.request.model,
            settings: prepared.request.settings,
            model_profile_digest: prepared.profile.digest,
            source_leaf_id: context.source_leaf_id,
            context_sequence: context.journal_sequence,
            context_checksum: context
                .head_checksum
                .ok_or_else(|| DurableHostError::new("durable_context_missing"))?,
            required_artifacts: prepared
                .request
                .attachments
                .iter()
                .map(|input| input.artifact.clone())
                .collect::<Vec<_>>()
                .into(),
        };
        definition.validate_artifacts(&descriptor).await?;
        descriptor.save(self.adapters.as_ref())?;
        let mut coordinator = runtime
            .coordinator_for_run(Some(prepared.locator.run_id))
            .await
            .map_err(|error| DurableHostError::new(error.code()))?;
        let committed = coordinator
            .submit(
                NativeIds::environment(1, 1, 0, 0, 0, 0)?,
                KernelInput::AcceptRun(AcceptRun {
                    session_id: prepared.session_id,
                    lane_id: prepared.lane_id,
                    accepted: prepared.accepted,
                }),
            )
            .await
            .map_err(|error| DurableHostError::new(error.code()))?;
        if committed.fault.is_some() || committed.dispatched_actions != 0 {
            return Err(DurableHostError::new("durable_admission_fault"));
        }
        runtime.release_run(prepared.lane_id, prepared.locator.run_id);
        self.enqueue(&descriptor, self.now()?)?;
        Ok(prepared.locator)
    }

    fn enqueue(&self, descriptor: &Descriptor, now: Timestamp) -> Result<(), DurableHostError> {
        let locator = &descriptor.locator;
        self.adapters.upsert(&WakeRow {
            tenant_scope: Arc::clone(&locator.tenant_scope),
            session_id: locator.session_id,
            lane_id: locator.lane_id,
            run_id: locator.run_id,
            workflow_kind: Arc::clone(&descriptor.workflow_kind),
            reason: WakeReason::Runnable,
            wake_at: Some(now),
            expires_at: None,
            pending_id: Arc::from(locator.run_id.to_canonical_string()),
            leased_by: None,
            lease_expires_at: None,
            attempts: 0,
        })?;
        Ok(())
    }

    /// Rebuild and validate one run from committed history and its descriptor.
    ///
    /// # Errors
    /// Returns cross-tenant, missing descriptor, configuration drift, missing
    /// artifact, unresolved effect or journal recovery failures explicitly.
    pub async fn inspect(
        &self,
        locator: &OperationLocator,
    ) -> Result<DurableInspection, DurableHostError> {
        if locator.tenant_scope != self.tenant_scope {
            return Err(DurableHostError::new("durable_tenant_mismatch"));
        }
        let descriptor = Descriptor::load(self.adapters.as_ref(), locator)?;
        let definition = self.definition(&descriptor.workflow_kind)?;
        let recovered = CommitCoordinator::recover(Arc::clone(&self.journal), locator.session_id)
            .await
            .map_err(|error| DurableHostError::new(error.code()))?;
        definition.descriptor(locator, recovered.state())?;
        Definition::validate_uncertainty(recovered.state())?;
        definition.validate_artifacts(&descriptor).await?;
        Ok(DurableInspection {
            locator: locator.clone(),
            workflow_kind: descriptor.workflow_kind,
            state: recovered.state().clone(),
            wait: classify_wait(recovered.state()),
        })
    }

    /// Reconcile admission-to-wake crashes and advance due work under leases.
    /// Each tick is bounded by the worker's batch and driving limits.
    ///
    /// # Errors
    /// Returns explicit recovery, configuration or storage failures. Individual
    /// execution failures remain counted in the worker report with wake backoff.
    pub async fn tick(&self) -> Result<TickReport, DurableHostError> {
        // Dropping the caller's await detaches observation; the owned tick
        // retains its lease and joins its local owner before releasing the gate.
        let host = self.clone();
        tokio::spawn(async move { Box::pin(host.tick_owned()).await })
            .await
            .map_err(|_| DurableHostError::new("durable_tick_task_failed"))?
    }

    async fn tick_owned(&self) -> Result<TickReport, DurableHostError> {
        let _guard = self.gate.lock().await;
        self.ensure_open()?;
        let now = self.now()?;
        let after = *self
            .recovery_cursor
            .lock()
            .map_err(|_| DurableHostError::new("durable_host_lock"))?;
        let descriptors = self
            .adapters
            .scan_recovery(&self.tenant_scope, after, 256)?;
        let next = descriptors.last().map(|row| row.locator.run_id);
        for row in descriptors {
            let recovered =
                CommitCoordinator::recover(Arc::clone(&self.journal), row.locator.session_id)
                    .await
                    .map_err(|error| DurableHostError::new(error.code()))?;
            // Retired artifacts or definitions on completed runs do not
            // prevent unrelated live work from progressing.
            if recovered.state().terminal().is_some() {
                continue;
            }
            let inspected = self.inspect(&row.locator).await?;
            if inspected.state.terminal().is_none()
                && !self.adapters.recovery_has_wake(&row.locator)?
            {
                if let Some(wait) = inspected.wait {
                    let mut session = finstack_ai_runtime::workflow::WorkflowSession::trusted(
                        Arc::clone(&self.journal),
                        row.locator.clone(),
                        self.clock.clone(),
                    )
                    .await?;
                    let checkpoint = finstack_ai_workflow_worker::park_for_wake(
                        &mut session,
                        self.adapters.as_ref(),
                        &inspected.workflow_kind,
                    )?;
                    if matches!(&wait, WorkflowWait::Interaction { .. }) {
                        let accepted = inspected
                            .state
                            .accepted()
                            .ok_or_else(|| DurableHostError::new("durable_admission_incomplete"))?;
                        finstack_ai_workflow_hitl::capture(
                            self.hitl.as_ref(),
                            &checkpoint,
                            &wait,
                            accepted.security(),
                            now,
                        )?;
                    }
                } else {
                    let descriptor = Descriptor::load(self.adapters.as_ref(), &row.locator)?;
                    self.enqueue(&descriptor, now)?;
                }
            }
        }
        *self
            .recovery_cursor
            .lock()
            .map_err(|_| DurableHostError::new("durable_host_lock"))? = next;
        // A poisoned wake without a descriptor fails explicitly before dispatch.
        for row in self
            .adapters
            .load_due(now, 256)?
            .into_iter()
            .filter(|row| row.tenant_scope == self.tenant_scope)
        {
            let locator = OperationLocator::try_new(
                &self.tenant_scope,
                row.session_id,
                row.lane_id,
                row.run_id,
            )
            .map_err(|_| DurableHostError::new("durable_locator_mismatch"))?;
            let recovered =
                CommitCoordinator::recover(Arc::clone(&self.journal), locator.session_id)
                    .await
                    .map_err(|error| DurableHostError::new(error.code()))?;
            if recovered.state().terminal().is_none() {
                self.inspect(&locator).await?;
            }
        }
        Ok(Box::pin(self.worker.tick()).await?)
    }

    /// List this host tenant's pending interactions.
    ///
    /// # Errors
    /// Returns interaction-store failures.
    pub fn pending(&self) -> Result<Vec<InteractionRow>, DurableHostError> {
        Ok(self.router.pending(&self.tenant_scope)?)
    }

    /// Buffer an authorized response for the next tick. The principal and
    /// authorization evidence must equal the accepted run's context.
    ///
    /// # Errors
    /// Returns authorization, duplicate/conflict, expiry or storage errors.
    pub fn resolve(
        &self,
        interaction_id: &str,
        input: ResolutionInput,
    ) -> Result<(), DurableHostError> {
        self.ensure_open()?;
        Ok(self
            .router
            .resolve(&self.tenant_scope, interaction_id, input, self.now()?)?)
    }

    /// Reconcile abandoned interaction hints without changing accepted delivery outcomes.
    ///
    /// # Errors
    /// Returns interaction or wake-store failures.
    pub fn sweep_interactions(
        &self,
    ) -> Result<finstack_ai_workflow_hitl::SweepReport, DurableHostError> {
        Ok(self.router.sweep(self.now()?)?)
    }

    /// Stop admitting ticks and wait for local driving and its owner joins.
    /// Accepted runs remain in the journal for another process to recover.
    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        let _guard = self.gate.lock().await;
    }
}
