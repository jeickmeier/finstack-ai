use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{CapabilityActivation, InstructionSpec};
use finstack_ai_kernel::{
    AcceptRun, ActiveCapability, CapabilitiesActivated, CapabilityActivationSource, KernelInput,
    LaneId, OperationLocator, OutputConfiguration, OutputEndStrategy, OutputSpec, OutputValidated,
    RawJson, ReducerStageOutcome, RetryClassification, RetryDirective, RetrySafety, RunAccepted,
    RunId, RunPhase, SessionId, Stage, TerminalState,
};
use finstack_ai_runtime::{LockedModelContextProfile, RunHandle};

use super::handle::Agent;
use super::prepare::{
    NativeIds, RunContextSeed, StageIds, ensure_nonterminal_failure, model_draft,
    model_output_contract, structured_candidate, submit, submit_stage, wait_for_cycle,
    wait_for_phase,
};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunOutput, AgentRunRequest,
};

impl Agent {
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
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
        context_seed: RunContextSeed,
    ) -> Result<AgentRunOutput, AgentRunError> {
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
        let lock = self.resolved.lock().ok_or_else(|| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires an exact resolved lock",
            )
        })?;
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
        let lock_digest = lock.fingerprint().map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        if let Some(host) = self.activation_host() {
            host.set_lock_digest(lock_digest);
            host.seed_active(locator.run_id, active.clone().into());
        }
        if !active.is_empty() {
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
        submit_stage(
            handle,
            0,
            Stage::BeforeRun,
            ReducerStageOutcome::Continue,
            StageIds::continued(),
        )
        .await?;

        loop {
            let state = handle.live_state();
            self.validate_restored_mask(&state.active_capabilities)?;
            if let Some(host) = self.activation_host() {
                host.seed_active(locator.run_id, Arc::clone(&state.active_capabilities));
            }
            if state.cycle >= request.max_cycles {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "run exceeded max_cycles before producing a final response",
                ));
            }
            let extra = self.extra_capability_instructions(&state.active_capabilities);
            let messages =
                self.context_messages(&context_seed, &state.committed_run_messages, &extra)?;
            submit_stage(
                handle,
                state.cycle,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared { messages },
                StageIds::context(),
            )
            .await?;
            let state = handle.live_state();
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
                &profile,
            )?;
            let request_json =
                RawJson::parse(draft.canonical_bytes().map_err(AgentRunError::model)?)
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
            .await?;

            let mut after_model = wait_for_phase(
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
            ensure_nonterminal_failure(&after_model)?;
            if after_model.phase == Some(RunPhase::AfterToolBatch) {
                self.submit_pending_activation(handle, locator.run_id)
                    .await?;
                submit_stage(
                    handle,
                    after_model.cycle,
                    Stage::AfterToolBatch,
                    ReducerStageOutcome::Continue,
                    StageIds::continued(),
                )
                .await?;
                continue;
            }
            if let Some(output) = &self.structured_output
                && after_model.phase == Some(RunPhase::AfterModel)
            {
                let (message_id, candidate, source) = structured_candidate(&after_model)
                    .ok_or_else(|| {
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
                after_model = handle.live_state();
            }
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
            ensure_nonterminal_failure(&next)?;
            if next.phase == Some(RunPhase::AfterToolBatch) {
                self.submit_pending_activation(handle, locator.run_id)
                    .await?;
                submit_stage(
                    handle,
                    next.cycle,
                    Stage::AfterToolBatch,
                    ReducerStageOutcome::Continue,
                    StageIds::continued(),
                )
                .await?;
                continue;
            }

            if next
                .validation_failure
                .as_ref()
                .is_some_and(|failure| failure.error.retryable)
            {
                submit_stage(
                    handle,
                    next.cycle,
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
                await_retry_cycle(handle, next.cycle).await?;
                continue;
            }

            submit_stage(
                handle,
                next.cycle,
                Stage::BeforeFinalize,
                ReducerStageOutcome::FinalizeAccepted,
                StageIds::finalize(),
            )
            .await?;
            let terminal = handle.live_state();
            if terminal.terminal.is_none() {
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
                    await_retry_cycle(handle, terminal.cycle).await?;
                    continue;
                }
                return Err(AgentRunError::runtime_message(format!(
                    "finalize settled without a terminal state (phase: {:?})",
                    terminal.phase
                )));
            }
            let completed = match terminal
                .terminal
                .as_ref()
                .ok_or_else(|| AgentRunError::runtime_message("terminal state is missing"))?
            {
                TerminalState::Completed(completed) => completed,
                TerminalState::Failed(failed) => {
                    return Err(AgentRunError::runtime_message(format!(
                        "run failed: {}: {}",
                        failed.error.code, failed.error.message
                    )));
                }
                TerminalState::Cancelled(_) => return Err(AgentRunError::Cancelled),
            };
            let message = terminal
                .committed_run_messages
                .iter()
                .find(|message| message.id() == &completed.result_message_id)
                .cloned()
                .ok_or_else(|| AgentRunError::runtime_message("result message is missing"))?;
            return Ok(AgentRunOutput {
                locator,
                message,
                retry_attempts: terminal.retry_attempts,
                active_capabilities: terminal.active_capabilities.clone(),
                record_kinds: handle.record_kinds(),
            });
        }
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
        self.capability_specs()
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
        let Some(host) = self.activation_host() else {
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

/// Wait out a `before_finalize` retry: the run must land back in
/// `PreparingContext` (or terminally fail/cancel) before the drive loop
/// takes another cycle. Shared by the structured-output validation retry
/// and the middleware-superseded finalize paths so their wait semantics
/// cannot drift.
async fn await_retry_cycle(handle: &RunHandle, prior_cycle: u64) -> Result<(), AgentRunError> {
    let retry = wait_for_cycle(handle, prior_cycle).await?;
    ensure_nonterminal_failure(&retry)
}
