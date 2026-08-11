//! Deterministic target-neutral Toolset leaf and scheduler controls.

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ErrorCategory, Metadata, ValidatedToolCall};
use finstack_ai_runtime::{
    CancellationSignal, PortFuture, ToolCallContext, ToolError, ToolEventStream, ToolSpec,
    ToolStreamItem, Toolset, ToolsetDescriptor,
};
use futures_core::Stream;

/// One fully expressive scripted tool-stream action.
#[derive(Debug, Clone)]
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
        self.gate(Arc::from(name.as_ref()))
            .entered
            .load(Ordering::Acquire)
    }

    fn gate(&self, name: Arc<str>) -> Arc<Gate> {
        self.gates
            .lock()
            .expect("scripted tool gate registry is not poisoned")
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
    control: ScriptedToolsetControl,
    calls: Arc<AtomicUsize>,
    active_calls: Arc<AtomicUsize>,
    max_active_calls: Arc<AtomicUsize>,
    cancellations: Arc<AtomicUsize>,
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
            control: ScriptedToolsetControl::default(),
            calls: Arc::new(AtomicUsize::new(0)),
            active_calls: Arc::new(AtomicUsize::new(0)),
            max_active_calls: Arc::new(AtomicUsize::new(0)),
            cancellations: Arc::new(AtomicUsize::new(0)),
        }
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

    /// Effect-local cancellation acknowledgements observed.
    #[must_use]
    pub fn cancellation_count(&self) -> usize {
        self.cancellations.load(Ordering::Acquire)
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
        _call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let plan = self
            .plans
            .lock()
            .expect("scripted tool plan queue is not poisoned")
            .pop_front();
        let control = self.control.clone();
        let active = Arc::clone(&self.active_calls);
        let maximum = Arc::clone(&self.max_active_calls);
        let cancellations = Arc::clone(&self.cancellations);
        Box::pin(async move {
            let plan = plan.ok_or_else(|| {
                scripted_error(
                    "scripted_toolset_exhausted",
                    "no scripted tool call remains",
                )
            })?;
            if let Some(payload) = plan.panic_on_call {
                panic!("{payload}");
            }
            let current = active.fetch_add(1, Ordering::AcqRel) + 1;
            maximum.fetch_max(current, Ordering::AcqRel);
            Ok(Box::pin(ScriptedToolStream {
                actions: plan.actions.into(),
                cancellation: ctx.run.cancellation,
                cancellation_wait: None,
                control,
                active,
                cancellations,
                active_gate: None,
            }) as ToolEventStream)
        })
    }
}

struct ScriptedToolStream {
    actions: VecDeque<ScriptedToolAction>,
    cancellation: CancellationSignal,
    cancellation_wait: Option<PortFuture<()>>,
    control: ScriptedToolsetControl,
    active: Arc<AtomicUsize>,
    cancellations: Arc<AtomicUsize>,
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

    fn cancellation_error(&self) -> Poll<Option<Result<ToolStreamItem, ToolError>>> {
        self.cancellations.fetch_add(1, Ordering::AcqRel);
        Poll::Ready(Some(Err(scripted_error(
            "scripted_tool_cancelled",
            "scripted tool acknowledged effect cancellation",
        ))))
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
                        return self.cancellation_error();
                    }
                    return Poll::Pending;
                }
                ScriptedToolAction::Block(name) => {
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
                        return self.cancellation_error();
                    }
                    return Poll::Pending;
                }
                ScriptedToolAction::Panic(payload) => panic!("{payload}"),
            }
        }
    }
}

fn scripted_error(code: &'static str, message: &'static str) -> ToolError {
    ToolError::try_new(code, ErrorCategory::Tool, false, message, Metadata::empty())
        .expect("scripted tool error is valid")
}
