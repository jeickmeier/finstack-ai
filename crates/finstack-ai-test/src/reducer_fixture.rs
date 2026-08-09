//! Real native Rust adapter for PR-009 model-only golden traces.

use std::sync::Arc;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendBatchId, AppendBatchTag, BudgetPropagation,
    CancellationPropagation, CommittedBatch, ComponentId, ContentBlock, DeadlinePropagation,
    Decision, Digest, EffectCompleted, EffectDeferred, EffectOutputContract, EffectOutputKind,
    EventId, EventTag, ExternalEffectCompletedInput, ExternalEffectCompletion,
    ExternalEffectOutcome, ExternalHandleRef, Id, IdTag, Kernel, KernelInput, KernelState, LaneId,
    Message, MessageId, MessageRole, MessageTag, Metadata, ModelSettled, ModelSettlement,
    ModelTextDelta, PostCommitAction, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson,
    ReconciliationPolicy, RecordDraft, RecordEnvelope, RecordTag, ReducerStageOutcome, RetrySafety,
    RunAccepted, RunEvent, RunEventBody, RunEventClass, RunLimits, RunPropagationPolicy,
    RunRelation, RunSecurityContext, Sensitivity, SessionId, Stage, StageCursor, StageSettled,
    TextBlock, Timestamp, TransitionEnv, TurnTag,
};
use serde::Deserialize;
use serde_json::json;

use crate::conformance::{AdapterCapability, AdapterOutcome, ConformanceAdapter, TargetKind};
use crate::scripted_model::{ScriptedStep, ScriptedStepKind};
use crate::trace_fixture::{
    DurabilityClass, EffectExpectation, ExpectedTrace, GoldenTrace, NormalizedEvent, TraceError,
    TraceRecord, normalize_json_value,
};

const DEFAULT_SESSION_ID: &str = "01234567-89ab-7cde-89ab-012345678901";
const DEFAULT_LANE_ID: &str = "01234567-89ab-7cde-89ab-012345678902";
const DEFAULT_RUN_ID: &str = "01234567-89ab-7cde-89ab-012345678903";
const DEFAULT_USER_MESSAGE_ID: &str = "01234567-89ab-7cde-89ab-012345678904";
/// Native adapter that executes PR-009 fixtures through the real kernel reducer.
///
/// The adapter consumes only fixture inputs and deterministic transition values.
/// It never reads `GoldenTrace::expected` while constructing observed output.
///
#[derive(Debug, Default)]
pub struct ReducerRustAdapter;
/// Detailed reducer execution retained for replay-focused conformance tests.
#[derive(Debug, Clone, PartialEq)]
pub struct ReducerExecution {
    /// Observed conformance projection.
    pub observed: ExpectedTrace,
    /// Ordered batches committed during the execution.
    pub committed_batches: Vec<CommittedBatch>,
    /// Authoritative state from the live reducer execution.
    pub kernel_state: KernelState,
}
/// Terminal state projection shared by execution and committed-record replay.
#[derive(Debug, Clone, PartialEq)]
pub struct ReducerTerminalProjection {
    /// Stable compact terminal state.
    pub final_state: serde_json::Value,
    /// Serialized terminal result.
    pub final_result: serde_json::Value,
    /// Real kernel semantic state hash.
    pub state_hash: String,
}
impl ConformanceAdapter for ReducerRustAdapter {
    fn target(&self) -> TargetKind {
        TargetKind::Rust
    }

    fn capability(&self) -> AdapterCapability {
        AdapterCapability::Available
    }

    fn execute(&self, trace: &GoldenTrace) -> Result<AdapterOutcome, TraceError> {
        let observed = execute_reducer_trace(trace)?.observed;
        let value =
            serde_json::to_value(observed).map_err(|error| adapter_error(error.to_string()))?;
        Ok(AdapterOutcome::Observed(value))
    }
}
#[derive(Debug, Clone, Copy, Default)]
struct EnvRequirements {
    records: usize,
    events: usize,
    effects: usize,
    messages: usize,
    turns: usize,
    model_requests: usize,
}
impl EnvRequirements {
    const fn new(
        records: usize,
        events: usize,
        effects: usize,
        messages: usize,
        turns: usize,
        model_requests: usize,
    ) -> Self {
        Self {
            records,
            events,
            effects,
            messages,
            turns,
            model_requests,
        }
    }
}
struct FixtureValues<'a> {
    timestamps_ms: &'a [u64],
    ids: &'a [String],
    timestamp_index: usize,
    id_index: usize,
}
impl<'a> FixtureValues<'a> {
    const fn new(timestamps_ms: &'a [u64], ids: &'a [String]) -> Self {
        Self {
            timestamps_ms,
            ids,
            timestamp_index: 0,
            id_index: 0,
        }
    }

    fn take_env(&mut self, required: EnvRequirements) -> Result<TransitionEnv, TraceError> {
        let now_ms = *self
            .timestamps_ms
            .get(self.timestamp_index)
            .ok_or_else(|| adapter_error("transition_env.timestamps_ms exhausted"))?;
        self.timestamp_index += 1;
        let now_ms = i64::try_from(now_ms)
            .map_err(|_| adapter_error("transition timestamp does not fit i64"))?;
        let now = Timestamp::from_unix_ms(now_ms)
            .map_err(|error| adapter_error(format!("invalid transition timestamp: {error}")))?;

        let record_ids = self.take_ids::<RecordTag>(required.records, "record_ids")?;
        let event_ids = self.take_ids::<EventTag>(required.events, "event_ids")?;
        let effect_ids =
            self.take_ids::<finstack_ai_kernel::EffectTag>(required.effects, "effect_ids")?;
        let turn_ids = self.take_ids::<TurnTag>(required.turns, "turn_ids")?;
        let model_request_ids = self.take_ids::<finstack_ai_kernel::ModelRequestTag>(
            required.model_requests,
            "model_request_ids",
        )?;
        let message_ids = self.take_ids::<MessageTag>(required.messages, "message_ids")?;
        let append_batch_ids = self.take_ids::<AppendBatchTag>(1, "append_batch_ids")?;
        let ids = AllocatedIds::try_new(
            record_ids,
            event_ids,
            effect_ids,
            Vec::new(),
            message_ids,
            turn_ids,
            model_request_ids,
            Vec::new(),
            Vec::new(),
            append_batch_ids,
            Vec::new(),
        )
        .map_err(|error| adapter_error(format!("invalid allocated IDs: {error}")))?;
        Ok(TransitionEnv { now, ids })
    }

    fn take_ids<T: IdTag>(
        &mut self,
        count: usize,
        queue: &'static str,
    ) -> Result<Vec<Id<T>>, TraceError> {
        (0..count)
            .map(|_| {
                let raw = self.ids.get(self.id_index).ok_or_else(|| {
                    adapter_error(format!(
                        "transition_env.ids exhausted while filling {queue}"
                    ))
                })?;
                self.id_index += 1;
                Id::<T>::parse(raw).map_err(|error| {
                    adapter_error(format!("invalid canonical UUID for {queue}: {error}"))
                })
            })
            .collect()
    }

    fn finish(&self) -> Result<(), TraceError> {
        if self.timestamp_index != self.timestamps_ms.len() {
            return Err(adapter_error(format!(
                "transition_env.timestamps_ms has {} unused value(s)",
                self.timestamps_ms.len() - self.timestamp_index
            )));
        }
        if self.id_index != self.ids.len() {
            return Err(adapter_error(format!(
                "transition_env.ids has {} unused value(s)",
                self.ids.len() - self.id_index
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialUserFixture {
    session_id: SessionId,
    lane_id: LaneId,
    run_id: finstack_ai_kernel::RunId,
    timestamp_ms: u64,
    text: String,
}
struct InitialContext {
    session_id: SessionId,
    lane_id: LaneId,
    run_id: finstack_ai_kernel::RunId,
    user_message: Message,
}
struct ReducerDriver<'a> {
    kernel: Kernel,
    values: FixtureValues<'a>,
    initial: InitialContext,
    committed_batches: Vec<CommittedBatch>,
    events: Vec<RunEvent>,
    effects: Vec<PostCommitAction>,
    chunks: String,
    last_transition_at: Option<Timestamp>,
}
impl<'a> ReducerDriver<'a> {
    fn new(trace: &'a GoldenTrace) -> Result<Self, TraceError> {
        Ok(Self {
            kernel: Kernel::default(),
            values: FixtureValues::new(
                &trace.transition_env.timestamps_ms,
                &trace.transition_env.ids,
            ),
            initial: initial_context(trace)?,
            committed_batches: Vec::new(),
            events: Vec::new(),
            effects: Vec::new(),
            chunks: String::new(),
            last_transition_at: None,
        })
    }

    fn drive(mut self, trace: &GoldenTrace) -> Result<ReducerExecution, TraceError> {
        self.accept(trace)?;
        self.settle_stage(0, Stage::BeforeRun, ReducerStageOutcome::Continue)?;
        self.prepare_context(0)?;
        self.request_model(0)?;

        for step in &trace.scripted_outcomes.steps {
            self.apply_scripted_step(step)?;
        }

        match self.kernel.state().phase {
            Some(finstack_ai_kernel::RunPhase::BeforeFinalize) => self.finalize()?,
            Some(finstack_ai_kernel::RunPhase::Completed) => {}
            phase => {
                return Err(adapter_error(format!(
                    "script ended before a terminal result; phase was {phase:?}"
                )));
            }
        }
        self.values.finish()?;
        let observed = project_observed(
            &self.kernel,
            &self.committed_batches,
            &self.events,
            &self.effects,
        )?;
        Ok(ReducerExecution {
            observed,
            committed_batches: self.committed_batches,
            kernel_state: self.kernel.state().clone(),
        })
    }

    fn accept(&mut self, trace: &GoldenTrace) -> Result<(), TraceError> {
        let relation = RunRelation::root(self.initial.run_id)
            .map_err(|error| adapter_error(format!("root relation: {error}")))?;
        let principal = PrincipalRef::try_new("fixture-issuer", "fixture-subject", Some("fixture"))
            .map_err(|error| adapter_error(format!("fixture principal: {error}")))?;
        let security = RunSecurityContext::try_new(
            "fixture",
            principal,
            "fixture",
            "high",
            "fixture-policy-v1",
            "fixture-decision-v1",
            None,
        )
        .map_err(|error| adapter_error(format!("fixture security: {error}")))?;
        let propagation = RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        };
        let agent_bytes = normalize_json_value(&trace.initial_agent_spec);
        let accepted = RunAccepted::try_new(
            self.initial.run_id,
            relation,
            security,
            None,
            RunLimits::empty(),
            propagation,
            Digest::raw_json(&agent_bytes),
            None,
        )
        .map_err(|error| adapter_error(format!("fixture run acceptance: {error}")))?;
        let env = self
            .values
            .take_env(EnvRequirements::new(1, 1, 0, 0, 0, 0))?;
        self.apply_input(
            &env,
            KernelInput::AcceptRun(AcceptRun {
                session_id: self.initial.session_id,
                lane_id: self.initial.lane_id,
                accepted,
            }),
        )
    }

    fn settle_stage(
        &mut self,
        cycle: u64,
        stage: Stage,
        outcome: ReducerStageOutcome,
    ) -> Result<(), TraceError> {
        let required = match (&stage, &outcome) {
            (Stage::BeforeRun | Stage::AfterModel, ReducerStageOutcome::Continue)
            | (Stage::BeforeFinalize, ReducerStageOutcome::ContinueModel { .. }) => {
                EnvRequirements::new(1, 0, 0, 0, 0, 0)
            }
            (Stage::PrepareContext, ReducerStageOutcome::ContextPrepared { .. }) => {
                EnvRequirements::new(2, 0, 0, 0, 1, 0)
            }
            (Stage::BeforeModel, ReducerStageOutcome::ModelRequestPrepared { .. }) => {
                EnvRequirements::new(2, 1, 1, 0, 0, 1)
            }
            (Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted) => {
                EnvRequirements::new(2, 1, 0, 0, 0, 0)
            }
            _ => {
                return Err(adapter_error(
                    "fixture adapter requested an unsupported aggregate settlement",
                ));
            }
        };
        let env = self.values.take_env(required)?;
        self.apply_input(
            &env,
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor { cycle, stage },
                outcome,
            }),
        )
    }

    fn prepare_context(&mut self, cycle: u64) -> Result<(), TraceError> {
        let mut messages = Vec::with_capacity(self.kernel.state().messages.len() + 1);
        messages.push(self.initial.user_message.clone());
        messages.extend(self.kernel.state().messages.iter().cloned());
        self.settle_stage(
            cycle,
            Stage::PrepareContext,
            ReducerStageOutcome::ContextPrepared {
                messages: messages.into(),
            },
        )
    }

    fn request_model(&mut self, cycle: u64) -> Result<(), TraceError> {
        let messages = self
            .kernel
            .state()
            .current_turn
            .as_ref()
            .map(|turn| turn.context.messages.as_ref())
            .ok_or_else(|| adapter_error("prepared context missing before model request"))?;
        let request_text = serde_json::to_string(&json!({ "messages": messages }))
            .map_err(|error| adapter_error(error.to_string()))?;
        let request = RawJson::parse(&request_text)
            .map_err(|error| adapter_error(format!("model request JSON: {error}")))?;
        self.settle_stage(
            cycle,
            Stage::BeforeModel,
            ReducerStageOutcome::ModelRequestPrepared {
                request,
                component: None,
                output_contract: output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
        )
    }

    fn apply_scripted_step(&mut self, step: &ScriptedStep) -> Result<(), TraceError> {
        validate_step_shape(step)?;
        match step.kind {
            ScriptedStepKind::ModelChunk => self.model_chunk(step),
            ScriptedStepKind::ModelCompleted => self.model_completed(step),
            ScriptedStepKind::ModelDeferred => self.model_deferred(step),
            ScriptedStepKind::ExternalCompleted => self.external_completed(step),
            ScriptedStepKind::BeforeFinalizeContinue => self.before_finalize_continue(step),
            ScriptedStepKind::ToolCall
            | ScriptedStepKind::ToolResult
            | ScriptedStepKind::Cancellation
            | ScriptedStepKind::Error
            | ScriptedStepKind::Timer => Err(adapter_error(format!(
                "scripted step {:?} is outside PR-009 model-only scope",
                step.kind
            ))),
        }
    }

    fn model_chunk(&mut self, step: &ScriptedStep) -> Result<(), TraceError> {
        let text = required_text(step)?;
        let event_id = EventId::parse(required_id(step)?)
            .map_err(|error| adapter_error(format!("model_chunk id: {error}")))?;
        let pending = self
            .kernel
            .state()
            .pending_model_effect
            .as_ref()
            .ok_or_else(|| adapter_error("model_chunk requires a pending model effect"))?;
        if self.kernel.state().phase != Some(finstack_ai_kernel::RunPhase::AwaitingModel) {
            return Err(adapter_error(
                "model_chunk is valid only while awaiting the direct model",
            ));
        }
        let timestamp = self
            .last_transition_at
            .ok_or_else(|| adapter_error("model_chunk has no preceding transition timestamp"))?;
        let transient_sequence = u64::try_from(self.events.len())
            .map_err(|_| adapter_error("transient event sequence overflow"))?;
        let event = RunEvent::try_transient(
            finstack_ai_kernel::RUN_EVENT_SCHEMA_VERSION,
            finstack_ai_kernel::RUN_EVENT_KIND_VERSION,
            event_id,
            self.initial.session_id,
            self.initial.lane_id,
            self.initial.run_id,
            Some(pending.turn_id),
            Some(pending.model_request_id),
            None,
            Some(pending.requested.effect_id()),
            None,
            transient_sequence,
            timestamp,
            Sensitivity::Confidential,
            RunEventBody::ModelTextDelta(
                ModelTextDelta::try_new(text)
                    .map_err(|error| adapter_error(format!("model chunk: {error}")))?,
            ),
        )
        .map_err(|error| adapter_error(format!("transient model event: {error}")))?;
        self.events.push(event);
        self.chunks.push_str(text);
        Ok(())
    }

    fn model_completed(&mut self, step: &ScriptedStep) -> Result<(), TraceError> {
        if self.chunks.is_empty() {
            return Err(adapter_error(
                "model_completed requires at least one accumulated text chunk",
            ));
        }
        let completion_id = required_id(step)?;
        let pending = self.pending_model()?.clone();
        let env = self
            .values
            .take_env(EnvRequirements::new(2, 2, 0, 1, 0, 0))?;
        let message_id = *env
            .ids
            .message_ids()
            .first()
            .ok_or_else(|| adapter_error("model completion message id missing"))?;
        let provider_ids = provider_ids(completion_id)?;
        let output = model_output(&self.chunks)?;
        let completion = EffectCompleted::try_new(
            pending.requested.effect_id(),
            pending.requested.output_contract().clone(),
            output,
            None,
            Vec::new(),
            provider_ids.clone(),
            Some(completion_id),
            None,
        )
        .map_err(|error| adapter_error(format!("model completion: {error}")))?;
        let assistant_message = assistant_message(message_id, env.now, &self.chunks, provider_ids)?;
        self.apply_input(
            &env,
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message,
                },
            }),
        )?;
        self.chunks.clear();
        self.settle_stage(
            pending.cycle,
            Stage::AfterModel,
            ReducerStageOutcome::Continue,
        )
    }

    fn model_deferred(&mut self, step: &ScriptedStep) -> Result<(), TraceError> {
        if !self.chunks.is_empty() {
            return Err(adapter_error(
                "model_deferred cannot follow accumulated final text",
            ));
        }
        let pending = self.pending_model()?.clone();
        let handle = ExternalHandleRef::try_new(
            ComponentId::parse("finstack.provider.fixture")
                .map_err(|error| adapter_error(format!("fixture component: {error}")))?,
            required_id(step)?,
            RawJson::parse("{}")
                .map_err(|error| adapter_error(format!("reconciliation metadata: {error}")))?,
        )
        .map_err(|error| adapter_error(format!("external handle: {error}")))?;
        let deferred = EffectDeferred {
            effect_id: pending.requested.effect_id(),
            handle,
            reconciliation: ReconciliationPolicy::CallbackOnly,
            next_poll_at: None,
            expires_at: None,
            output_contract: pending.requested.output_contract().clone(),
        };
        let env = self
            .values
            .take_env(EnvRequirements::new(1, 1, 0, 0, 0, 0))?;
        self.apply_input(
            &env,
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Deferred(deferred),
            }),
        )
    }

    fn external_completed(&mut self, step: &ScriptedStep) -> Result<(), TraceError> {
        let completion_id = required_id(step)?;
        let text = present_text(step)?;
        let pending = self.pending_model()?.clone();
        let env = self
            .values
            .take_env(EnvRequirements::new(2, 2, 0, 1, 0, 0))?;
        let message_id = *env
            .ids
            .message_ids()
            .first()
            .ok_or_else(|| adapter_error("external completion message id missing"))?;
        let provider_ids = provider_ids(completion_id)?;
        let completion = ExternalEffectCompletion::try_new(
            pending.requested.effect_id(),
            completion_id,
            ExternalEffectOutcome::Completed {
                output: model_output(text)?,
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .map_err(|error| adapter_error(format!("external completion: {error}")))?;
        let assistant_message = assistant_message(message_id, env.now, text, provider_ids)?;
        self.apply_input(
            &env,
            KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
                completion,
                assistant_message: Some(assistant_message),
            }),
        )?;
        self.settle_stage(
            pending.cycle,
            Stage::AfterModel,
            ReducerStageOutcome::Continue,
        )
    }

    fn before_finalize_continue(&mut self, step: &ScriptedStep) -> Result<(), TraceError> {
        let cycle = self.kernel.state().cycle;
        self.settle_stage(
            cycle,
            Stage::BeforeFinalize,
            ReducerStageOutcome::ContinueModel {
                reason: step.message.as_deref().map(Arc::from),
            },
        )?;
        let next_cycle = cycle
            .checked_add(1)
            .ok_or_else(|| adapter_error("fixture model cycle overflow"))?;
        self.prepare_context(next_cycle)?;
        self.request_model(next_cycle)
    }

    fn finalize(&mut self) -> Result<(), TraceError> {
        let cycle = self.kernel.state().cycle;
        self.settle_stage(
            cycle,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        )
    }

    fn pending_model(&self) -> Result<&finstack_ai_kernel::PendingModelEffect, TraceError> {
        self.kernel
            .state()
            .pending_model_effect
            .as_ref()
            .ok_or_else(|| adapter_error("scripted model outcome has no pending model effect"))
    }

    fn apply_input(&mut self, env: &TransitionEnv, input: KernelInput) -> Result<(), TraceError> {
        let batch_id = *env
            .ids
            .append_batch_ids()
            .first()
            .ok_or_else(|| adapter_error("append_batch_ids requires exactly one value"))?;
        if env.ids.append_batch_ids().len() != 1 {
            return Err(adapter_error(
                "append_batch_ids requires exactly one value per transition",
            ));
        }
        let transition_at = env.now;
        let decision = self
            .kernel
            .decide(env, input)
            .map_err(|error| adapter_error(format!("kernel decide {}: {error}", error.code())))?;
        if decision.records.is_empty() {
            return Err(adapter_error(
                "fixture transitions must not resolve as duplicate no-op decisions",
            ));
        }
        let committed = commit_decision(&decision, batch_id)?;
        let first_transient_sequence = u64::try_from(self.events.len())
            .map_err(|_| adapter_error("transient event sequence overflow"))?;
        let derived = self
            .kernel
            .apply(&committed, first_transient_sequence)
            .map_err(|error| adapter_error(format!("kernel apply {}: {error}", error.code())))?;
        self.events.extend(derived.iter().cloned());
        self.effects.extend(decision.actions);
        self.committed_batches.push(committed);
        self.last_transition_at = Some(transition_at);
        Ok(())
    }
}

/// Execute a PR-009 trace and retain its ordered committed batches.
///
/// # Errors
///
/// Returns [`TraceError`] for invalid fixture inputs or reducer failures.
pub fn execute_reducer_trace(trace: &GoldenTrace) -> Result<ReducerExecution, TraceError> {
    if trace.format_version != 1 {
        return Err(adapter_error(format!(
            "unsupported golden trace format_version {}",
            trace.format_version
        )));
    }
    if trace.scripted_outcomes.format_version != 1 {
        return Err(adapter_error(format!(
            "unsupported scripted input format_version {}",
            trace.scripted_outcomes.format_version
        )));
    }
    ReducerDriver::new(trace)?.drive(trace)
}

fn initial_context(trace: &GoldenTrace) -> Result<InitialContext, TraceError> {
    let (fixture, message_id) = match trace.initial_session_records.as_slice() {
        [] => (
            InitialUserFixture {
                session_id: SessionId::parse(DEFAULT_SESSION_ID)
                    .map_err(|error| adapter_error(error.to_string()))?,
                lane_id: LaneId::parse(DEFAULT_LANE_ID)
                    .map_err(|error| adapter_error(error.to_string()))?,
                run_id: finstack_ai_kernel::RunId::parse(DEFAULT_RUN_ID)
                    .map_err(|error| adapter_error(error.to_string()))?,
                timestamp_ms: 0,
                text: "Say hello.".to_owned(),
            },
            MessageId::parse(DEFAULT_USER_MESSAGE_ID)
                .map_err(|error| adapter_error(error.to_string()))?,
        ),
        [record] if record.kind == "user_message" => {
            if record.payload_declaration.is_some() {
                return Err(adapter_error(
                    "initial user_message does not accept payload_declaration",
                ));
            }
            let id = record
                .id
                .as_deref()
                .ok_or_else(|| adapter_error("initial user_message requires id"))?;
            let message_id = MessageId::parse(id)
                .map_err(|error| adapter_error(format!("user message id: {error}")))?;
            let payload = record
                .payload
                .clone()
                .ok_or_else(|| adapter_error("initial user_message requires payload"))?;
            let fixture: InitialUserFixture = serde_json::from_value(payload)
                .map_err(|error| adapter_error(format!("initial user_message payload: {error}")))?;
            (fixture, message_id)
        }
        [_] => {
            return Err(adapter_error(
                "the only supported initial record kind is user_message",
            ));
        }
        _ => {
            return Err(adapter_error(
                "PR-009 fixtures accept at most one initial user_message",
            ));
        }
    };
    let timestamp_ms = i64::try_from(fixture.timestamp_ms)
        .map_err(|_| adapter_error("initial user timestamp does not fit i64"))?;
    let created_at = Timestamp::from_unix_ms(timestamp_ms)
        .map_err(|error| adapter_error(format!("initial user timestamp: {error}")))?;
    let user_message = Message::try_new(
        message_id,
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new(&fixture.text)
                .map_err(|error| adapter_error(format!("initial user text: {error}")))?,
        )],
        created_at,
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .map_err(|error| adapter_error(format!("initial user message: {error}")))?;
    Ok(InitialContext {
        session_id: fixture.session_id,
        lane_id: fixture.lane_id,
        run_id: fixture.run_id,
        user_message,
    })
}

fn validate_step_shape(step: &ScriptedStep) -> Result<(), TraceError> {
    let unexpected = match step.kind {
        ScriptedStepKind::ModelChunk | ScriptedStepKind::ExternalCompleted => {
            step.tool_name.is_some()
                || step.arguments.is_some()
                || step.result.is_some()
                || step.error_code.is_some()
                || step.message.is_some()
                || step.duration_ms.is_some()
        }
        ScriptedStepKind::ModelCompleted | ScriptedStepKind::ModelDeferred => {
            step.text.is_some()
                || step.tool_name.is_some()
                || step.arguments.is_some()
                || step.result.is_some()
                || step.error_code.is_some()
                || step.message.is_some()
                || step.duration_ms.is_some()
        }
        ScriptedStepKind::BeforeFinalizeContinue => {
            step.id.is_some()
                || step.text.is_some()
                || step.tool_name.is_some()
                || step.arguments.is_some()
                || step.result.is_some()
                || step.error_code.is_some()
                || step.duration_ms.is_some()
        }
        ScriptedStepKind::ToolCall
        | ScriptedStepKind::ToolResult
        | ScriptedStepKind::Cancellation
        | ScriptedStepKind::Error
        | ScriptedStepKind::Timer => false,
    };
    if unexpected {
        Err(adapter_error(format!(
            "scripted step {:?} contains fields not valid for that kind",
            step.kind
        )))
    } else {
        Ok(())
    }
}

fn required_id(step: &ScriptedStep) -> Result<&str, TraceError> {
    step.id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| adapter_error(format!("{:?} requires non-empty id", step.kind)))
}

fn required_text(step: &ScriptedStep) -> Result<&str, TraceError> {
    step.text
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| adapter_error(format!("{:?} requires non-empty text", step.kind)))
}

fn present_text(step: &ScriptedStep) -> Result<&str, TraceError> {
    step.text
        .as_deref()
        .ok_or_else(|| adapter_error(format!("{:?} requires text", step.kind)))
}

fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}
fn provider_ids(completion_id: &str) -> Result<ProviderIds, TraceError> {
    ProviderIds::try_new(Some("fixture-request"), Some(completion_id), None::<&str>)
        .map_err(|error| adapter_error(format!("provider ids: {error}")))
}
fn model_output(text: &str) -> Result<RawJson, TraceError> {
    RawJson::parse(
        &serde_json::to_string(&json!({ "text": text }))
            .map_err(|error| adapter_error(error.to_string()))?,
    )
    .map_err(|error| adapter_error(format!("model output: {error}")))
}
fn assistant_message(
    message_id: MessageId,
    created_at: Timestamp,
    text: &str,
    provider_ids: ProviderIds,
) -> Result<Message, TraceError> {
    let content = vec![ContentBlock::Text(
        TextBlock::try_new(text)
            .map_err(|error| adapter_error(format!("assistant text: {error}")))?,
    )];
    Message::try_new(
        message_id,
        MessageRole::Assistant,
        content,
        created_at,
        None,
        provider_ids,
        Metadata::empty(),
    )
    .map_err(|error| adapter_error(format!("assistant message: {error}")))
}
fn commit_decision(
    decision: &Decision,
    batch_id: AppendBatchId,
) -> Result<CommittedBatch, TraceError> {
    let records = decision
        .records
        .iter()
        .enumerate()
        .map(|(index, draft)| commit_record(decision.expected_sequence, index, draft))
        .collect::<Result<Vec<_>, _>>()?;
    let count = u64::try_from(records.len())
        .map_err(|_| adapter_error("committed record count does not fit u64"))?;
    let last_sequence = decision
        .expected_sequence
        .checked_add(
            count
                .checked_sub(1)
                .ok_or_else(|| adapter_error("cannot commit an empty decision"))?,
        )
        .ok_or_else(|| adapter_error("committed sequence overflow"))?;
    CommittedBatch::try_new(batch_id, decision.expected_sequence, last_sequence, records)
        .map_err(|error| adapter_error(format!("committed batch {}: {error}", error.code())))
}
fn commit_record(
    first_sequence: u64,
    index: usize,
    draft: &RecordDraft,
) -> Result<RecordEnvelope, TraceError> {
    let offset =
        u64::try_from(index).map_err(|_| adapter_error("record index does not fit u64"))?;
    let sequence = first_sequence
        .checked_add(offset)
        .ok_or_else(|| adapter_error("record sequence overflow"))?;
    let body = serde_json::to_vec(draft.body())
        .map_err(|error| adapter_error(format!("record body serialization: {error}")))?;
    let record_id = draft.record_id().to_canonical_string();
    RecordEnvelope::try_new(
        draft.format_version(),
        draft.kind_version(),
        draft.record_id(),
        draft.session_id(),
        draft.lane_id(),
        draft.run_id(),
        sequence,
        draft.timestamp(),
        None,
        Digest::raw_json(&body),
        None,
        Digest::raw_json(format!("{record_id}:{sequence}").as_bytes()),
        draft.derived_event_ids().to_vec(),
        draft.body().clone(),
    )
    .map_err(|error| adapter_error(format!("commit record: {error}")))
}
fn project_observed(
    kernel: &Kernel,
    batches: &[CommittedBatch],
    events: &[RunEvent],
    effects: &[PostCommitAction],
) -> Result<ExpectedTrace, TraceError> {
    let durable_records = batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(|record| TraceRecord {
            kind: record.body().kind_name().to_owned(),
            id: Some(record.record_id().to_canonical_string()),
            payload: None,
            payload_declaration: None,
        })
        .collect();
    let normalized_events = events
        .iter()
        .map(project_event)
        .collect::<Result<Vec<_>, _>>()?;
    let effects = effects
        .iter()
        .map(|action| match action {
            PostCommitAction::ExecuteEffect { effect_id } => EffectExpectation {
                kind: "execute_effect".to_owned(),
                id: Some(effect_id.to_canonical_string()),
                payload: None,
                payload_declaration: None,
            },
        })
        .collect();
    let terminal = project_reducer_terminal(kernel)?;
    Ok(ExpectedTrace {
        durable_records,
        normalized_events,
        effects,
        final_state: terminal.final_state,
        final_result: terminal.final_result,
        state_hash: terminal.state_hash,
    })
}

/// Project terminal fields from any live, replayed, or programmatically assembled kernel state.
///
/// # Errors
///
/// Returns [`TraceError`] when terminal serialization or state hashing fails.
pub fn project_reducer_terminal(kernel: &Kernel) -> Result<ReducerTerminalProjection, TraceError> {
    let final_state = json!({
        "phase": kernel.state().phase,
        "cycle": kernel.state().cycle,
        "last_applied_sequence": kernel.state().last_applied_sequence,
        "message_ids": kernel
            .state()
            .messages
            .iter()
            .map(|message| message.id().to_canonical_string())
            .collect::<Vec<_>>(),
    });
    let final_result = serde_json::to_value(&kernel.state().terminal)
        .map_err(|error| adapter_error(format!("final result serialization: {error}")))?;
    let state_hash = kernel
        .state()
        .state_hash()
        .map_err(|error| adapter_error(format!("state hash {}: {error}", error.code())))?
        .to_hex();
    Ok(ReducerTerminalProjection {
        final_state,
        final_result,
        state_hash,
    })
}

fn project_event(event: &RunEvent) -> Result<NormalizedEvent, TraceError> {
    let kind = serde_json::to_value(event.kind())
        .map_err(|error| adapter_error(error.to_string()))?
        .as_str()
        .ok_or_else(|| adapter_error("event kind did not serialize as a string"))?
        .to_owned();
    let durability = match event.class() {
        RunEventClass::DurableDerived => DurabilityClass::Durable,
        RunEventClass::Transient => DurabilityClass::Transient,
    };
    Ok(NormalizedEvent {
        kind,
        durability,
        id: Some(event.event_id().to_canonical_string()),
        payload: match event.body() {
            RunEventBody::ModelTextDelta(delta) => Some(json!({ "text": delta.text() })),
            _ => None,
        },
        payload_declaration: None,
    })
}

fn adapter_error(message: impl Into<String>) -> TraceError {
    TraceError::Adapter(message.into())
}
