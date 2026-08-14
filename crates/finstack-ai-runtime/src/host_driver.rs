//! Host-driven local executor hooks for `wasm-host` builds.
//!
//! This module has no wasm-bindgen dependency. The binding crate installs
//! `spawn_local` and timer callbacks during `init`. Native `wasm-host` tests
//! fall back to a thread-local cooperative queue pumped by [`yield_now`].

use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

use crate::{Clock, IdGenerationError, PortFuture, PortObject, RandomSource, Timestamp};

/// No host driver is installed for a requested operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverUnavailable;

impl std::fmt::Display for DriverUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("host driver is unavailable")
    }
}

impl std::error::Error for DriverUnavailable {}

/// The host driver deadline elapsed before the future completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeoutElapsed;

impl std::fmt::Display for TimeoutElapsed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("host driver deadline elapsed")
    }
}

impl std::error::Error for TimeoutElapsed {}

#[cfg(not(target_arch = "wasm32"))]
type Spawner = Arc<dyn Fn(PortFuture<()>) + Send + Sync>;
#[cfg(target_arch = "wasm32")]
type Spawner = Arc<dyn Fn(PortFuture<()>)>;
#[cfg(not(target_arch = "wasm32"))]
type SleepCallback = Box<dyn FnOnce() + Send + 'static>;
#[cfg(target_arch = "wasm32")]
type SleepCallback = Box<dyn FnOnce() + 'static>;
#[cfg(not(target_arch = "wasm32"))]
type Sleeper = Arc<dyn Fn(Duration, SleepCallback) + Send + Sync>;
#[cfg(target_arch = "wasm32")]
type Sleeper = Arc<dyn Fn(Duration, SleepCallback)>;
#[cfg(not(target_arch = "wasm32"))]
type LocalSleep = (Instant, SleepCallback);

thread_local! {
    static SPAWNER: RefCell<Option<Spawner>> = const { RefCell::new(None) };
    static SLEEPER: RefCell<Option<Sleeper>> = const { RefCell::new(None) };
    static CLOCK: RefCell<Option<Arc<dyn Clock>>> = const { RefCell::new(None) };
    static RANDOM: RefCell<Option<Arc<dyn RandomSource>>> = const { RefCell::new(None) };
    #[cfg(not(target_arch = "wasm32"))]
    static LOCAL_TASKS: RefCell<Vec<PortFuture<()>>> = const { RefCell::new(Vec::new()) };
    #[cfg(not(target_arch = "wasm32"))]
    static LOCAL_SLEEPS: RefCell<Vec<LocalSleep>> = const { RefCell::new(Vec::new()) };
}

/// Install the host spawn function used by [`spawn`].
pub fn install_spawner(spawner: impl Fn(PortFuture<()>) + PortObject) {
    SPAWNER.with(|slot| *slot.borrow_mut() = Some(Arc::new(spawner)));
}

/// Install the host sleep function used by [`timeout`].
pub fn install_sleeper(sleeper: impl Fn(Duration, SleepCallback) + PortObject) {
    SLEEPER.with(|slot| *slot.borrow_mut() = Some(Arc::new(sleeper)));
}

/// Install the clock used by [`InstalledClock`].
pub fn install_clock(clock: Arc<dyn Clock>) {
    CLOCK.with(|slot| *slot.borrow_mut() = Some(clock));
}

/// Install the entropy source used by [`InstalledRandom`].
pub fn install_random(random: Arc<dyn RandomSource>) {
    RANDOM.with(|slot| *slot.borrow_mut() = Some(random));
}

/// Spawn one detached host-driver future.
///
/// # Errors
///
/// Returns [`DriverUnavailable`] only when a required installed spawner is
/// missing on `wasm32`. Native `wasm-host` tests queue the future locally.
pub fn spawn(future: PortFuture<()>) -> Result<(), DriverUnavailable> {
    if let Some(spawner) = SPAWNER.with(|slot| slot.borrow().clone()) {
        spawner(future);
        return Ok(());
    }
    #[cfg(target_arch = "wasm32")]
    {
        return Err(DriverUnavailable);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        LOCAL_TASKS.with(|tasks| tasks.borrow_mut().push(future));
        Ok(())
    }
}

/// Cooperatively yield one turn to the host driver.
pub async fn yield_now() {
    pump_local();
    #[cfg(target_arch = "wasm32")]
    {
        // Yield through the installed sleeper so wait loops cannot starve
        // host promises, AbortSignals, or other spawn_local tasks.
        sleep(Duration::ZERO).await;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut yielded = false;
        poll_fn(|cx| {
            if yielded {
                return Poll::Ready(());
            }
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;
    }
}

/// Await a future until the host driver deadline elapses.
///
/// # Errors
///
/// Returns [`TimeoutElapsed`] when the future does not complete before the
/// requested duration.
pub async fn timeout<F>(duration: Duration, future: F) -> Result<F::Output, TimeoutElapsed>
where
    F: Future,
{
    let sleep = sleep(duration);
    let mut future = std::pin::pin!(future);
    let mut sleep = std::pin::pin!(sleep);
    poll_fn(|cx| {
        if let Poll::Ready(output) = future.as_mut().poll(cx) {
            return Poll::Ready(Ok(output));
        }
        if sleep.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(TimeoutElapsed));
        }
        Poll::Pending
    })
    .await
}

async fn sleep(duration: Duration) {
    let (signal, done) = oneshot();
    if let Some(sleeper) = SLEEPER.with(|slot| slot.borrow().clone()) {
        sleeper(duration, Box::new(move || signal.send()));
        done.await;
        return;
    }
    if duration.is_zero() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let deadline = Instant::now() + duration;
        LOCAL_SLEEPS.with(|sleeps| {
            sleeps
                .borrow_mut()
                .push((deadline, Box::new(move || signal.send())));
        });
        done.await;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (signal, done, duration);
    }
}

/// Drive queued local tasks once. Native `wasm-host` tests use this as a
/// cooperative executor step.
pub fn drive_local() {
    pump_local();
}

fn pump_local() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let now = Instant::now();
        let due = LOCAL_SLEEPS.with(|sleeps| {
            let mut sleeps = sleeps.borrow_mut();
            let mut due = Vec::new();
            let mut remain = Vec::new();
            for (deadline, callback) in sleeps.drain(..) {
                if deadline <= now {
                    due.push(callback);
                } else {
                    remain.push((deadline, callback));
                }
            }
            *sleeps = remain;
            due
        });
        for callback in due {
            callback();
        }
        LOCAL_TASKS.with(|tasks| {
            let mut tasks = tasks.borrow_mut();
            let mut idx = 0;
            while idx < tasks.len() {
                let waker = noop_waker();
                let mut cx = Context::from_waker(&waker);
                if tasks[idx].as_mut().poll(&mut cx).is_ready() {
                    drop(tasks.remove(idx));
                } else {
                    idx += 1;
                }
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn noop_waker() -> Waker {
    Waker::noop().clone()
}

fn oneshot() -> (OneshotSignal, OneshotWait) {
    let inner = Arc::new(OneshotInner {
        fired: std::sync::atomic::AtomicBool::new(false),
        waker: std::sync::Mutex::new(None),
    });
    (
        OneshotSignal {
            inner: Arc::clone(&inner),
        },
        OneshotWait { inner },
    )
}

struct OneshotInner {
    fired: std::sync::atomic::AtomicBool,
    waker: std::sync::Mutex<Option<Waker>>,
}

struct OneshotSignal {
    inner: Arc<OneshotInner>,
}

impl OneshotSignal {
    fn send(self) {
        self.inner
            .fired
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut waker) = self.inner.waker.lock()
            && let Some(waker) = waker.take()
        {
            waker.wake();
        }
    }
}

struct OneshotWait {
    inner: Arc<OneshotInner>,
}

impl Future for OneshotWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.inner.fired.load(std::sync::atomic::Ordering::Acquire) {
            return Poll::Ready(());
        }
        if let Ok(mut waker) = self.inner.waker.lock() {
            *waker = Some(cx.waker().clone());
        }
        if self.inner.fired.load(std::sync::atomic::Ordering::Acquire) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Cloneable one-way notification used by host-driven SDK state machines.
#[derive(Clone, Default)]
pub struct Signal {
    inner: Arc<SignalInner>,
}

#[derive(Default)]
struct SignalInner {
    generation: std::sync::atomic::AtomicU64,
    wakers: std::sync::Mutex<Vec<Waker>>,
}

impl Signal {
    /// Create an empty notification signal.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a waiter before checking its guarded state.
    pub fn notified(&self) -> impl Future<Output = ()> + '_ {
        SignalWait {
            inner: Arc::clone(&self.inner),
            seen: self
                .inner
                .generation
                .load(std::sync::atomic::Ordering::Acquire),
        }
    }

    /// Wake every waiter registered before this call.
    pub fn notify_waiters(&self) {
        self.inner
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let wakers = self
            .inner
            .wakers
            .lock()
            .map(|mut wakers| std::mem::take(&mut *wakers))
            .unwrap_or_default();
        for waker in wakers {
            waker.wake();
        }
    }
}

struct SignalWait {
    inner: Arc<SignalInner>,
    seen: u64,
}

impl Future for SignalWait {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let current = self
            .inner
            .generation
            .load(std::sync::atomic::Ordering::Acquire);
        if current != self.seen {
            return Poll::Ready(());
        }
        if let Ok(mut wakers) = self.inner.wakers.lock() {
            wakers.push(cx.waker().clone());
        }
        let current = self
            .inner
            .generation
            .load(std::sync::atomic::Ordering::Acquire);
        if current == self.seen {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

/// Clock that reads the installed host clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct InstalledClock;

impl Clock for InstalledClock {
    fn now(&self) -> Result<Timestamp, IdGenerationError> {
        CLOCK.with(|slot| {
            let clock = slot.borrow();
            let clock = clock
                .as_ref()
                .ok_or_else(|| IdGenerationError::Source("host clock is not installed".into()))?;
            (**clock).now()
        })
    }
}

/// Random source that reads the installed host entropy.
#[derive(Clone, Copy, Debug, Default)]
pub struct InstalledRandom;

impl RandomSource for InstalledRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        RANDOM.with(|slot| {
            let random = slot.borrow();
            let random = random
                .as_ref()
                .ok_or_else(|| IdGenerationError::Source("host random is not installed".into()))?;
            (**random).fill_bytes(buf)
        })
    }
}
