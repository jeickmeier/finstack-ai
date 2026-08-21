//! Executor-neutral synchronization used by session ownership.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::BTreeMap;
use std::sync::Mutex;

use super::session::SessionError;

#[derive(Default)]
struct GateState {
    locked: bool,
    next_waiter: u64,
    waiters: BTreeMap<u64, Waker>,
}

/// Cancellation-safe async mutex that does not depend on a native executor.
#[derive(Default)]
pub(super) struct StructuralGate {
    state: Mutex<GateState>,
}

impl StructuralGate {
    pub(super) fn acquire(&self) -> StructuralAcquire<'_> {
        StructuralAcquire {
            gate: self,
            waiter: None,
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, GateState>, SessionError> {
        self.state.lock().map_err(|_| SessionError::Poisoned)
    }
}

pub(super) struct StructuralAcquire<'a> {
    gate: &'a StructuralGate,
    waiter: Option<u64>,
}

impl<'a> Future for StructuralAcquire<'a> {
    type Output = Result<StructuralGuard<'a>, SessionError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = match self.gate.lock() {
            Ok(state) => state,
            Err(error) => return Poll::Ready(Err(error)),
        };
        if !state.locked {
            state.locked = true;
            if let Some(waiter) = self.waiter.take() {
                state.waiters.remove(&waiter);
            }
            drop(state);
            return Poll::Ready(Ok(StructuralGuard { gate: self.gate }));
        }

        let waiter = if let Some(waiter) = self.waiter {
            waiter
        } else {
            let waiter = state.next_waiter;
            state.next_waiter = state.next_waiter.wrapping_add(1);
            self.waiter = Some(waiter);
            waiter
        };
        let replace = state
            .waiters
            .get(&waiter)
            .is_none_or(|registered| !registered.will_wake(context.waker()));
        if replace {
            state.waiters.insert(waiter, context.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for StructuralAcquire<'_> {
    fn drop(&mut self) {
        let Some(waiter) = self.waiter else {
            return;
        };
        if let Ok(mut state) = self.gate.state.lock() {
            state.waiters.remove(&waiter);
        }
    }
}

pub(super) struct StructuralGuard<'a> {
    gate: &'a StructuralGate,
}

impl Drop for StructuralGuard<'_> {
    fn drop(&mut self) {
        let waiters = if let Ok(mut state) = self.gate.state.lock() {
            state.locked = false;
            core::mem::take(&mut state.waiters)
        } else {
            BTreeMap::new()
        };
        for (_, waiter) in waiters {
            waiter.wake();
        }
    }
}
