use std::sync::Arc;

use crate::CapabilityActivation;
use finstack_ai_kernel::{
    AcceptRun, ActiveCapability, CapabilitiesActivated, CapabilityActivationSource, KernelInput,
    LaneId, OperationLocator, OutputConfiguration, OutputEndStrategy, OutputSpec, OutputValidated,
    RawJson, ReducerStageOutcome, RetryClassification, RetryDirective, RetrySafety, RunAccepted,
    RunPhase, SessionId, Stage, TerminalState,
};
use finstack_ai_runtime::{LoadRequest, LockedModelContextProfile, RunHandle};

use super::Agent;
use super::prepare::{
    NativeIds, StageIds, ensure_nonterminal_failure, model_draft, model_output_contract,
    recover_state, structured_candidate, submit, submit_stage, wait_for_phase,
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
        store: Arc<dyn finstack_ai_runtime::JournalStore>,
        session_id: SessionId,
        lane_id: LaneId,
        accepted: RunAccepted,
        request: AgentRunRequest,
        profile: LockedModelContextProfile,
        locator: OperationLocator,
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
        let active = lock
            .capabilities
            .iter()
            .filter(|capability| capability.active)
            .map(|capability| ActiveCapability {
                capability_id: capability.id.clone(),
                source: match capability.activation {
                    CapabilityActivation::Always => CapabilityActivationSource::Always,
                    CapabilityActivation::Application => CapabilityActivationSource::Application,
                    CapabilityActivation::Model => CapabilityActivationSource::Model,
                    CapabilityActivation::Disabled => {
                        unreachable!("a disabled capability cannot be active in a validated lock")
                    }
                },
            })
            .collect::<Vec<_>>();
        if !active.is_empty() {
            submit(
                handle,
                NativeIds::environment(1, 0, 0, 0, 0, 0)?,
                KernelInput::CapabilitiesActivated(CapabilitiesActivated {
                    prior_plan_digest: None,
                    resolved_plan_digest: lock.fingerprint().map_err(|error| {
                        AgentRunError::configuration(
                            AGENT_RUN_INVALID_CONFIGURATION,
                            error.to_string(),
                        )
                    })?,
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
            let state = recover_state(Arc::clone(&store), session_id).await?;
            if state.cycle >= request.max_cycles {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "run exceeded max_cycles before producing a final response",
                ));
            }
            let messages = self.context_messages(&request.input, &state.messages)?;
            submit_stage(
                handle,
                state.cycle,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared { messages },
                StageIds::context(),
            )
            .await?;
            let state = recover_state(Arc::clone(&store), session_id).await?;
            let draft = model_draft(
                request.model.clone(),
                state
                    .current_turn
                    .as_ref()
                    .map(|turn| Arc::clone(&turn.context.messages))
                    .ok_or_else(|| AgentRunError::runtime_message("prepared context is missing"))?,
                self.tools.tools().map(|tool| tool.spec.clone()).collect(),
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
                Arc::clone(&store),
                session_id,
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
                after_model = recover_state(Arc::clone(&store), session_id).await?;
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
                    Arc::clone(&store),
                    session_id,
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
                let retry = wait_for_phase(
                    handle,
                    Arc::clone(&store),
                    session_id,
                    &[
                        RunPhase::PreparingContext,
                        RunPhase::Failed,
                        RunPhase::Cancelled,
                    ],
                )
                .await?;
                ensure_nonterminal_failure(&retry)?;
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
            let terminal = recover_state(Arc::clone(&store), session_id).await?;
            let TerminalState::Completed(completed) = terminal
                .terminal
                .as_ref()
                .ok_or_else(|| AgentRunError::runtime_message("terminal state is missing"))?
            else {
                return Err(AgentRunError::runtime_message(
                    "run did not complete successfully",
                ));
            };
            let message = terminal
                .messages
                .iter()
                .find(|message| message.id() == &completed.result_message_id)
                .cloned()
                .ok_or_else(|| AgentRunError::runtime_message("result message is missing"))?;
            let loaded = store
                .load(LoadRequest { session_id })
                .await
                .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
            let record_kinds = loaded
                .committed_batches
                .iter()
                .flat_map(|batch| batch.records.iter())
                .map(|record| Arc::from(record.body().kind_name()))
                .collect::<Vec<_>>();
            return Ok(AgentRunOutput {
                locator,
                message,
                retry_attempts: terminal.retry.attempts,
                active_capabilities: terminal.active_capabilities.clone(),
                record_kinds: record_kinds.into(),
            });
        }
    }
}
