use std::sync::{Arc, Mutex, OnceLock, Weak};

use finstack_ai_kernel::{CancelRequested, CancellationInitiator, KernelInput, OperationLocator};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::CommitCoordinator;
use finstack_ai_runtime::{EventBatch, EventSubscription, RunHandle};

#[cfg(feature = "native-tokio")]
use finstack_ai_kernel::{InteractionRequest, InteractionResolution, InteractionSettled};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::InteractionRouter;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver as driver;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::native_driver as driver;

use super::prepare::{NativeIds, submit};
use super::types::{AgentRunError, AgentRunOutput};
use crate::ChildRunPolicy;

pub(super) struct AgentRunInner {
    pub(super) locator: OperationLocator,
    pub(super) store: Arc<dyn finstack_ai_runtime::JournalStore>,
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        allow(dead_code)
    )]
    pub(super) child_runs: ChildRunPolicy,
    pub(super) cancellation_initiator: CancellationInitiator,
    pub(super) handle: Mutex<Option<Result<RunHandle, AgentRunError>>>,
    pub(super) handle_ready: driver::Signal,
    pub(super) result: Mutex<Option<Result<AgentRunOutput, AgentRunError>>>,
    pub(super) result_ready: driver::Signal,
    pub(super) events: Mutex<EventStreamState>,
    pub(super) events_fault: OnceLock<AgentRunError>,
    pub(super) cancellation: Mutex<CancellationState>,
    pub(super) cancellation_ready: driver::Signal,
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        allow(dead_code)
    )]
    pub(super) children: Mutex<Vec<AgentRun>>,
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        allow(dead_code)
    )]
    pub(super) remote_invoker: Mutex<Option<Arc<dyn finstack_ai_runtime::AgentInvoker>>>,
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        allow(dead_code)
    )]
    pub(super) remote_child: Option<finstack_ai_kernel::ChildRunLocator>,
}

const EVENT_LOCK_POISONED: &str = "run event lock is poisoned";
const RESULT_LOCK_POISONED: &str = "run result lock is poisoned";
const HANDLE_LOCK_POISONED: &str = "run handle lock is poisoned";

fn record_events_fault(inner: &AgentRunInner, message: &'static str) {
    let _ = inner
        .events_fault
        .set(AgentRunError::runtime_message(message));
    inner.result_ready.notify_waiters();
    inner.handle_ready.notify_waiters();
}

fn events_fault(inner: &AgentRunInner) -> Result<(), AgentRunError> {
    match inner.events_fault.get() {
        Some(error) => Err(error.clone()),
        None => Ok(()),
    }
}

pub(super) enum EventStreamState {
    Waiting,
    Active(EventSubscription),
    Busy,
    CloseRequested,
    Closed,
    StartupFailed(AgentRunError),
}

struct EventConsumerGuard {
    inner: Arc<AgentRunInner>,
    subscription: Option<EventSubscription>,
}

impl EventConsumerGuard {
    fn subscription_mut(&mut self) -> Result<&mut EventSubscription, AgentRunError> {
        self.subscription.as_mut().ok_or_else(|| {
            record_events_fault(&self.inner, EVENT_LOCK_POISONED);
            AgentRunError::runtime_message(EVENT_LOCK_POISONED)
        })
    }

    fn finish(mut self, batch: Option<EventBatch>) -> Result<Option<EventBatch>, AgentRunError> {
        let mut state = self.inner.events.lock().map_err(|_| {
            record_events_fault(&self.inner, EVENT_LOCK_POISONED);
            AgentRunError::runtime_message(EVENT_LOCK_POISONED)
        })?;
        let Some(mut subscription) = self.subscription.take() else {
            record_events_fault(&self.inner, EVENT_LOCK_POISONED);
            return Err(AgentRunError::runtime_message(EVENT_LOCK_POISONED));
        };
        match &*state {
            EventStreamState::CloseRequested | EventStreamState::Closed => {
                subscription.close();
                *state = EventStreamState::Closed;
                Ok(None)
            }
            EventStreamState::Busy => {
                if batch.is_some() {
                    *state = EventStreamState::Active(subscription);
                } else {
                    *state = EventStreamState::Closed;
                }
                Ok(batch)
            }
            _ => {
                subscription.close();
                *state = EventStreamState::Closed;
                Err(AgentRunError::runtime_message(
                    "run event consumer state is inconsistent",
                ))
            }
        }
    }
}

impl Drop for EventConsumerGuard {
    fn drop(&mut self) {
        let Some(mut subscription) = self.subscription.take() else {
            return;
        };
        let Ok(mut state) = self.inner.events.lock() else {
            subscription.close();
            record_events_fault(&self.inner, EVENT_LOCK_POISONED);
            return;
        };
        if matches!(&*state, EventStreamState::Busy) {
            *state = EventStreamState::Active(subscription);
        } else {
            subscription.close();
            *state = EventStreamState::Closed;
        }
    }
}

#[derive(Default)]
pub(super) struct CancellationState {
    pub(super) started: bool,
    pub(super) result: Option<Result<(), AgentRunError>>,
}

/// Cloneable control and observation handle for one Rust-owned native run.
///
/// Dropping every clone detaches local observation but does not cancel the
/// durable run. Call [`AgentRun::cancel`] for explicit durable cancellation.
#[derive(Clone)]
pub struct AgentRun {
    pub(super) inner: Arc<AgentRunInner>,
}

impl AgentRun {
    /// Borrow the immutable durable locator allocated before run execution.
    #[must_use]
    pub fn locator(&self) -> &OperationLocator {
        &self.inner.locator
    }

    /// Borrow the journal store that owns this run.
    #[must_use]
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        allow(dead_code)
    )]
    pub(crate) fn journal_store(&self) -> &Arc<dyn finstack_ai_runtime::JournalStore> {
        &self.inner.store
    }

    /// Live session handle for this run. Does not respawn parked runs.
    #[must_use]
    pub fn session(&self) -> crate::Session {
        crate::Session::pending(
            Arc::clone(&self.inner.store),
            self.inner.locator.session_id,
            Arc::clone(&self.inner.locator.tenant_scope),
        )
    }

    /// List the outstanding typed interaction for this run (0 or 1).
    ///
    /// Native-only. Browser WASM list/resolve stays on the worker client.
    ///
    /// The owned handle treats an unpublished or not-yet-accepted journal as
    /// empty. After accept, listing goes through [`InteractionRouter`].
    ///
    /// # Errors
    ///
    /// Returns a stable runtime failure when the authenticated locator cannot
    /// be listed through [`InteractionRouter`].
    #[cfg(feature = "native-tokio")]
    pub async fn list_interactions(&self) -> Result<Vec<InteractionRequest>, AgentRunError> {
        let CancellationInitiator::Principal {
            principal,
            authorization,
        } = &self.inner.cancellation_initiator
        else {
            return Err(AgentRunError::runtime_message(
                "interaction list requires a principal-authored run",
            ));
        };
        let Ok(recovered) = CommitCoordinator::recover(
            Arc::clone(&self.inner.store),
            self.inner.locator.session_id,
        )
        .await
        else {
            return Ok(Vec::new());
        };
        if recovered.state().accepted.is_none() {
            return Ok(Vec::new());
        }
        let router = InteractionRouter::trusted(Arc::clone(&self.inner.store))
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        router
            .list(
                &self.inner.locator,
                principal,
                authorization,
                NativeIds::now()?,
            )
            .await
            .map_err(|error| AgentRunError::runtime_message(error.to_string()))
    }

    /// Resolve the outstanding interaction through the live run handle.
    ///
    /// Native-only. Browser WASM list/resolve stays on the worker client.
    ///
    /// # Arguments
    ///
    /// * `resolution` - Binding-neutral settlement for the outstanding interaction.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime failure when the handle is unavailable or the
    /// settlement is rejected.
    #[cfg(feature = "native-tokio")]
    pub async fn resolve_interaction(
        &self,
        resolution: InteractionResolution,
    ) -> Result<(), AgentRunError> {
        let handle = self.runtime_handle().await?;
        submit(
            &handle,
            NativeIds::interaction_resolve_environment()?,
            KernelInput::InteractionSettled(InteractionSettled::Resolved(resolution)),
        )
        .await
    }

    /// Wait for the final committed result.
    ///
    /// The result is retained, so multiple callers observe the same terminal
    /// value without rerunning any model or tool.
    ///
    /// # Errors
    ///
    /// Returns the stable configuration, runtime, cancellation, or timeout
    /// error that settled the run.
    pub async fn result(&self) -> Result<AgentRunOutput, AgentRunError> {
        loop {
            events_fault(&self.inner)?;
            let notified = self.inner.result_ready.notified();
            let result = self
                .inner
                .result
                .lock()
                .map_err(|_| {
                    record_events_fault(&self.inner, RESULT_LOCK_POISONED);
                    AgentRunError::runtime_message(RESULT_LOCK_POISONED)
                })?
                .clone();
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }

    /// Submit one idempotent durable cancellation request.
    ///
    /// Cancellation is explicit and independent of Python/Rust handle drops.
    /// Repeated calls share the first submission outcome.
    ///
    /// # Errors
    ///
    /// Returns a stable runtime failure if run startup or cancellation commit
    /// fails.
    pub async fn cancel(&self) -> Result<(), AgentRunError> {
        let should_start = {
            let mut cancellation =
                self.inner.cancellation.lock().map_err(|_| {
                    AgentRunError::runtime_message("run cancellation lock is poisoned")
                })?;
            if let Some(result) = cancellation.result.clone() {
                return result;
            }
            if cancellation.started {
                false
            } else {
                cancellation.started = true;
                true
            }
        };
        if should_start {
            let run = self.clone();
            if let Err(error) = driver::spawn(Box::pin(async move {
                let result = run.submit_cancellation().await;
                if let Ok(mut cancellation) = run.inner.cancellation.lock() {
                    cancellation.result = Some(result);
                }
                run.inner.cancellation_ready.notify_waiters();
            })) {
                let error = AgentRunError::runtime_message(error.to_string());
                let mut cancellation = self.inner.cancellation.lock().map_err(|_| {
                    AgentRunError::runtime_message("run cancellation lock is poisoned")
                })?;
                cancellation.result = Some(Err(error));
                self.inner.cancellation_ready.notify_waiters();
            }
        }
        loop {
            let notified = self.inner.cancellation_ready.notified();
            let result = self
                .inner
                .cancellation
                .lock()
                .map_err(|_| AgentRunError::runtime_message("run cancellation lock is poisoned"))?
                .result
                .clone();
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }

    /// Receive the next bounded transport batch in source order.
    ///
    /// Exactly one consumer may advance this subscription. `None` means the
    /// event hub closed after terminal settlement or explicit event closure.
    ///
    /// # Errors
    ///
    /// Returns the stable startup failure if the run could not publish its
    /// event subscription.
    pub async fn next_event_batch(&self) -> Result<Option<EventBatch>, AgentRunError> {
        loop {
            events_fault(&self.inner)?;
            let subscription = {
                let mut state = self.inner.events.lock().map_err(|_| {
                    record_events_fault(&self.inner, EVENT_LOCK_POISONED);
                    AgentRunError::runtime_message(EVENT_LOCK_POISONED)
                })?;
                match &*state {
                    EventStreamState::Waiting
                    | EventStreamState::Busy
                    | EventStreamState::CloseRequested => None,
                    EventStreamState::Closed => return Ok(None),
                    EventStreamState::StartupFailed(error) => return Err(error.clone()),
                    EventStreamState::Active(_) => {
                        match std::mem::replace(&mut *state, EventStreamState::Busy) {
                            EventStreamState::Active(subscription) => Some(subscription),
                            other => {
                                *state = other;
                                None
                            }
                        }
                    }
                }
            };
            let Some(subscription) = subscription else {
                driver::yield_now().await;
                continue;
            };
            let mut guard = EventConsumerGuard {
                inner: Arc::clone(&self.inner),
                subscription: Some(subscription),
            };
            let batch = guard.subscription_mut()?.next_batch().await;
            events_fault(&self.inner)?;
            return guard.finish(batch);
        }
    }

    /// Close frontend event delivery without cancelling the owning run.
    pub fn close_events(&self) {
        let Ok(mut state) = self.inner.events.lock() else {
            record_events_fault(&self.inner, EVENT_LOCK_POISONED);
            return;
        };
        match &mut *state {
            EventStreamState::Active(subscription) => {
                subscription.close();
                *state = EventStreamState::Closed;
            }
            EventStreamState::Busy => *state = EventStreamState::CloseRequested,
            _ => *state = EventStreamState::Closed,
        }
    }

    pub(crate) async fn runtime_handle(&self) -> Result<RunHandle, AgentRunError> {
        loop {
            events_fault(&self.inner)?;
            let notified = self.inner.handle_ready.notified();
            let result = self
                .inner
                .handle
                .lock()
                .map_err(|_| {
                    record_events_fault(&self.inner, HANDLE_LOCK_POISONED);
                    AgentRunError::runtime_message(HANDLE_LOCK_POISONED)
                })?
                .clone();
            if let Some(result) = result {
                return result;
            }
            notified.await;
        }
    }

    pub(super) async fn submit_cancellation(&self) -> Result<(), AgentRunError> {
        if self
            .inner
            .result
            .lock()
            .map_err(|_| AgentRunError::runtime_message("run result lock is poisoned"))?
            .is_some()
        {
            return Ok(());
        }
        if let Some(locator) = self.inner.remote_child.as_ref() {
            let invoker = self
                .inner
                .remote_invoker
                .lock()
                .map_err(|_| AgentRunError::runtime_message("run remote invoker lock is poisoned"))?
                .clone();
            if let Some(invoker) = invoker {
                return invoker
                    .cancel(locator)
                    .await
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()));
            }
            return Err(AgentRunError::configuration(
                crate::AGENT_RUN_INVALID_CONFIGURATION,
                "remote child cancel has no installed invoker",
            ));
        }
        let handle = self.runtime_handle().await?;
        submit(
            &handle,
            NativeIds::cancellation_environment()?,
            KernelInput::CancelRequested(CancelRequested {
                initiator: self.inner.cancellation_initiator.clone(),
                reason: Some(Arc::from("frontend cancellation")),
            }),
        )
        .await?;
        #[cfg(feature = "native-tokio")]
        Box::pin(self.fan_out_cancellation()).await?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn clear_child_handles(&self) {
        if let Ok(mut children) = self.inner.children.lock() {
            children.clear();
        }
    }

    #[cfg(test)]
    pub(super) fn poison_events_lock(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.inner.events.lock().expect("event lock");
            panic!("poison");
        }));
    }
}

pub(super) fn publish_start_failure(execution: &Weak<AgentRunInner>, error: &AgentRunError) {
    let Some(inner) = execution.upgrade() else {
        return;
    };
    if let Ok(mut handle) = inner.handle.lock() {
        *handle = Some(Err(error.clone()));
    } else {
        record_events_fault(&inner, HANDLE_LOCK_POISONED);
    }
    inner.handle_ready.notify_waiters();
    match inner.events.lock() {
        Ok(mut events) if matches!(*events, EventStreamState::Waiting) => {
            *events = EventStreamState::StartupFailed(error.clone());
        }
        Ok(_) => {}
        Err(_) => record_events_fault(&inner, EVENT_LOCK_POISONED),
    }
}

pub(super) fn publish_started(
    execution: &Weak<AgentRunInner>,
    runtime_handle: RunHandle,
    subscription: EventSubscription,
) {
    let Some(inner) = execution.upgrade() else {
        return;
    };
    if let Ok(mut handle) = inner.handle.lock() {
        *handle = Some(Ok(runtime_handle));
    } else {
        record_events_fault(&inner, HANDLE_LOCK_POISONED);
    }
    inner.handle_ready.notify_waiters();
    match inner.events.lock() {
        Ok(mut events) if matches!(*events, EventStreamState::Waiting) => {
            *events = EventStreamState::Active(subscription);
        }
        Ok(_) => {}
        Err(_) => record_events_fault(&inner, EVENT_LOCK_POISONED),
    }
}

pub(super) fn publish_result(
    execution: &Weak<AgentRunInner>,
    result: Result<AgentRunOutput, AgentRunError>,
) {
    let Some(inner) = execution.upgrade() else {
        return;
    };
    if let Ok(mut retained) = inner.result.lock() {
        *retained = Some(result);
    } else {
        record_events_fault(&inner, RESULT_LOCK_POISONED);
    }
    inner.result_ready.notify_waiters();
}
