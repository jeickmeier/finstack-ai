//! Deterministic scripted input format and provider-neutral model leaf.
//!
//! Covers model chunks, tool calls/results, cancellation, errors, and timers
//! without requiring a live model or Phase 1 reducer.

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use finstack_ai_kernel::{
    ComponentId, ContentBlock, ErrorCategory, ExternalHandleRef, Metadata, PendingModelEffect,
    ProviderIds, RawJson, ReconciliationPolicy, TextBlock, Usage,
};
use finstack_ai_runtime::{
    CancellationSignal, InputCapabilities, Model, ModelCapabilities, ModelContextProfile,
    ModelDeferral, ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelReconcileResult,
    ModelRequest, ModelResponse, ModelStreamItem, ModelTokenEstimate, ModelToolCall,
    ModelWarmupContext, PortFuture, ReconcileContext, StructuredOutputCapability, TextDelta,
    ToolCallDelta,
};
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::fixtures::trace::PayloadDeclaration;

/// Kind discriminator for one scripted step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptedStepKind {
    /// One model text chunk.
    ModelChunk,
    /// Complete the current model request from accumulated text chunks.
    ModelCompleted,
    /// Defer the current model request under a non-secret external handle.
    ModelDeferred,
    /// Complete the deferred model request with final external text.
    ExternalCompleted,
    /// Continue from `before_finalize` into a fresh model cycle.
    BeforeFinalizeContinue,
    /// One tool invocation request.
    ToolCall,
    /// One tool result.
    ToolResult,
    /// Cancellation boundary.
    Cancellation,
    /// Provider or tool error.
    Error,
    /// Manual-clock timer fire.
    Timer,
}

/// One deterministic scripted outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptedStep {
    /// Step kind.
    pub kind: ScriptedStepKind,
    /// Optional stable identifier for correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Text payload for model chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Tool name for call/result steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool arguments object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<serde_json::Value>,
    /// Tool result payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Stable error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Human-readable diagnostic message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Timer duration in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Optional contract section 6.5 payload declaration ceilings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_declaration: Option<PayloadDeclaration>,
}

/// Scripted input sequence consumed by golden traces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptedInput {
    /// Format major version. Always `1` for this schema.
    pub format_version: u32,
    /// Ordered scripted steps.
    pub steps: Vec<ScriptedStep>,
}

impl ScriptedInput {
    /// Construct an empty scripted input.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            format_version: 1,
            steps: Vec::new(),
        }
    }
}

/// One fully expressive programmatic scripted model action.
#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "the test-only action keeps normalized stream items directly inspectable in fixtures"
)]
pub enum ScriptedModelAction {
    /// Emit one normalized stream item or adapter error.
    Emit(Result<ModelStreamItem, ModelError>),
    /// Block at a named deterministic gate until released or cancelled.
    Block(Arc<str>),
    /// Block at a named gate while deliberately ignoring cancellation.
    ///
    /// This action exists to prove the runtime's grace-deadline abort path.
    BlockUninterruptibly(Arc<str>),
    /// Block until the effect-local cancellation signal fires.
    AwaitCancellation,
}

/// One request's fully expressive stream plan.
#[derive(Debug, Clone, Default)]
pub struct ScriptedModelPlan {
    /// Source-ordered stream actions.
    pub actions: Vec<ScriptedModelAction>,
}

#[derive(Debug, Default)]
struct Gate {
    released: AtomicBool,
    entered: AtomicUsize,
    waiters: Mutex<Vec<Waker>>,
}

impl Gate {
    fn release(&self) {
        self.released.store(true, Ordering::Release);
        if let Ok(mut waiters) = self.waiters.lock() {
            for waiter in waiters.drain(..) {
                waiter.wake();
            }
        }
    }

    fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.released.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if let Ok(mut waiters) = self.waiters.lock()
            && !waiters.iter().any(|waiter| waiter.will_wake(cx.waker()))
        {
            waiters.push(cx.waker().clone());
        }
        if self.released.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Deterministic blocking-point controller for a [`ScriptedModel`].
#[derive(Debug, Clone, Default)]
pub struct ScriptedModelControl {
    gates: Arc<Mutex<BTreeMap<Arc<str>, Arc<Gate>>>>,
}

impl ScriptedModelControl {
    /// Release a named gate. Future requests using the same name also pass.
    pub fn release(&self, name: impl AsRef<str>) {
        self.gate(Arc::from(name.as_ref())).release();
    }

    /// Number of stream entries observed at a named gate.
    #[must_use]
    pub fn entries(&self, name: impl AsRef<str>) -> usize {
        self.gate(Arc::from(name.as_ref()))
            .entered
            .load(Ordering::Acquire)
    }

    fn gate(&self, name: Arc<str>) -> Arc<Gate> {
        self.gates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(name)
            .or_default()
            .clone()
    }
}

/// Actual deterministic leaf implementation of the public [`Model`] port.
#[derive(Debug)]
pub struct ScriptedModel {
    profile: ModelContextProfile,
    descriptor: ModelDescriptor,
    plans: Mutex<VecDeque<ScriptedModelPlan>>,
    reconcile_results: Mutex<VecDeque<ModelReconcileResult>>,
    control: ScriptedModelControl,
    warmups: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    reconciles: Arc<AtomicUsize>,
    last_request: Mutex<Option<ModelRequest>>,
    cancellation_acknowledgements: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
    dropped_streams: Arc<AtomicUsize>,
    idempotent_requests: bool,
}

impl ScriptedModel {
    /// Construct from fully expressive programmatic stream plans.
    #[must_use]
    pub fn from_plans(profile: ModelContextProfile, plans: Vec<ScriptedModelPlan>) -> Self {
        let descriptor = ModelDescriptor {
            provider: Arc::clone(&profile.provider),
            models: Arc::from([profile.model.clone()]),
            metadata: Metadata::empty(),
        };
        Self {
            profile,
            descriptor,
            plans: Mutex::new(plans.into()),
            reconcile_results: Mutex::new(VecDeque::new()),
            control: ScriptedModelControl::default(),
            warmups: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(AtomicUsize::new(0)),
            reconciles: Arc::new(AtomicUsize::new(0)),
            last_request: Mutex::new(None),
            cancellation_acknowledgements: Arc::new(AtomicUsize::new(0)),
            active_streams: Arc::new(AtomicUsize::new(0)),
            dropped_streams: Arc::new(AtomicUsize::new(0)),
            idempotent_requests: true,
        }
    }

    /// Queue provider reconcile outcomes consumed in order. Default is `Unknown`.
    #[must_use]
    pub fn with_reconcile_results(self, results: Vec<ModelReconcileResult>) -> Self {
        *self
            .reconcile_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = results.into();
        self
    }

    /// Override the advertised idempotent-request capability.
    #[must_use]
    pub fn with_idempotent_requests(mut self, idempotent_requests: bool) -> Self {
        self.idempotent_requests = idempotent_requests;
        self
    }

    /// Construct from the existing version-1 golden-trace fixture language.
    ///
    /// Each [`ScriptedInput`] is consumed by exactly one model request. Existing
    /// fixture kinds retain their reducer meaning; model-only irrelevant kinds
    /// become explicit adapter errors rather than being silently ignored.
    #[must_use]
    pub fn from_inputs(profile: ModelContextProfile, inputs: Vec<ScriptedInput>) -> Self {
        let plans = inputs
            .into_iter()
            .map(|input| fixture_plan(&input))
            .collect();
        Self::from_plans(profile, plans)
    }

    /// Clone the deterministic blocking controller.
    #[must_use]
    pub fn control(&self) -> ScriptedModelControl {
        self.control.clone()
    }

    /// Number of warmup calls observed.
    #[must_use]
    pub fn warmup_count(&self) -> usize {
        self.warmups.load(Ordering::Acquire)
    }

    /// Number of request calls observed.
    #[must_use]
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Acquire)
    }

    /// Last request observed by [`Model::request`], if any.
    #[must_use]
    pub fn last_request(&self) -> Option<ModelRequest> {
        self.last_request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Number of reconcile calls observed.
    #[must_use]
    pub fn reconcile_count(&self) -> usize {
        self.reconciles.load(Ordering::Acquire)
    }

    /// Number of effect-local cancellations acknowledged by a blocked stream.
    #[must_use]
    pub fn cancellation_acknowledgement_count(&self) -> usize {
        self.cancellation_acknowledgements.load(Ordering::Acquire)
    }

    /// Number of currently retained scripted streams.
    #[must_use]
    pub fn active_stream_count(&self) -> usize {
        self.active_streams.load(Ordering::Acquire)
    }

    /// Number of scripted streams dropped after construction.
    #[must_use]
    pub fn dropped_stream_count(&self) -> usize {
        self.dropped_streams.load(Ordering::Acquire)
    }
}

impl Model for ScriptedModel {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor.clone()
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: true,
                audio: true,
                files: true,
            },
            context_profile: self.profile.clone(),
            native_tool_calls: true,
            parallel_tool_calls: true,
            structured_output: StructuredOutputCapability::Native,
            reasoning: true,
            prompt_cache: false,
            resumable_stream: true,
            idempotent_requests: self.idempotent_requests,
            native_capabilities: BTreeSet::default(),
        }
    }

    fn warmup(&self, _ctx: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
        self.warmups.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Ok(()) })
    }

    fn estimate_input_tokens(
        &self,
        _model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        let input_tokens = u64::try_from(canonical_request.len()).map_err(|_| {
            ModelError::try_new(
                "scripted_estimate_overflow",
                ErrorCategory::Limit,
                false,
                "scripted request byte length overflow",
                Metadata::empty(),
            )
            .unwrap_or_else(ModelError::from)
        })?;
        Ok(ModelTokenEstimate {
            input_tokens,
            estimator: self.profile.estimator.clone(),
        })
    }

    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        self.requests.fetch_add(1, Ordering::AcqRel);
        *self
            .last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(request.clone());
        let plan = self
            .plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        let control = self.control.clone();
        let acknowledgements = Arc::clone(&self.cancellation_acknowledgements);
        let active_streams = Arc::clone(&self.active_streams);
        let dropped_streams = Arc::clone(&self.dropped_streams);
        Box::pin(async move {
            let plan = plan.ok_or_else(|| {
                scripted_error("scripted_model_exhausted", "no scripted request remains")
            })?;
            active_streams.fetch_add(1, Ordering::AcqRel);
            Ok(Box::pin(ScriptedStream {
                actions: plan.actions.into(),
                cancellation: request.call.run.cancellation,
                cancellation_wait: None,
                control,
                acknowledgements,
                active_streams,
                dropped_streams,
                active_gate: None,
            }) as ModelEventStream)
        })
    }

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingModelEffect,
    ) -> PortFuture<Result<ModelReconcileResult, ModelError>> {
        self.reconciles.fetch_add(1, Ordering::AcqRel);
        let result = self
            .reconcile_results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or(ModelReconcileResult::Unknown);
        Box::pin(async move { Ok(result) })
    }
}

struct ScriptedStream {
    actions: VecDeque<ScriptedModelAction>,
    cancellation: CancellationSignal,
    cancellation_wait: Option<PortFuture<()>>,
    control: ScriptedModelControl,
    acknowledgements: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
    dropped_streams: Arc<AtomicUsize>,
    active_gate: Option<Arc<str>>,
}

impl Drop for ScriptedStream {
    fn drop(&mut self) {
        self.active_streams.fetch_sub(1, Ordering::AcqRel);
        self.dropped_streams.fetch_add(1, Ordering::AcqRel);
    }
}

impl ScriptedStream {
    fn cancellation_poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.cancellation.is_cancelled() {
            return Poll::Ready(());
        }
        let cancellation = self.cancellation.clone();
        let wait = self
            .cancellation_wait
            .get_or_insert_with(|| Box::pin(async move { cancellation.cancelled().await }));
        wait.as_mut().poll(cx)
    }

    fn acknowledge_cancellation(&self) -> Poll<Option<Result<ModelStreamItem, ModelError>>> {
        self.acknowledgements.fetch_add(1, Ordering::AcqRel);
        Poll::Ready(Some(Err(ModelError::try_new(
            "model_cancelled",
            ErrorCategory::Cancellation,
            false,
            "scripted model acknowledged effect cancellation",
            Metadata::empty(),
        )
        .unwrap_or_else(ModelError::from))))
    }
}

impl Stream for ScriptedStream {
    type Item = Result<ModelStreamItem, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let Some(action) = self.actions.front().cloned() else {
                return Poll::Ready(None);
            };
            match action {
                ScriptedModelAction::Emit(item) => {
                    self.actions.pop_front();
                    return Poll::Ready(Some(item));
                }
                ScriptedModelAction::AwaitCancellation => {
                    if self.cancellation_poll(cx).is_ready() {
                        self.actions.pop_front();
                        return self.acknowledge_cancellation();
                    }
                    return Poll::Pending;
                }
                ScriptedModelAction::Block(name) => {
                    let gate = self.control.gate(name.clone());
                    if self.active_gate.as_ref() != Some(&name) {
                        gate.entered.fetch_add(1, Ordering::AcqRel);
                        self.active_gate = Some(name);
                    }
                    if gate.poll(cx).is_ready() {
                        self.actions.pop_front();
                        self.active_gate = None;
                        continue;
                    }
                    if self.cancellation_poll(cx).is_ready() {
                        self.actions.pop_front();
                        self.active_gate = None;
                        return self.acknowledge_cancellation();
                    }
                    return Poll::Pending;
                }
                ScriptedModelAction::BlockUninterruptibly(name) => {
                    let gate = self.control.gate(name.clone());
                    if self.active_gate.as_ref() != Some(&name) {
                        gate.entered.fetch_add(1, Ordering::AcqRel);
                        self.active_gate = Some(name);
                    }
                    if gate.poll(cx).is_ready() {
                        self.actions.pop_front();
                        self.active_gate = None;
                        continue;
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

fn fixture_plan(input: &ScriptedInput) -> ScriptedModelPlan {
    let mut actions = Vec::new();
    let mut text = String::new();
    let mut calls = Vec::<ModelToolCall>::new();
    for step in &input.steps {
        match step.kind {
            ScriptedStepKind::ModelChunk | ScriptedStepKind::ExternalCompleted => {
                let value = step.text.clone().unwrap_or_default();
                text.push_str(&value);
                actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                    TextDelta { text: value.into() },
                ))));
            }
            ScriptedStepKind::ToolCall => {
                let name: Arc<str> = step.tool_name.clone().unwrap_or_default().into();
                let arguments = step.arguments.clone().unwrap_or(serde_json::Value::Null);
                let arguments_text = serde_json::to_string(&arguments).unwrap_or_default();
                if let Ok(arguments) = RawJson::parse(arguments_text.as_bytes()) {
                    calls.push(ModelToolCall {
                        name: name.clone(),
                        arguments,
                        provider_call_id: None,
                    });
                }
                let index = u32::try_from(calls.len().saturating_sub(1)).unwrap_or(u32::MAX);
                actions.push(ScriptedModelAction::Emit(Ok(
                    ModelStreamItem::ToolCallDelta(ToolCallDelta {
                        index,
                        name: Some(name),
                        arguments_delta: arguments_text.into(),
                        provider_call_id: None,
                    }),
                )));
            }
            ScriptedStepKind::ModelCompleted => {
                actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
                    completed_response(
                        &text,
                        &calls,
                        step.id.as_deref().unwrap_or("scripted-completion"),
                    ),
                ))));
            }
            ScriptedStepKind::ModelDeferred => {
                actions.push(ScriptedModelAction::Emit(model_deferral(step)));
            }
            ScriptedStepKind::Cancellation => {
                actions.push(ScriptedModelAction::AwaitCancellation);
            }
            ScriptedStepKind::Error => {
                actions.push(ScriptedModelAction::Emit(Err(scripted_error(
                    step.error_code.as_deref().unwrap_or("scripted_model_error"),
                    step.message.as_deref().unwrap_or("scripted model error"),
                ))));
            }
            ScriptedStepKind::Timer => {
                let metadata = Metadata::parse(
                    serde_json::json!({"duration_ms":step.duration_ms.unwrap_or(0)})
                        .to_string()
                        .as_bytes(),
                )
                .unwrap_or_else(|_| Metadata::empty());
                actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::Heartbeat(
                    metadata,
                ))));
            }
            ScriptedStepKind::BeforeFinalizeContinue | ScriptedStepKind::ToolResult => {
                actions.push(ScriptedModelAction::Emit(Err(scripted_error(
                    "scripted_step_not_model_stream",
                    "scripted step belongs outside the model stream",
                ))));
            }
        }
    }
    ScriptedModelPlan { actions }
}

fn completed_response(text: &str, calls: &[ModelToolCall], completion_id: &str) -> ModelResponse {
    let assistant_content: Arc<[ContentBlock]> = if text.is_empty() {
        Arc::from([])
    } else {
        match TextBlock::try_new(text) {
            Ok(block) => Arc::from([ContentBlock::Text(block)]),
            Err(_) => Arc::from([]),
        }
    };
    ModelResponse {
        assistant_content,
        tool_calls: calls.to_vec().into(),
        usage: Usage::empty(),
        provider_ids: ProviderIds::try_new(None::<&str>, Some(completion_id), None::<&str>)
            .unwrap_or_else(|_| ProviderIds::empty()),
        completion_id: Arc::from(completion_id),
        continuation_state: None,
    }
}

fn model_deferral(step: &ScriptedStep) -> Result<ModelStreamItem, ModelError> {
    let handle = step.id.as_deref().unwrap_or("scripted-deferral");
    let provider = finstack_ai_kernel::static_key!(ComponentId, "finstack.model.scripted");
    let handle =
        ExternalHandleRef::try_new(provider, handle, Metadata::empty().as_raw_json().clone())
            .map_err(|_| {
                scripted_error("scripted_deferral_invalid", "invalid scripted deferral")
            })?;
    Ok(ModelStreamItem::Deferred(ModelDeferral {
        handle,
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: None,
        expires_at: None,
    }))
}

fn scripted_error(code: &str, message: &str) -> ModelError {
    ModelError::try_new(
        code,
        ErrorCategory::Model,
        false,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(ModelError::from)
}
