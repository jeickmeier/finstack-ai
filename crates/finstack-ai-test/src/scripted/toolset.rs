//! Deterministic target-neutral Toolset leaf and scheduler controls.

use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use finstack_ai_kernel::{ErrorCategory, Metadata, ValidatedToolCall};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{CancellationSignal, ReconcileContext, ToolSpec};
use finstack_ai_runtime::ports::tool::{
    PendingToolEffect, ToolCallContext, ToolError, ToolEventStream, ToolReconcileResult,
    ToolStreamItem, Toolset, ToolsetDescriptor,
};
use futures_core::Stream;

use crate::fakes::Gate;

/// One fully expressive scripted tool-stream action.
#[derive(Debug, Clone)]
#[allow(
    clippy::large_enum_variant,
    reason = "boxing the primary test action would make every scripted fixture noisier"
)]
pub enum ScriptedToolAction {
    /// Emit one normalized stream item or adapter error.
    Emit(Result<ToolStreamItem, ToolError>),
    /// Block at a deterministic gate until released or cancelled.
    Block(Arc<str>),
    /// Block until effect-local cancellation is observed.
    AwaitCancellation,
    /// Panic while polling the native extension stream boundary.
    Panic(Arc<str>),
}

/// One call's deterministic scripted behavior.
#[derive(Debug, Clone, Default)]
pub struct ScriptedToolPlan {
    /// Panic from `Toolset::call` before returning a stream.
    pub panic_on_call: Option<Arc<str>>,
    /// Source-ordered stream actions.
    pub actions: Vec<ScriptedToolAction>,
}

/// Deterministic gate and concurrency controller for [`ScriptedToolset`].
#[derive(Debug, Clone, Default)]
pub struct ScriptedToolsetControl {
    gates: Arc<Mutex<BTreeMap<Arc<str>, Arc<Gate>>>>,
}

impl ScriptedToolsetControl {
    /// Release a named gate. Future calls using the same gate also pass.
    pub fn release(&self, name: impl AsRef<str>) {
        self.gate(Arc::from(name.as_ref())).release();
    }

    /// Number of stream entries observed at a named gate.
    #[must_use]
    pub fn entries(&self, name: impl AsRef<str>) -> usize {
        self.gate(Arc::from(name.as_ref())).entries()
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

/// Actual deterministic leaf implementation of the public [`Toolset`] port.
#[derive(Debug)]
pub struct ScriptedToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    plans: Mutex<VecDeque<ScriptedToolPlan>>,
    reconcile_results: Mutex<VecDeque<ToolReconcileResult>>,
    control: ScriptedToolsetControl,
    calls: Arc<AtomicUsize>,
    reconciles: Arc<AtomicUsize>,
    last_effect_id: Mutex<Option<finstack_ai_kernel::EffectId>>,
    last_call: Mutex<Option<ValidatedToolCall>>,
    active_calls: Arc<AtomicUsize>,
    max_active_calls: Arc<AtomicUsize>,
}

impl ScriptedToolset {
    /// Construct a scripted Toolset with source-ordered call plans.
    #[must_use]
    pub fn new(tools: Arc<[ToolSpec]>, plans: Vec<ScriptedToolPlan>) -> Self {
        Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack.scripted_toolset"),
                metadata: Metadata::empty(),
            },
            tools,
            plans: Mutex::new(plans.into()),
            reconcile_results: Mutex::new(VecDeque::new()),
            control: ScriptedToolsetControl::default(),
            calls: Arc::new(AtomicUsize::new(0)),
            reconciles: Arc::new(AtomicUsize::new(0)),
            last_effect_id: Mutex::new(None),
            last_call: Mutex::new(None),
            active_calls: Arc::new(AtomicUsize::new(0)),
            max_active_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Queue tool reconcile outcomes consumed in order. Default is `Unknown`.
    #[must_use]
    pub fn with_reconcile_results(self, results: Vec<ToolReconcileResult>) -> Self {
        *self
            .reconcile_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = results.into();
        self
    }

    /// Clone the deterministic blocking controller.
    #[must_use]
    pub fn control(&self) -> ScriptedToolsetControl {
        self.control.clone()
    }

    /// Total calls observed.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }

    /// Number of reconcile calls observed.
    #[must_use]
    pub fn reconcile_count(&self) -> usize {
        self.reconciles.load(Ordering::Acquire)
    }

    /// Last effect identity observed by [`Toolset::call`] or [`Toolset::reconcile`].
    #[must_use]
    pub fn last_effect_id(&self) -> Option<finstack_ai_kernel::EffectId> {
        *self
            .last_effect_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Last frozen call observed by [`Toolset::call`].
    #[must_use]
    pub fn last_call(&self) -> Option<ValidatedToolCall> {
        self.last_call
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Current live streams.
    #[must_use]
    pub fn active_call_count(&self) -> usize {
        self.active_calls.load(Ordering::Acquire)
    }

    /// Maximum simultaneous live streams observed.
    #[must_use]
    pub fn max_active_call_count(&self) -> usize {
        self.max_active_calls.load(Ordering::Acquire)
    }
}

impl Toolset for ScriptedToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        *self
            .last_effect_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ctx.run.effect_id);
        *self
            .last_call
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(call);
        let plan = self
            .plans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        let control = self.control.clone();
        let active = Arc::clone(&self.active_calls);
        let maximum = Arc::clone(&self.max_active_calls);
        Box::pin(async move {
            let plan = plan.ok_or_else(|| {
                scripted_error(
                    "scripted_toolset_exhausted",
                    "no scripted tool call remains",
                )
            })?;
            if let Some(payload) = plan.panic_on_call {
                std::panic::resume_unwind(Box::new(payload.to_string()));
            }
            let current = active.fetch_add(1, Ordering::AcqRel) + 1;
            maximum.fetch_max(current, Ordering::AcqRel);
            Ok(Box::pin(ScriptedToolStream {
                actions: plan.actions.into(),
                cancellation: ctx.run.cancellation,
                cancellation_wait: None,
                control,
                active,
                active_gate: None,
            }) as ToolEventStream)
        })
    }

    fn reconcile(
        &self,
        ctx: ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        self.reconciles.fetch_add(1, Ordering::AcqRel);
        *self
            .last_effect_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ctx.run.effect_id);
        let result = self
            .reconcile_results
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or(ToolReconcileResult::Unknown);
        Box::pin(async move { Ok(result) })
    }
}

struct ScriptedToolStream {
    actions: VecDeque<ScriptedToolAction>,
    cancellation: CancellationSignal,
    cancellation_wait: Option<PortFuture<()>>,
    control: ScriptedToolsetControl,
    active: Arc<AtomicUsize>,
    active_gate: Option<Arc<str>>,
}

impl Drop for ScriptedToolStream {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl ScriptedToolStream {
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
}

impl Stream for ScriptedToolStream {
    type Item = Result<ToolStreamItem, ToolError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let Some(action) = self.actions.front().cloned() else {
                return Poll::Ready(None);
            };
            match action {
                ScriptedToolAction::Emit(item) => {
                    self.actions.pop_front();
                    return Poll::Ready(Some(item));
                }
                ScriptedToolAction::AwaitCancellation => {
                    if self.cancellation_poll(cx).is_ready() {
                        self.actions.pop_front();
                        return cancellation_error();
                    }
                    return Poll::Pending;
                }
                ScriptedToolAction::Block(name) => {
                    let gate = self.control.gate(name.clone());
                    if self.active_gate.as_ref() != Some(&name) {
                        gate.enter();
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
                        return cancellation_error();
                    }
                    return Poll::Pending;
                }
                ScriptedToolAction::Panic(payload) => {
                    std::panic::resume_unwind(Box::new(payload.to_string()));
                }
            }
        }
    }
}

fn cancellation_error() -> Poll<Option<Result<ToolStreamItem, ToolError>>> {
    Poll::Ready(Some(Err(scripted_error(
        "scripted_tool_cancelled",
        "scripted tool acknowledged effect cancellation",
    ))))
}

fn scripted_error(code: &'static str, message: &'static str) -> ToolError {
    ToolError::try_new(code, ErrorCategory::Tool, false, message, Metadata::empty())
        .unwrap_or_else(ToolError::from)
}
