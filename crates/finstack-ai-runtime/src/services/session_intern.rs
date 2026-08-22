//! Cancellation-safe singleflight construction for session runtimes.

use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex, Weak};

use finstack_ai_kernel::SessionId;

use super::session::{SessionError, SessionRuntime};
use crate::ports::journal::JournalStore;

type InternKey = (usize, SessionId);

enum InternSlot {
    Initializing(Arc<InitSignal>),
    Ready(Weak<SessionRuntime>),
}

#[derive(Default)]
struct InitSignal {
    generation: AtomicU64,
    waiters: Mutex<Vec<Waker>>,
}

impl InitSignal {
    fn wait(self: &Arc<Self>, generation: u64) -> InitWait {
        InitWait {
            signal: Arc::clone(self),
            generation,
        }
    }

    fn notify(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let waiters = self
            .waiters
            .lock()
            .map(|mut waiters| core::mem::take(&mut *waiters))
            .unwrap_or_default();
        for waiter in waiters {
            waiter.wake();
        }
    }
}

struct InitWait {
    signal: Arc<InitSignal>,
    generation: u64,
}

impl Future for InitWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        if self.signal.generation.load(Ordering::Acquire) != self.generation {
            return Poll::Ready(());
        }
        let Ok(mut waiters) = self.signal.waiters.lock() else {
            return Poll::Ready(());
        };
        if self.signal.generation.load(Ordering::Acquire) != self.generation {
            return Poll::Ready(());
        }
        if !waiters
            .iter()
            .any(|waiter| waiter.will_wake(context.waker()))
        {
            waiters.push(context.waker().clone());
        }
        Poll::Pending
    }
}

pub(super) enum InternDecision {
    Existing(Arc<SessionRuntime>),
    Lead(InternLeader),
    Wait(InternFollower),
}

pub(super) struct InternLeader {
    key: InternKey,
    signal: Arc<InitSignal>,
    completed: bool,
}

impl InternLeader {
    pub(super) fn complete(
        mut self,
        runtime: SessionRuntime,
    ) -> Result<Arc<SessionRuntime>, SessionError> {
        // The wasm runtime is single-threaded. Keeping `Arc` here preserves
        // one interned-session ownership model across native and wasm hosts.
        #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
        let runtime = Arc::new(runtime);
        with_interns(|map| {
            let is_owner = matches!(map.get(&self.key), Some(InternSlot::Initializing(signal)) if Arc::ptr_eq(signal, &self.signal));
            if !is_owner {
                return map.get(&self.key).and_then(|slot| match slot {
                    InternSlot::Ready(existing) => existing.upgrade(),
                    InternSlot::Initializing(_) => None,
                });
            }
            map.insert(self.key, InternSlot::Ready(Arc::downgrade(&runtime)));
            Some(Arc::clone(&runtime))
        })?
        .ok_or(SessionError::Poisoned)
        .inspect(|_| {
            self.completed = true;
            self.signal.notify();
        })
    }
}

impl Drop for InternLeader {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let _ = with_interns(|map| {
            let is_owner = matches!(map.get(&self.key), Some(InternSlot::Initializing(signal)) if Arc::ptr_eq(signal, &self.signal));
            if is_owner {
                map.remove(&self.key);
            }
        });
        self.signal.notify();
    }
}

pub(super) struct InternFollower {
    signal: Arc<InitSignal>,
    generation: u64,
}

impl InternFollower {
    pub(super) async fn wait(self) {
        self.signal.wait(self.generation).await;
    }
}

pub(super) fn decide(
    store: &Arc<dyn JournalStore>,
    session_id: SessionId,
) -> Result<InternDecision, SessionError> {
    let key = (store_key(store), session_id);
    with_interns(|map| match map.get(&key) {
        Some(InternSlot::Ready(runtime)) => {
            if let Some(runtime) = runtime.upgrade() {
                InternDecision::Existing(runtime)
            } else {
                let signal = Arc::new(InitSignal::default());
                map.insert(key, InternSlot::Initializing(Arc::clone(&signal)));
                InternDecision::Lead(InternLeader {
                    key,
                    signal,
                    completed: false,
                })
            }
        }
        Some(InternSlot::Initializing(signal)) => {
            let generation = signal.generation.load(Ordering::Acquire);
            InternDecision::Wait(InternFollower {
                signal: Arc::clone(signal),
                generation,
            })
        }
        None => {
            let signal = Arc::new(InitSignal::default());
            map.insert(key, InternSlot::Initializing(Arc::clone(&signal)));
            InternDecision::Lead(InternLeader {
                key,
                signal,
                completed: false,
            })
        }
    })
}

pub(super) fn existing(
    store: &Arc<dyn JournalStore>,
    session_id: SessionId,
) -> Result<Option<Arc<SessionRuntime>>, SessionError> {
    let key = (store_key(store), session_id);
    with_interns(|map| {
        map.get(&key).and_then(|slot| match slot {
            InternSlot::Ready(runtime) => runtime.upgrade(),
            InternSlot::Initializing(_) => None,
        })
    })
}

fn store_key(store: &Arc<dyn JournalStore>) -> usize {
    Arc::as_ptr(store).cast::<u8>() as usize
}

#[cfg(test)]
thread_local! {
    static FAIL_INTERNS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn with_interns<R>(
    f: impl FnOnce(&mut BTreeMap<InternKey, InternSlot>) -> R,
) -> Result<R, SessionError> {
    #[cfg(test)]
    if FAIL_INTERNS.with(std::cell::Cell::get) {
        return Err(SessionError::Poisoned);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static INTERNS: OnceLock<Mutex<BTreeMap<InternKey, InternSlot>>> = OnceLock::new();
        INTERNS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map(|mut map| f(&mut map))
            .map_err(|_| SessionError::Poisoned)
    }
    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        thread_local! {
            static INTERNS: RefCell<BTreeMap<InternKey, InternSlot>> =
                const { RefCell::new(BTreeMap::new()) };
        }
        INTERNS.with(|cell| {
            cell.try_borrow_mut()
                .map(|mut map| f(&mut map))
                .map_err(|_| SessionError::Poisoned)
        })
    }
}

#[cfg(test)]
pub(super) struct FailInterns;

#[cfg(test)]
impl FailInterns {
    pub(super) fn arm() -> Self {
        FAIL_INTERNS.with(|flag| flag.set(true));
        Self
    }
}

#[cfg(test)]
impl Drop for FailInterns {
    fn drop(&mut self) {
        FAIL_INTERNS.with(|flag| flag.set(false));
    }
}
