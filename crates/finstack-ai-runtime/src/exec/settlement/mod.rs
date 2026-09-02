//! Shared commit-settlement helpers for native and host-driven run owners.

mod cancel;
mod context;
mod ids;
mod interaction;
mod model;
mod nested_sample;
#[cfg(any(feature = "native-tokio", test))]
mod poll;
mod stage;
mod tool;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ArtifactRef, EventId, Id, IdTag, InteractionId, InteractionKind, InteractionTerminal,
    InteractionTerminalOutcome, OperationLocator, Sensitivity, StageCursor, ToolCallId,
};

use crate::coordinator::{
    CommitCoordinator, CommitCoordinatorError, CommitOutcome, ModelDispatchSeed, ToolDispatchSeed,
};
use crate::ids::{Clock, IdGenerationError, RandomSource, UuidV7Generator};
use crate::ports::model::{
    ApprovalGrantMode, CancellationSignal, LockedModelContextProfile,
    MODEL_RECONCILIATION_UNSUPPORTED, Model, ModelContextProfileOverride, ModelError,
    ModelRequestDraft, ModelResumeAction, ModelTerminal, resolve_model_context_profile,
};
use crate::ports::tool::{
    ApprovalState, ResolvedToolCatalog, TOOL_RECONCILIATION_UNSUPPORTED, ToolError,
    ToolResumeAction,
};
use crate::run_types::RunHandleError;
use crate::tool::AssembledToolTerminal;

pub(crate) use cancel::drain_idle_cancellation;
#[cfg(feature = "native-tokio")]
pub(crate) use cancel::reconcile_cancelled_effect;
#[cfg_attr(
    feature = "native-tokio",
    allow(
        unused_imports,
        reason = "host-task recover consumes this under wasm-host"
    )
)]
pub(crate) use context::resume_pending_context_effects;
pub(crate) use interaction::apply_interaction_resume;
pub(crate) use model::{process_model_progress, process_model_result, resume_pending_model_effect};
#[cfg(any(feature = "native-tokio", test))]
pub(crate) use poll::due_polls;
#[cfg(test)]
pub(crate) use poll::{DuePoll, expired};
#[cfg(feature = "native-tokio")]
pub(crate) use poll::{drive_due_polls, next_due_poll_or_expiry};
pub(crate) use stage::{prepare_tool_batch_if_ready, stage_allocation};
pub(crate) use tool::{
    ToolResultDisposition, continue_after_interaction, parked_tool_continue, process_tool_progress,
    process_tool_result, resume_pending_tool_effects,
};

pub(crate) struct ModelDriverResult {
    pub(crate) seed: ModelDispatchSeed,
    pub(crate) draft: ModelRequestDraft,
    pub(crate) provider: Arc<str>,
    pub(crate) result: Result<ModelTerminal, ModelError>,
}

pub(crate) struct ToolDriverResult {
    pub(crate) seed: ToolDispatchSeed,
    pub(crate) result: Result<AssembledToolTerminal, ToolError>,
}

pub(crate) struct NestedSamplingPorts {
    pub(crate) model: Arc<dyn Model>,
    pub(crate) profile: LockedModelContextProfile,
    pub(crate) catalog: Arc<ResolvedToolCatalog>,
    pub(crate) cancellation: CancellationSignal,
}

/// Live, process-local approval grants for one run owner.
///
/// Not kernel state. Crash recovery rebuilds the ledger from committed
/// `InteractionRequested` metadata (`tool_call_ids`) paired with later
/// approval terminals. A missing or unreadable pair stays unpaid and
/// re-prompts — never an unapproved execute.
struct ApprovalGrantLedger {
    mode: ApprovalGrantMode,
    cursor: Option<StageCursor>,
    granted: BTreeSet<ToolCallId>,
    refused: BTreeSet<ToolCallId>,
    last_parked: Option<Vec<ToolCallId>>,
    consumed_terminal: Option<InteractionId>,
}

impl ApprovalGrantLedger {
    fn new(mode: ApprovalGrantMode) -> Self {
        Self {
            mode,
            cursor: None,
            granted: BTreeSet::new(),
            refused: BTreeSet::new(),
            last_parked: None,
            consumed_terminal: None,
        }
    }

    fn reset_if_cursor_changed(&mut self, cursor: StageCursor) {
        if self.cursor == Some(cursor) {
            return;
        }
        self.cursor = Some(cursor);
        self.granted.clear();
        self.refused.clear();
        self.last_parked = None;
        self.consumed_terminal = None;
    }

    fn state_for(&self, id: &ToolCallId) -> ApprovalState {
        if self.granted.contains(id) {
            ApprovalState::Granted
        } else if self.refused.contains(id) {
            ApprovalState::Refused
        } else {
            ApprovalState::Unpaid
        }
    }
}

pub(crate) struct SettlementSources<C, R> {
    clock: Arc<C>,
    random: R,
    progress_random: ProgressRandom,
    nested_sampling: Option<NestedSamplingPorts>,
    approval: Mutex<ApprovalGrantLedger>,
    artifact_store: Option<Arc<dyn crate::artifact::ArtifactStore>>,
    artifact_locator: Option<OperationLocator>,
}

impl<C: Clock, R: RandomSource> SettlementSources<C, R> {
    pub(crate) fn try_new(clock: C, random: R) -> Result<Self, RunHandleError> {
        let progress_random = ProgressRandom::try_new(&random)?;
        Ok(Self {
            clock: Arc::new(clock),
            random,
            progress_random,
            nested_sampling: None,
            approval: Mutex::new(ApprovalGrantLedger::new(ApprovalGrantMode::PerCall)),
            artifact_store: None,
            artifact_locator: None,
        })
    }

    pub(crate) fn attach_artifact_store(
        &mut self,
        store: Arc<dyn crate::artifact::ArtifactStore>,
        locator: OperationLocator,
    ) {
        self.artifact_store = Some(store);
        self.artifact_locator = Some(locator);
    }

    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) async fn reconcile_recovered_artifacts(
        &self,
        coordinator: &CommitCoordinator,
    ) -> Result<(), RunHandleError> {
        let Some(locator) = self.artifact_locator.as_ref() else {
            return Ok(());
        };
        let loaded = coordinator
            .journal_store()
            .load(crate::ports::journal::LoadRequest {
                session_id: locator.session_id,
            })
            .await
            .map_err(|_| RunHandleError::Artifact {
                code: "artifact_recovery_journal_load_failed",
            })?;
        for record in loaded
            .committed_batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .filter(|record| record.run_id() == Some(locator.run_id))
        {
            if let finstack_ai_kernel::RecordBody::EffectCompleted(completion) = record.body() {
                self.pin_committed_artifacts(locator, completion.artifacts())
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn pin_committed_artifacts(
        &self,
        locator: &OperationLocator,
        artifacts: &[ArtifactRef],
    ) -> Result<(), RunHandleError> {
        if artifacts.is_empty() {
            return Ok(());
        }
        let store = self
            .artifact_store
            .as_ref()
            .ok_or(RunHandleError::Artifact {
                code: "artifact_store_missing",
            })?;
        let owner =
            crate::artifact::ArtifactOwnerId::try_new(format!("journal:{}", locator.session_id))
                .map_err(|error| RunHandleError::Artifact { code: error.code() })?;
        for artifact in artifacts {
            let scope = [
                Sensitivity::Public,
                Sensitivity::Internal,
                Sensitivity::Confidential,
                Sensitivity::Secret,
                Sensitivity::Credential,
            ]
            .into_iter()
            .flat_map(|sensitivity| {
                [Some(locator.run_id), None].map(|run_id| (sensitivity, run_id))
            })
            .map(|(sensitivity, run_id)| crate::artifact::ArtifactScope {
                tenant_scope: Arc::clone(&locator.tenant_scope),
                session_id: locator.session_id,
                run_id,
                sensitivity,
            })
            .find(|scope| scope.digest().ok() == Some(artifact.scope_digest()))
            .ok_or(RunHandleError::Artifact {
                code: crate::artifact::ARTIFACT_SCOPE_MISMATCH,
            })?;
            store
                .pin(scope, artifact.clone(), owner.clone())
                .await
                .map_err(|error| RunHandleError::Artifact { code: error.code() })?;
        }
        Ok(())
    }

    pub(crate) fn set_approval_grant(&self, mode: ApprovalGrantMode) {
        self.lock_approval().mode = mode;
    }

    pub(crate) fn approval_grant(&self) -> ApprovalGrantMode {
        self.lock_approval().mode
    }

    pub(crate) fn prepare_approval_cursor(&self, cursor: StageCursor) {
        self.lock_approval().reset_if_cursor_changed(cursor);
    }

    pub(crate) fn needs_journaled_approval_absorb(
        &self,
        remaining_paid_len: usize,
        terminal: Option<&InteractionTerminal>,
        cursor: StageCursor,
    ) -> bool {
        let Some(terminal) = terminal else {
            return false;
        };
        if terminal.kind != InteractionKind::Approval || terminal.cursor != cursor {
            return false;
        }
        let ledger = self.lock_approval();
        ledger.last_parked.is_none()
            && ledger.consumed_terminal != Some(terminal.interaction_id)
            && ledger.mode == ApprovalGrantMode::PerCall
            && remaining_paid_len > 1
    }

    pub(crate) fn absorb_approval_terminal(
        &self,
        terminal: Option<&InteractionTerminal>,
        cursor: StageCursor,
        remaining_paid: &[ToolCallId],
        journaled: &[(ToolCallId, InteractionTerminalOutcome)],
    ) {
        let Some(terminal) = terminal else {
            return;
        };
        if terminal.kind != InteractionKind::Approval || terminal.cursor != cursor {
            return;
        }
        let mut ledger = self.lock_approval();
        if ledger.consumed_terminal == Some(terminal.interaction_id) {
            return;
        }
        if let Some(ids) = ledger.last_parked.take() {
            apply_approval_ids(&mut ledger, &ids, terminal.outcome);
        } else if ledger.mode == ApprovalGrantMode::InformedBatch || remaining_paid.len() == 1 {
            apply_approval_ids(&mut ledger, remaining_paid, terminal.outcome);
        } else if !journaled.is_empty() {
            for (id, outcome) in journaled {
                apply_approval_ids(&mut ledger, std::slice::from_ref(id), *outcome);
            }
        }
        ledger.consumed_terminal = Some(terminal.interaction_id);
    }

    pub(crate) fn approval_state(&self, id: &ToolCallId) -> ApprovalState {
        self.lock_approval().state_for(id)
    }

    pub(crate) fn record_parked_approval(&self, ids: Vec<ToolCallId>) {
        self.lock_approval().last_parked = Some(ids);
    }

    fn lock_approval(&self) -> std::sync::MutexGuard<'_, ApprovalGrantLedger> {
        self.approval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn attach_nested_sampling(&mut self, ports: NestedSamplingPorts) {
        self.nested_sampling = Some(ports);
    }

    pub(crate) fn nested_sampling(&self) -> Option<&NestedSamplingPorts> {
        self.nested_sampling.as_ref()
    }

    pub(crate) fn now(&self) -> Result<finstack_ai_kernel::Timestamp, RunHandleError> {
        self.clock.now().map_err(id_source_error)
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn clock(&self) -> Arc<C> {
        Arc::clone(&self.clock)
    }

    pub(crate) fn generate<T: IdTag>(&self) -> Result<Id<T>, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.random)
            .generate()
            .map_err(id_source_error)
    }

    pub(crate) fn generate_progress_event(&self) -> Result<EventId, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.progress_random)
            .generate()
            .map_err(id_source_error)
    }
}

fn apply_approval_ids(
    ledger: &mut ApprovalGrantLedger,
    ids: &[ToolCallId],
    outcome: InteractionTerminalOutcome,
) {
    match outcome {
        InteractionTerminalOutcome::Granted => {
            for id in ids {
                if !ledger.refused.contains(id) {
                    ledger.granted.insert(*id);
                }
            }
        }
        InteractionTerminalOutcome::Denied
        | InteractionTerminalOutcome::Expired
        | InteractionTerminalOutcome::Cancelled => {
            for id in ids {
                ledger.granted.remove(id);
                ledger.refused.insert(*id);
            }
        }
    }
}

struct ProgressRandom {
    seed: [u8; 32],
    counter: AtomicU64,
}

impl ProgressRandom {
    fn try_new(random: &impl RandomSource) -> Result<Self, RunHandleError> {
        let mut seed = [0_u8; 32];
        random.fill_bytes(&mut seed).map_err(id_source_error)?;
        Ok(Self {
            seed,
            counter: AtomicU64::new(0),
        })
    }
}

impl RandomSource for ProgressRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let mut written = 0_usize;
        while written < bytes.len() {
            let counter = self
                .counter
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| {
                    IdGenerationError::Source("progress event entropy exhausted".into())
                })?;
            let mut material = [0_u8; 40];
            material[..32].copy_from_slice(&self.seed);
            material[32..].copy_from_slice(&counter.to_be_bytes());
            let digest = finstack_ai_kernel::Digest::raw_json(&material);
            let count = (bytes.len() - written).min(digest.as_bytes().len());
            bytes[written..written + count].copy_from_slice(&digest.as_bytes()[..count]);
            written += count;
        }
        Ok(())
    }
}

pub(crate) fn model_handle_error(error: &ModelError) -> RunHandleError {
    RunHandleError::Model {
        code: Arc::from(error.code().as_str()),
    }
}

/// Classify a model resume action without owning the native or host dispatcher.
pub(crate) fn model_resume_retry_seed(
    action: ModelResumeAction,
    pending_seed: Option<ModelDispatchSeed>,
) -> Result<Option<ModelDispatchSeed>, RunHandleError> {
    match action {
        ModelResumeAction::Retry => pending_seed
            .ok_or(RunHandleError::ModelSettlement {
                code: "model_resume_seed_missing",
            })
            .map(Some),
        ModelResumeAction::SuspendUncertain => Err(RunHandleError::Model {
            code: Arc::from(MODEL_RECONCILIATION_UNSUPPORTED),
        }),
        ModelResumeAction::NoOutstanding
        | ModelResumeAction::UseRecorded
        | ModelResumeAction::Reconcile
        | ModelResumeAction::WaitExternal => Ok(None),
    }
}

/// Classify a tool resume action without owning the native or host dispatcher.
pub(crate) fn tool_resume_retry_seeds(
    action: ToolResumeAction,
    pending_seeds: Vec<ToolDispatchSeed>,
) -> Result<Option<Vec<ToolDispatchSeed>>, RunHandleError> {
    match action {
        ToolResumeAction::Retry => Ok(Some(pending_seeds)),
        ToolResumeAction::SuspendUncertain => Err(RunHandleError::Tool {
            code: Arc::from(TOOL_RECONCILIATION_UNSUPPORTED),
        }),
        ToolResumeAction::NoOutstanding
        | ToolResumeAction::UseRecorded
        | ToolResumeAction::Reconcile
        | ToolResumeAction::WaitExternal => Ok(None),
    }
}

pub(crate) fn tool_handle_error(error: &ToolError) -> RunHandleError {
    RunHandleError::Tool {
        code: Arc::from(error.code().as_str()),
    }
}

pub(crate) fn validate_model_binding(
    model: &dyn Model,
    profile: &LockedModelContextProfile,
) -> Result<(), RunHandleError> {
    let descriptor = model.descriptor();
    descriptor
        .validate()
        .map_err(|error| model_handle_error(&error))?;
    if descriptor.provider != profile.profile.provider
        || !descriptor.models.contains(&profile.profile.model)
    {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::ports::model::MODEL_PROFILE_INVALID),
        });
    }
    let provider = model.capabilities(&profile.profile.model).context_profile;
    let effective = &profile.profile;
    let overlay = ModelContextProfileOverride {
        hard_input_bytes: Some(effective.hard_input_bytes),
        context_window_tokens: Some(effective.context_window_tokens),
        max_output_tokens: Some(effective.max_output_tokens),
        reserved_output_tokens: Some(effective.reserved_output_tokens),
        provider_overhead_tokens: Some(effective.provider_overhead_tokens),
    };
    let relocked = resolve_model_context_profile(provider, Some(&overlay), None, false)
        .map_err(|error| model_handle_error(&error))?;
    if relocked != *profile {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::ports::model::MODEL_PROFILE_INVALID),
        });
    }
    Ok(())
}

pub(crate) fn id_source_error(_error: IdGenerationError) -> RunHandleError {
    RunHandleError::ModelSettlement {
        code: "model_settlement_id_source_failed",
    }
}

/// Map one commit result, surfacing a post-commit dispatch fault as an error.
pub(super) fn committed(
    result: Result<CommitOutcome, CommitCoordinatorError>,
) -> Result<(), RunHandleError> {
    match result.map_err(RunHandleError::Coordinator)?.fault {
        Some(fault) => Err(RunHandleError::Faulted { code: fault.code }),
        None => Ok(()),
    }
}
