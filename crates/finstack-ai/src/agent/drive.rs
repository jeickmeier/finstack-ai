use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{CapabilityActivation, InstructionSpec};
use finstack_ai_kernel::{
    AcceptRun, ActiveCapability, CapabilitiesActivated, CapabilityActivationSource, KernelInput,
    LaneId, OperationLocator, OutputConfiguration, OutputEndStrategy, OutputSpec, OutputValidated,
    RawJson, ReducerStageOutcome, RetryClassification, RetryDirective, RetrySafety, RunAccepted,
    RunId, RunPhase, SessionId, Stage, TerminalState,
};
use finstack_ai_runtime::ports::model::{LockedModelContextProfile, ModelName, ModelSettings};
use finstack_ai_runtime::run::{LiveRunState, RunHandle};
use finstack_ai_runtime::session::LaneRunContext;

use super::handle::Agent;
use super::prepare::{
    NativeIds, StageIds, ensure_nonterminal_failure, model_draft, model_output_contract,
    structured_candidate, submit, submit_stage, wait_for_cycle, wait_for_phase,
};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunOutput, AgentRunRequest,
};

/// Non-authority reconstruction inputs for the shared stage driver.
/// Authority, deadline, effect state and budgets remain in the accepted journal.
pub(super) struct StageDriverConfig {
    pub(super) model: ModelName,
    pub(super) settings: ModelSettings,
    pub(super) timeout: std::time::Duration,
    pub(super) max_cycles: u64,
}

impl From<&AgentRunRequest> for StageDriverConfig {
    fn from(request: &AgentRunRequest) -> Self {
        Self {
            model: request.model.clone(),
            settings: request.settings.clone(),
            timeout: request.timeout,
            max_cycles: request.max_cycles,
        }
    }
}

impl Agent {
    #[expect(
        clippy::too_many_arguments,
        reason = "the preview driver keeps the complete aggregate-stage sequence auditable"
    )]
    pub(super) async fn drive(
        &self,
        handle: &RunHandle,
        session_id: SessionId,
        lane_id: LaneId,
        accepted: RunAccepted,
        request: AgentRunRequest,
        profile: LockedModelContextProfile,
        locator: OperationLocator,
        context_seed: LaneRunContext,
    ) -> Result<AgentRunOutput, AgentRunError> {
        if handle.live_state().phase.is_none() {
            submit(
                handle,
                NativeIds::environment(1, 1, 0, 0, 0, 0)?,
                KernelInput::AcceptRun(AcceptRun {
                    session_id,
                    lane_id,
                    accepted,
                }),
            )
            .await?;
        }
        self.drive_existing(
            handle,
            StageDriverConfig::from(&request),
            profile,
            locator,
            context_seed,
        )
        .await
    }

    /// Advance an already accepted run using the same decisions on initial
    /// execution and recovery. This entrypoint never accepts a run or appends input.
    pub(super) async fn drive_existing(
        &self,
        handle: &RunHandle,
        request: StageDriverConfig,
        profile: LockedModelContextProfile,
        locator: OperationLocator,
        context_seed: LaneRunContext,
    ) -> Result<AgentRunOutput, AgentRunError> {
        if handle.live_state().phase.is_none() {
            return Err(AgentRunError::runtime_message(
                "stage driver requires an accepted run",
            ));
        }
        self.settle_before_run(handle, locator.run_id).await?;
        loop {
            let state = handle.live_state();
            if state.terminal.is_some() {
                return output_from_live_state(handle, locator, &state, request.timeout);
            }
            if state.phase == Some(RunPhase::Sleeping) {
                await_retry_cycle(handle, state.cycle, request.timeout).await?;
                continue;
            }
            self.validate_restored_mask(&state.active_capabilities)?;
            if let Some(host) = &self.activation_host {
                host.seed_active(locator.run_id, Arc::clone(&state.active_capabilities));
            }
            if state.cycle >= request.max_cycles {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "run exceeded max_cycles before producing a final response",
                ));
            }
            if state.phase == Some(RunPhase::PreparingContext) {
                self.submit_prepared_context(handle, &state, &context_seed)
                    .await?;
            }
            let state = handle.live_state();
            if state.phase == Some(RunPhase::BeforeModel) {
                self.submit_model_request(handle, &state, &request, &profile)
                    .await?;
            }
            let Some(next) = self
                .settle_model_response(handle, locator.run_id, request.timeout)
                .await?
            else {
                continue;
            };
            if next
                .validation_failure
                .as_ref()
                .is_some_and(|failure| failure.error.retryable)
            {
                retry_structured_output(handle, next.cycle, request.timeout).await?;
                continue;
            }
            if let Some(terminal) = finalize_cycle(handle, next.cycle, request.timeout).await? {
                return output_from_live_state(handle, locator, &terminal, request.timeout);
            }
        }
    }

    async fn settle_before_run(
        &self,
        handle: &RunHandle,
        run_id: RunId,
    ) -> Result<(), AgentRunError> {
        if handle.live_state().phase == Some(RunPhase::BeforeRun) {
            self.configure_accepted_run(handle, run_id).await?;
        }
        if handle.live_state().phase == Some(RunPhase::BeforeRun) {
            submit_stage(
                handle,
                0,
                Stage::BeforeRun,
                ReducerStageOutcome::Continue,
                StageIds::continued(),
            )
            .await?;
        }
        Ok(())
    }

    async fn configure_accepted_run(
        &self,
        handle: &RunHandle,
        run_id: RunId,
    ) -> Result<(), AgentRunError> {
        let lock = self.resolved.lock().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires an exact resolved lock",
            )
        })?;
        let active = initially_active_capabilities(lock)?;
        let lock_digest = lock.fingerprint().map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        if let Some(host) = &self.activation_host {
            host.set_lock_digest(lock_digest);
            host.seed_active(run_id, active.clone().into());
        }
        if !active.is_empty() && handle.live_state().resolved_plan_digest.is_none() {
            submit(
                handle,
                NativeIds::environment(1, 0, 0, 0, 0, 0)?,
                KernelInput::CapabilitiesActivated(CapabilitiesActivated {
                    prior_plan_digest: None,
                    resolved_plan_digest: lock_digest,
                    active: active.into(),
                }),
            )
            .await?;
        }
        if let Some(output) = &self.structured_output {
            submit(
                handle,
                NativeIds::environment(1, 0, 0, 0, 0, 0)?,
                KernelInput::ConfigureOutput(OutputConfiguration {
                    output: OutputSpec::JsonSchema {
                        schema: output.schema_ref.clone(),
                    },
                    end_strategy: OutputEndStrategy::Early,
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn submit_prepared_context(
        &self,
        handle: &RunHandle,
        state: &LiveRunState,
        context_seed: &LaneRunContext,
    ) -> Result<(), AgentRunError> {
        let extra = self.extra_capability_instructions(&state.active_capabilities);
        let messages =
            self.context_messages(context_seed, &state.committed_run_messages, &extra)?;
        submit_stage(
            handle,
            state.cycle,
            Stage::PrepareContext,
            ReducerStageOutcome::ContextPrepared { messages },
            StageIds::context(),
        )
        .await
    }

    async fn submit_model_request(
        &self,
        handle: &RunHandle,
        state: &LiveRunState,
        request: &StageDriverConfig,
        profile: &LockedModelContextProfile,
    ) -> Result<(), AgentRunError> {
        let draft = model_draft(
            request.model.clone(),
            (!state.prepared_context_messages.is_empty())
                .then(|| Arc::clone(&state.prepared_context_messages))
                .ok_or_else(|| AgentRunError::runtime_message("prepared context is missing"))?,
            self.live_tool_specs(&state.active_capabilities),
            self.structured_output
                .as_ref()
                .map_or(OutputSpec::PlainText, |output| OutputSpec::JsonSchema {
                    schema: output.schema_ref.clone(),
                }),
            request.settings.clone(),
            profile,
        )?;
        let request_json = RawJson::parse(draft.canonical_bytes().map_err(AgentRunError::model)?)
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        submit_stage(
            handle,
            state.cycle,
            Stage::BeforeModel,
            ReducerStageOutcome::ModelRequestPrepared {
                request: request_json,
                component: None,
                output_contract: model_output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
            StageIds::model_request(),
        )
        .await
    }

    /// Settle the model response through `after_model`, returning the
    /// `before_finalize` state, or `None` when a tool batch continued the cycle.
    async fn settle_model_response(
        &self,
        handle: &RunHandle,
        run_id: RunId,
        timeout: std::time::Duration,
    ) -> Result<Option<LiveRunState>, AgentRunError> {
        let after_model = wait_for_phase(
            handle,
            &[
                RunPhase::AfterModel,
                RunPhase::AfterToolBatch,
                RunPhase::BeforeFinalize,
                RunPhase::Failed,
                RunPhase::Cancelled,
            ],
        )
        .await?;
        ensure_nonterminal_failure(&after_model, timeout)?;
        if after_model.phase == Some(RunPhase::AfterToolBatch) {
            self.continue_after_tool_batch(handle, after_model.cycle, run_id)
                .await?;
            return Ok(None);
        }
        let after_model = self
            .submit_structured_validation(handle, after_model)
            .await?;
        let next = if after_model.phase == Some(RunPhase::BeforeFinalize) {
            after_model
        } else {
            submit_stage(
                handle,
                after_model.cycle,
                Stage::AfterModel,
                ReducerStageOutcome::Continue,
                StageIds::continued(),
            )
            .await?;
            wait_for_phase(
                handle,
                &[
                    RunPhase::BeforeFinalize,
                    RunPhase::AfterToolBatch,
                    RunPhase::Failed,
                    RunPhase::Cancelled,
                ],
            )
            .await?
        };
        ensure_nonterminal_failure(&next, timeout)?;
        if next.phase == Some(RunPhase::AfterToolBatch) {
            self.continue_after_tool_batch(handle, next.cycle, run_id)
                .await?;
            return Ok(None);
        }
        Ok(Some(next))
    }

    async fn submit_structured_validation(
        &self,
        handle: &RunHandle,
        after_model: LiveRunState,
    ) -> Result<LiveRunState, AgentRunError> {
        let Some(output) = &self.structured_output else {
            return Ok(after_model);
        };
        if after_model.phase != Some(RunPhase::AfterModel) {
            return Ok(after_model);
        }
        let (message_id, candidate, source) =
            structured_candidate(&after_model).ok_or_else(|| {
                AgentRunError::runtime_message(
                    "structured model response did not contain a JSON candidate",
                )
            })?;
        submit(
            handle,
            NativeIds::environment(1, 0, 0, 0, 0, 0)?,
            KernelInput::OutputValidated(OutputValidated {
                message_id,
                schema: output.schema_ref.clone(),
                candidate: candidate.clone(),
                source,
                outcome: output.validator.validate(&candidate),
            }),
        )
        .await?;
        Ok(handle.live_state())
    }

    async fn continue_after_tool_batch(
        &self,
        handle: &RunHandle,
        cycle: u64,
        run_id: RunId,
    ) -> Result<(), AgentRunError> {
        self.submit_pending_activation(handle, run_id).await?;
        submit_stage(
            handle,
            cycle,
            Stage::AfterToolBatch,
            ReducerStageOutcome::Continue,
            StageIds::continued(),
        )
        .await
    }

    fn extra_capability_instructions(&self, active: &[ActiveCapability]) -> Vec<InstructionSpec> {
        let Some(lock) = self.resolved.lock() else {
            return Vec::new();
        };
        let initially_active = lock
            .capabilities
            .iter()
            .filter(|capability| capability.active)
            .map(|capability| capability.id.clone())
            .collect::<BTreeSet<_>>();
        self.capability_specs
            .iter()
            .filter(|spec| {
                active.iter().any(|item| item.capability_id == spec.id)
                    && !initially_active.contains(&spec.id)
            })
            .flat_map(|spec| spec.instructions.iter().cloned())
            .collect()
    }

    async fn submit_pending_activation(
        &self,
        handle: &RunHandle,
        run_id: RunId,
    ) -> Result<(), AgentRunError> {
        let Some(host) = &self.activation_host else {
            return Ok(());
        };
        let Some(complete) = host.take_pending(run_id) else {
            return Ok(());
        };
        self.validate_restored_mask(&complete)?;
        let state = handle.live_state();
        let Some(digest) = host.lock_digest() else {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "capability activation is missing the resolved-plan digest",
            ));
        };
        submit(
            handle,
            NativeIds::environment(1, 0, 0, 0, 0, 0)?,
            KernelInput::CapabilitiesActivated(CapabilitiesActivated {
                prior_plan_digest: state.resolved_plan_digest,
                resolved_plan_digest: digest,
                active: complete.clone().into(),
            }),
        )
        .await?;
        host.seed_active(run_id, complete.into());
        Ok(())
    }
}

fn initially_active_capabilities(
    lock: &crate::ResolvedAgentLock,
) -> Result<Vec<ActiveCapability>, AgentRunError> {
    let mut active = Vec::new();
    for capability in lock
        .capabilities
        .iter()
        .filter(|capability| capability.active)
    {
        let source = match capability.activation {
            CapabilityActivation::Always => CapabilityActivationSource::Always,
            CapabilityActivation::Application => CapabilityActivationSource::Application,
            CapabilityActivation::Model => CapabilityActivationSource::Model,
            CapabilityActivation::Disabled => {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "a disabled capability cannot be active in a validated lock",
                ));
            }
        };
        active.push(ActiveCapability {
            capability_id: capability.id.clone(),
            source,
        });
    }
    Ok(active)
}

async fn retry_structured_output(
    handle: &RunHandle,
    cycle: u64,
    timeout: std::time::Duration,
) -> Result<(), AgentRunError> {
    submit_stage(
        handle,
        cycle,
        Stage::BeforeFinalize,
        ReducerStageOutcome::Retry(
            RetryDirective::try_new(
                RetryClassification::Validation,
                finstack_ai_kernel::Duration::from_millis(1),
                "native-structured-output-v1",
            )
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
        ),
        StageIds::retry(),
    )
    .await?;
    await_retry_cycle(handle, cycle, timeout).await
}

/// Accept finalization, returning the terminal state, or `None` when a
/// `before_finalize` retry landed and the drive loop must take another cycle.
async fn finalize_cycle(
    handle: &RunHandle,
    cycle: u64,
    timeout: std::time::Duration,
) -> Result<Option<LiveRunState>, AgentRunError> {
    submit_stage(
        handle,
        cycle,
        Stage::BeforeFinalize,
        ReducerStageOutcome::FinalizeAccepted,
        StageIds::finalize(),
    )
    .await?;
    let terminal = handle.live_state();
    if terminal.terminal.is_some() {
        return Ok(Some(terminal));
    }
    if matches!(
        terminal.phase,
        Some(RunPhase::Sleeping | RunPhase::PreparingContext)
    ) {
        // A `before_finalize` middleware superseded the submitted
        // `FinalizeAccepted` with a `Retry`: the kernel committed
        // `RetryScheduled` plus a timer effect, and the post-commit
        // action fires the timer. Wait for the timer to land the
        // run back in `PreparingContext` and continue the drive loop.
        //
        // The phase set is a proxy for "a retry landed", exhaustive
        // today because the fold admits only `Fail`/`Retry`
        // terminals at `before_finalize` and a landed retry moves
        // through exactly `Sleeping` then `PreparingContext`. A new
        // phase on that path must be added here too; the error
        // below names the phase so a mismatch is diagnosable.
        await_retry_cycle(handle, terminal.cycle, timeout).await?;
        return Ok(None);
    }
    Err(AgentRunError::runtime_message(format!(
        "finalize settled without a terminal state (phase: {:?})",
        terminal.phase
    )))
}

/// Wait out a `before_finalize` retry: the run must land back in
/// `PreparingContext` (or terminally fail/cancel) before the drive loop
/// takes another cycle. Shared by the structured-output validation retry
/// and the middleware-superseded finalize paths so their wait semantics
/// cannot drift.
async fn await_retry_cycle(
    handle: &RunHandle,
    prior_cycle: u64,
    timeout: std::time::Duration,
) -> Result<(), AgentRunError> {
    let retry = wait_for_cycle(handle, prior_cycle).await?;
    ensure_nonterminal_failure(&retry, timeout)
}

pub(super) fn output_from_live_state(
    handle: &RunHandle,
    locator: OperationLocator,
    state: &finstack_ai_runtime::run::LiveRunState,
    timeout: std::time::Duration,
) -> Result<AgentRunOutput, AgentRunError> {
    let completed = match state
        .terminal
        .as_ref()
        .ok_or_else(|| AgentRunError::runtime_message("terminal state is missing"))?
    {
        TerminalState::Completed(completed) => completed,
        TerminalState::Failed(failed) if failed.error.code.as_ref() == "deadline_exceeded" => {
            return Err(AgentRunError::Timeout { timeout });
        }
        TerminalState::Failed(failed) => {
            return Err(AgentRunError::runtime_message(format!(
                "run failed: {}: {}",
                failed.error.code, failed.error.message
            )));
        }
        TerminalState::Cancelled(_) => return Err(AgentRunError::Cancelled),
    };
    let message = state
        .committed_run_messages
        .iter()
        .find(|message| message.id() == &completed.result_message_id)
        .cloned()
        .ok_or_else(|| AgentRunError::runtime_message("result message is missing"))?;
    Ok(AgentRunOutput {
        locator,
        message,
        retry_attempts: state.retry_attempts,
        active_capabilities: state.active_capabilities.clone(),
        record_kinds: handle.record_kinds(),
    })
}
