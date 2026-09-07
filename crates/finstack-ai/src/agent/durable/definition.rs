//! One resolved application definition bound to worker recovery.

use finstack_ai_kernel::{
    ActiveToolCallStatus, EntryBody, KernelState, OperationLocator, RetrySafety, Sensitivity,
};
use finstack_ai_runtime::artifact::{
    ArtifactScope, artifact_scope_for_locator, get_required_artifact,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ids::Clock;
use finstack_ai_runtime::ports::journal::{JournalStore, LoadRequest};
use finstack_ai_runtime::ports::model::{
    LockedModelContextProfile, Model, ReadyModel, resolve_model_context_profile,
};
use finstack_ai_runtime::workflow::{WorkflowSession, WorkflowWait, classify_wait};
use finstack_ai_workflow_worker::{PortsFactory, RecoveryStore, WorkerError, WorkflowExecution};
use std::sync::Arc;
use std::time::Duration;

use super::super::Agent;
use super::super::drive::StageDriverConfig;
use super::DurableHostError;
use super::descriptor::Descriptor;

pub(super) struct Definition {
    pub(super) kind: Arc<str>,
    pub(super) agent: Agent,
    pub(super) journal: Arc<dyn JournalStore>,
    pub(super) recovery: Arc<dyn RecoveryStore>,
}

impl Definition {
    async fn context_messages(
        &self,
        descriptor: &Descriptor,
    ) -> Result<Arc<[finstack_ai_kernel::Message]>, WorkerError> {
        let recovered =
            CommitCoordinator::recover(Arc::clone(&self.journal), descriptor.locator.session_id)
                .await
                .map_err(|error| {
                    WorkerError::Driver(
                        finstack_ai_runtime::workflow::WorkflowDriverError::Recover {
                            code: error.code(),
                        },
                    )
                })?;
        let history = recovered
            .session()
            .history(descriptor.source_leaf_id)
            .map_err(|_| worker_error(DurableHostError::new("durable_context_missing")))?;
        Ok(history
            .into_iter()
            .map(|entry| match entry.body() {
                EntryBody::Message(message) => message.clone(),
            })
            .collect::<Vec<_>>()
            .into())
    }

    pub(super) fn descriptor(
        &self,
        locator: &OperationLocator,
        state: &KernelState,
    ) -> Result<(Descriptor, Arc<ReadyModel>, LockedModelContextProfile), DurableHostError> {
        let descriptor = Descriptor::load(self.recovery.as_ref(), locator)?;
        let accepted = state
            .accepted()
            .ok_or_else(|| DurableHostError::new("durable_admission_incomplete"))?;
        let fingerprint = self
            .agent
            .resolved
            .lock()
            .ok_or_else(|| DurableHostError::new("durable_lock_missing"))?
            .fingerprint()
            .map_err(|_| DurableHostError::new("durable_lock_invalid"))?;
        if descriptor.workflow_kind != self.kind
            || descriptor.output_schema
                != self
                    .agent
                    .structured_output
                    .as_ref()
                    .map(|output| output.schema_ref.clone())
            || descriptor.lock_fingerprint != fingerprint
            || accepted.resolved_agent_lock_digest() != fingerprint
        {
            return Err(DurableHostError::new("durable_configuration_drift"));
        }
        if accepted.run_id() != locator.run_id
            || state.session_id() != Some(locator.session_id)
            || state.lane_id() != Some(locator.lane_id)
            || accepted.security().tenant_scope() != locator.tenant_scope.as_ref()
        {
            return Err(DurableHostError::new("durable_locator_mismatch"));
        }
        let model = Arc::clone(self.agent.resolved.run_plan().model().handle());
        let profile = resolve_model_context_profile(
            model.capabilities(&descriptor.model).context_profile,
            None,
            None,
            false,
        )
        .map_err(|_| DurableHostError::new("durable_model_profile"))?;
        if profile.digest != descriptor.model_profile_digest {
            return Err(DurableHostError::new("durable_configuration_drift"));
        }
        Ok((descriptor, model, profile))
    }

    pub(super) fn validate_uncertainty(state: &KernelState) -> Result<(), DurableHostError> {
        // A committed non-retryable dispatch with no terminal receipt is
        // uncertainty, even when no usage was recorded. Never replace it.
        if state.active_tool_batch().is_some_and(|batch| batch.calls.iter().any(|call| matches!(&call.status, ActiveToolCallStatus::Requested { requested, deferred: None } if matches!(requested.retry_safety(), RetrySafety::AtMostOnce | RetrySafety::Unknown)))) {
            return Err(DurableHostError::new("durable_effect_uncertain"));
        }
        if state
            .cancellation()
            .is_some_and(|cancel| !cancel.uncertain_effects.is_empty())
        {
            return Err(DurableHostError::new("durable_effect_uncertain"));
        }
        Ok(())
    }

    pub(super) async fn validate_artifacts(
        &self,
        descriptor: &Descriptor,
    ) -> Result<(), DurableHostError> {
        let mut artifacts = descriptor
            .required_artifacts
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let loaded = self
            .journal
            .load(LoadRequest {
                session_id: descriptor.locator.session_id,
            })
            .await
            .map_err(|_| DurableHostError::new("durable_artifact_coverage_unavailable"))?;
        if !loaded
            .committed_batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .any(|record| {
                record.sequence() == descriptor.context_sequence
                    && record.checksum() == descriptor.context_checksum
            })
        {
            return Err(DurableHostError::new(
                "durable_context_coverage_unavailable",
            ));
        }
        super::capability::validate_activation_prefix(
            self.agent.activation_host.as_deref(),
            descriptor.locator.run_id,
            loaded
                .committed_batches
                .iter()
                .flat_map(|batch| batch.records.iter()),
        )?;
        for record in loaded
            .committed_batches
            .iter()
            .flat_map(|batch| batch.records.iter())
        {
            if record.run_id() == Some(descriptor.locator.run_id)
                && let finstack_ai_kernel::RecordBody::EffectCompleted(completed) = record.body()
            {
                artifacts.extend_from_slice(completed.artifacts());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for artifact in artifacts {
            if !seen.insert(artifact.id()) {
                continue;
            }
            let locator = &descriptor.locator;
            let store = self
                .agent
                .artifact_store
                .as_ref()
                .ok_or_else(|| DurableHostError::new("artifact_store_missing"))?;
            let scope = artifact_scope_for_locator(locator, &artifact)
                .or_else(|| {
                    // Python and document ingestion share the canonical tenant-bound
                    // pre-run upload scope. Only an explicitly admitted input ref
                    // can select it; effect outputs remain run/session scoped.
                    let upload = ArtifactScope {
                        tenant_scope: Arc::clone(&locator.tenant_scope),
                        session_id: finstack_ai_kernel::SessionId::from_bytes([0; 16]),
                        run_id: None,
                        sensitivity: Sensitivity::Internal,
                    };
                    if descriptor.required_artifacts.contains(&artifact)
                        && upload.digest().ok() == Some(artifact.scope_digest())
                    {
                        Some(upload)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| DurableHostError::new("artifact_scope_mismatch"))?;
            get_required_artifact(store.as_ref(), scope, artifact)
                .await
                .map_err(|error| DurableHostError::new(error.code()))?;
        }
        Ok(())
    }
}

fn worker_error(error: DurableHostError) -> WorkerError {
    let DurableHostError { code } = error;
    WorkerError::Driver(
        finstack_ai_runtime::workflow::WorkflowDriverError::Recover {
            code: match code.as_ref() {
                "durable_configuration_drift" => "durable_configuration_drift",
                "durable_descriptor_missing" => "durable_descriptor_missing",
                "durable_effect_uncertain" => "durable_effect_uncertain",
                _ => "durable_recovery_failed",
            },
        },
    )
}

impl PortsFactory for Definition {
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError> {
        let (descriptor, model, profile) = self
            .descriptor(session.locator(), session.last_state())
            .map_err(worker_error)?;
        Self::validate_uncertainty(session.last_state()).map_err(worker_error)?;
        if let Some(host) = &self.agent.activation_host {
            host.set_lock_digest(descriptor.lock_fingerprint);
            host.seed_active(
                session.locator().run_id,
                Arc::clone(session.last_state().active_capabilities()),
            );
        }
        let plan = self.agent.resolved.run_plan();
        let providers = plan
            .context_providers()
            .iter()
            .map(|component| Arc::clone(component.handle()))
            .collect::<Vec<_>>()
            .into();
        let observers = plan
            .observers()
            .iter()
            .map(|component| {
                (
                    Arc::clone(component.handle()),
                    super::super::prepare::event_subscription(Sensitivity::Credential),
                )
            })
            .collect();
        let mut bound = session
            .with_ready_ports(model, profile, Some(Arc::clone(&self.agent.tools)))
            .with_middleware_chain(Arc::clone(plan.middleware_chain()))
            .with_context_providers(providers)
            .with_observers(observers)
            .with_capability_owners(self.agent.capability_index.as_arc_owners())
            .with_approval_grant(
                self.agent
                    .resolved
                    .spec()
                    .map(|spec| spec.policy.approval_grant)
                    .unwrap_or_default(),
            );
        if let Some(store) = &self.agent.artifact_store {
            bound = bound.with_artifact_store(Arc::clone(store));
        }
        Ok(bound)
    }
}

impl WorkflowExecution for Definition {
    fn prepare<'a>(
        &'a self,
        session: &'a WorkflowSession,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), WorkerError>> + Send + 'a>>
    {
        Box::pin(async move {
            let (descriptor, _, _) = self
                .descriptor(session.locator(), session.last_state())
                .map_err(worker_error)?;
            self.validate_artifacts(&descriptor)
                .await
                .map_err(worker_error)
        })
    }

    fn advance<'a>(
        &'a self,
        session: &'a mut WorkflowSession,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkflowWait, WorkerError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let locator = session.locator().clone();
            let (descriptor, _, profile) = self
                .descriptor(&locator, session.last_state())
                .map_err(worker_error)?;
            self.validate_artifacts(&descriptor)
                .await
                .map_err(worker_error)?;
            let messages = self.context_messages(&descriptor).await?;
            let accepted = session.last_state().accepted().ok_or_else(|| {
                worker_error(DurableHostError::new("durable_admission_incomplete"))
            })?;
            let config = StageDriverConfig {
                model: descriptor.model.clone(),
                settings: descriptor.settings.clone(),
                timeout: Duration::from_millis(
                    accepted
                        .limits()
                        .max_wall_time
                        .map_or(30_000, finstack_ai_kernel::Duration::as_millis),
                ),
                max_cycles: accepted
                    .limits()
                    .max_model_requests
                    .unwrap_or(super::super::DEFAULT_MAX_CYCLES),
            };
            let handle = session.run_handle().ok_or(WorkerError::Driver(
                finstack_ai_runtime::workflow::WorkflowDriverError::PortsRequired,
            ))?;
            let deadline = accepted.effective_deadline();
            let remaining = deadline.map(|deadline| {
                let now = super::super::prepare::NativeIds::now().map_or(deadline, |now| now);
                Duration::from_millis(
                    u64::try_from(deadline.as_unix_ms().saturating_sub(now.as_unix_ms()))
                        .unwrap_or(0),
                )
            });
            let timeout = config.timeout;
            let drive = async {
                if remaining == Some(Duration::ZERO) {
                    return super::super::prepare::settle_deadline_timeout(
                        &handle,
                        locator.clone(),
                        timeout,
                        deadline,
                    )
                    .await;
                }
                let advancement = self.agent.drive_existing(
                    &handle,
                    config,
                    profile,
                    locator.clone(),
                    descriptor.context(messages),
                );
                if let Some(remaining) = remaining {
                    match tokio::time::timeout(remaining, advancement).await {
                        Ok(result) => result,
                        Err(_) => {
                            super::super::prepare::settle_deadline_timeout(
                                &handle,
                                locator.clone(),
                                timeout,
                                deadline,
                            )
                            .await
                        }
                    }
                } else {
                    advancement.await
                }
            };
            let wait = async {
                loop {
                    session.ensure_owner().await?;
                    if let Some(wait) = current_wait(session)? {
                        return Ok(wait);
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            };
            tokio::select! {
                biased;
                result = wait => result,
                result = drive => {
                    session.ensure_owner().await?;
                    if let Some(wait) = current_wait(session)? { Ok(wait) }
                    else { Err(worker_error(result.err().map_or_else(|| DurableHostError::new("durable_drive_not_parked"), DurableHostError::from))) }
                }
            }
        })
    }
}

fn current_wait(session: &WorkflowSession) -> Result<Option<WorkflowWait>, WorkerError> {
    let now = session
        .clock()
        .now()
        .map_err(|_| WorkerError::TimeOverflow)?;
    Ok(classify_wait(session.last_state())
        .filter(|wait| !matches!(wait, WorkflowWait::Timer { due_at, .. } if *due_at <= now)))
}
