//! One bounded cancellation slot serviced by the journal owner, including
//! while it awaits an inline context or middleware invocation.

use std::future::{Future, poll_fn};
use std::sync::Mutex;
use std::task::{Poll, Waker};

use finstack_ai_kernel::{KernelInput, TransitionEnv};

use crate::commit::{CommitCoordinator, CommitCoordinatorError, CommitOutcome};
use crate::ids::{Clock, RandomSource};
use crate::ports::model::CancellationSignal;
use crate::run_types::{RunHandleError, result_fault_code};
use crate::settlement::{SettlementSources, drain_idle_cancellation};

#[cfg(feature = "native-tokio")]
type Reply = tokio::sync::oneshot::Sender<Result<CommitOutcome, RunHandleError>>;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
type Reply = super::host_task::oneshot::OneshotSender<Result<CommitOutcome, RunHandleError>>;

pub(crate) struct RunCommand {
    pub(crate) env: TransitionEnv,
    pub(crate) input: KernelInput,
    pub(crate) reply: Reply,
}

fn send_reply(reply: Reply, result: Result<CommitOutcome, RunHandleError>) {
    #[cfg(feature = "native-tokio")]
    let _ = reply.send(result);
    #[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
    reply.send(result);
}

#[derive(Default)]
struct ControlState {
    pending: Option<RunCommand>,
    waiter: Option<Waker>,
    space: CancellationSignal,
    closed: bool,
}

#[derive(Default)]
pub(crate) struct RunControl {
    state: Mutex<ControlState>,
}

impl RunControl {
    pub(crate) async fn push(&self, command: RunCommand) -> Result<(), RunHandleError> {
        loop {
            let space = {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| RunHandleError::IntakeClosed)?;
                if state.closed {
                    return Err(RunHandleError::ShuttingDown);
                }
                if state.pending.is_none() {
                    state.pending = Some(command);
                    let waiter = state.waiter.take();
                    drop(state);
                    if let Some(waiter) = waiter {
                        waiter.wake();
                    }
                    return Ok(());
                }
                state.space.clone()
            };
            space.cancelled().await;
        }
    }

    pub(crate) fn close(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.closed = true;
        let pending = state.pending.take();
        let waiter = state.waiter.take();
        let space = state.space.clone();
        drop(state);
        if let Some(command) = pending {
            send_reply(command.reply, Err(RunHandleError::ShuttingDown));
        }
        space.cancel();
        if let Some(waiter) = waiter {
            waiter.wake();
        }
    }

    async fn recv(&self) -> Option<RunCommand> {
        poll_fn(|cx| {
            let Ok(mut state) = self.state.lock() else {
                return Poll::Ready(None);
            };
            if let Some(command) = state.pending.take() {
                let space = std::mem::take(&mut state.space);
                drop(state);
                space.cancel();
                return Poll::Ready(Some(command));
            }
            if state.closed {
                return Poll::Ready(None);
            }
            state.waiter = Some(cx.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Exactly one owner calls this or the invocation waiter at a time.
    pub(crate) async fn next(
        &self,
        ordinary: impl Future<Output = Option<RunCommand>>,
    ) -> Option<RunCommand> {
        let mut ordinary = std::pin::pin!(ordinary);
        let mut control = std::pin::pin!(self.recv());
        poll_fn(|cx| {
            if let Poll::Ready(command) = control.as_mut().poll(cx) {
                return Poll::Ready(command);
            }
            ordinary.as_mut().poll(cx)
        })
        .await
    }
}

/// Keep the invocation alive while committing cancellation. Only after a
/// confirmed cancellation do we signal/drop it and reconcile its durable
/// intent; unauthorized commands leave the invocation untouched.
pub(crate) async fn await_invocation<C: Clock, R: RandomSource, T>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
    invocation: impl Future<Output = T>,
) -> Result<T, RunHandleError> {
    let Some(control) = coordinator.run_control.clone() else {
        return Ok(invocation.await);
    };
    let mut invocation = Box::pin(invocation);
    loop {
        let mut command = std::pin::pin!(control.recv());
        let ready = poll_fn(|cx| {
            if let Poll::Ready(command) = command.as_mut().poll(cx) {
                return Poll::Ready(Err(command));
            }
            invocation.as_mut().poll(cx).map(Ok)
        })
        .await;
        let command = match ready {
            Ok(output) => return Ok(output),
            Err(Some(command)) => command,
            Err(None) => {
                cancellation.cancel();
                return Err(RunHandleError::ShuttingDown);
            }
        };
        let result = coordinator
            .submit(command.env, command.input)
            .await
            .map_err(RunHandleError::Coordinator);
        let fault = result_fault_code(&result);
        let cancelled = coordinator.state().cancellation().is_some();
        send_reply(command.reply, result);
        if cancelled || fault.is_some() {
            cancellation.cancel();
            drop(invocation);
            if let Some(code) = fault {
                return Err(RunHandleError::Faulted { code });
            }
            drain_idle_cancellation(coordinator, sources, false).await?;
            return Err(RunHandleError::Coordinator(
                CommitCoordinatorError::Decision {
                    code: "invalid_phase_input",
                },
            ));
        }
    }
}
