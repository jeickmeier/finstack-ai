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

use crate::Timestamp;
use crate::ids::{Clock, IdGenerationError, RandomSource};
use crate::ports::{PortFuture, PortObject};

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
/// One-shot callback fired by the installed host timer.
pub type SleepCallback = Box<dyn FnOnce() + Send + 'static>;
#[cfg(target_arch = "wasm32")]
/// One-shot callback fired by the installed host timer.
pub type SleepCallback = Box<dyn FnOnce() + 'static>;
#[cfg(not(target_arch = "wasm32"))]
type Sleeper = Arc<dyn Fn(Duration, SleepCallback) + Send + Sync>;
#[cfg(target_arch = "wasm32")]
type Sleeper = Arc<dyn Fn(Duration, SleepCallback)>;
#[cfg(not(target_arch = "wasm32"))]
type LocalSleep = (Instant, SleepCallback);

thread_local! {
    static DRIVER: RefCell<Option<HostDriverHooks>> = const { RefCell::new(None) };
    #[cfg(not(target_arch = "wasm32"))]
    static LOCAL_TASKS: RefCell<Vec<PortFuture<()>>> = const { RefCell::new(Vec::new()) };
    #[cfg(not(target_arch = "wasm32"))]
    static LOCAL_SLEEPS: RefCell<Vec<LocalSleep>> = const { RefCell::new(Vec::new()) };
}

/// One coherent set of browser-host executor, time, clock, and entropy hooks.
///
/// Keeping the hooks in one value prevents a run from observing a mixture of
/// old and new host capabilities while a binding is being initialized.
#[derive(Clone)]
pub struct HostDriverHooks {
    spawner: Spawner,
    sleeper: Sleeper,
    clock: Arc<dyn Clock>,
    random: Arc<dyn RandomSource>,
}

impl HostDriverHooks {
    /// Construct a complete host-driver snapshot.
    #[must_use]
    pub fn new(
        spawner: impl Fn(PortFuture<()>) + PortObject,
        sleeper: impl Fn(Duration, SleepCallback) + PortObject,
        clock: Arc<dyn Clock>,
        random: Arc<dyn RandomSource>,
    ) -> Self {
        Self {
            spawner: Arc::new(spawner),
            sleeper: Arc::new(sleeper),
            clock,
            random,
        }
    }

    /// Construct hooks backed by the native cooperative test executor.
    ///
    /// This is available only for non-WASM `wasm-host` builds and keeps tests
    /// on the same atomic installation path as browser bindings.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    #[must_use]
    pub(crate) fn local(clock: Arc<dyn Clock>, random: Arc<dyn RandomSource>) -> Self {
        Self::new(
            |future| LOCAL_TASKS.with(|tasks| tasks.borrow_mut().push(future)),
            |duration, callback| {
                LOCAL_SLEEPS.with(|sleeps| {
                    sleeps
                        .borrow_mut()
                        .push((Instant::now() + duration, callback));
                });
            },
            clock,
            random,
        )
    }
}

/// Atomically replace every host-driver hook for the current host thread.
pub fn install_driver(driver: HostDriverHooks) {
    DRIVER.with(|slot| *slot.borrow_mut() = Some(driver));
}

/// Abort and completion handle for one host-driver task.
#[derive(Clone)]
pub struct HostTaskHandle {
    state: Arc<HostTaskState>,
}

impl HostTaskHandle {
    /// Request cooperative abortion of the task wrapper.
    pub fn abort(&self) {
        self.state
            .aborted
            .store(true, std::sync::atomic::Ordering::Release);
        self.state.wake();
    }

    /// Wait until the wrapped future has completed or has been dropped.
    pub async fn completed(&self) {
        poll_fn(|cx| {
            if self.is_completed() {
                return Poll::Ready(());
            }
            if let Ok(mut wakers) = self.state.completion_wakers.lock()
                && !wakers.iter().any(|waker| waker.will_wake(cx.waker()))
            {
                wakers.push(cx.waker().clone());
            }
            if self.is_completed() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    /// Whether the wrapped task has completed or been dropped.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        self.state
            .completed
            .load(std::sync::atomic::Ordering::Acquire)
    }
}

struct HostTaskState {
    aborted: std::sync::atomic::AtomicBool,
    completed: std::sync::atomic::AtomicBool,
    task_waker: std::sync::Mutex<Option<Waker>>,
    completion_wakers: std::sync::Mutex<Vec<Waker>>,
}

impl HostTaskState {
    fn wake(&self) {
        if let Ok(mut waker) = self.task_waker.lock()
            && let Some(waker) = waker.take()
        {
            waker.wake();
        }
    }

    fn complete(&self) {
        if !self
            .completed
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            let wakers = self
                .completion_wakers
                .lock()
                .map(|mut wakers| std::mem::take(&mut *wakers))
                .unwrap_or_default();
            for waker in wakers {
                waker.wake();
            }
        }
    }
}

struct HostTask {
    future: Option<PortFuture<()>>,
    state: Arc<HostTaskState>,
}

impl Future for HostTask {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self
            .state
            .aborted
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.future.take();
            self.state.complete();
            return Poll::Ready(());
        }
        if let Ok(mut waker) = self.state.task_waker.lock() {
            *waker = Some(cx.waker().clone());
        }
        let ready = self
            .future
            .as_mut()
            .is_none_or(|future| future.as_mut().poll(cx).is_ready());
        if ready {
            self.future.take();
            self.state.complete();
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl Drop for HostTask {
    fn drop(&mut self) {
        self.future.take();
        self.state.complete();
    }
}

/// Spawn one owned host-driver future.
///
/// # Errors
///
/// Returns [`DriverUnavailable`] only when a required installed spawner is
/// missing on `wasm32`. Native `wasm-host` tests queue the future locally.
pub fn spawn(future: PortFuture<()>) -> Result<HostTaskHandle, DriverUnavailable> {
    let state = Arc::new(HostTaskState {
        aborted: std::sync::atomic::AtomicBool::new(false),
        completed: std::sync::atomic::AtomicBool::new(false),
        task_waker: std::sync::Mutex::new(None),
        completion_wakers: std::sync::Mutex::new(Vec::new()),
    });
    let handle = HostTaskHandle {
        state: Arc::clone(&state),
    };
    let future: PortFuture<()> = Box::pin(HostTask {
        future: Some(future),
        state,
    });
    if let Some(spawner) =
        DRIVER.with(|slot| slot.borrow().as_ref().map(|hooks| hooks.spawner.clone()))
    {
        spawner(future);
        return Ok(handle);
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(DriverUnavailable)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        LOCAL_TASKS.with(|tasks| tasks.borrow_mut().push(future));
        Ok(handle)
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

pub(crate) async fn sleep(duration: Duration) {
    let (signal, done) = oneshot();
    if let Some(sleeper) =
        DRIVER.with(|slot| slot.borrow().as_ref().map(|hooks| hooks.sleeper.clone()))
    {
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
        let mut ready = LOCAL_TASKS.with(|tasks| std::mem::take(&mut *tasks.borrow_mut()));
        let mut pending = Vec::with_capacity(ready.len());
        for mut task in ready.drain(..) {
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            if task.as_mut().poll(&mut cx).is_pending() {
                pending.push(task);
            }
        }
        // Poll without retaining the RefCell borrow: a running host task may
        // legitimately spawn bounded child work. New children stay ahead of
        // their still-pending parent for the next cooperative turn.
        LOCAL_TASKS.with(|tasks| {
            let mut tasks = tasks.borrow_mut();
            tasks.extend(pending);
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
        DRIVER.with(|slot| {
            let driver = slot.borrow();
            let clock = driver
                .as_ref()
                .map(|hooks| &hooks.clock)
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
        DRIVER.with(|slot| {
            let driver = slot.borrow();
            let random = driver
                .as_ref()
                .map(|hooks| &hooks.random)
                .ok_or_else(|| IdGenerationError::Source("host random is not installed".into()))?;
            (**random).fill_bytes(buf)
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    struct TestClock;

    impl Clock for TestClock {
        fn now(&self) -> Result<Timestamp, IdGenerationError> {
            Timestamp::from_unix_ms(1_000).map_err(Into::into)
        }
    }

    struct TestRandom;

    impl RandomSource for TestRandom {
        fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
            buf.fill(7);
            Ok(())
        }
    }

    struct PendingUntilDropped(Arc<AtomicBool>);

    impl Future for PendingUntilDropped {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl Drop for PendingUntilDropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn abort_drops_the_future_before_reporting_completion() {
        install_driver(HostDriverHooks::local(
            Arc::new(TestClock),
            Arc::new(TestRandom),
        ));
        let dropped = Arc::new(AtomicBool::new(false));
        let handle = spawn(Box::pin(PendingUntilDropped(Arc::clone(&dropped)))).expect("spawn");

        drive_local();
        assert!(!handle.is_completed());
        handle.abort();
        drive_local();

        assert!(dropped.load(Ordering::Acquire));
        assert!(handle.is_completed());
    }

    #[test]
    fn one_install_replaces_clock_and_random_together() {
        install_driver(HostDriverHooks::local(
            Arc::new(TestClock),
            Arc::new(TestRandom),
        ));
        assert_eq!(InstalledClock.now().expect("clock").as_unix_ms(), 1_000);
        let mut bytes = [0_u8; 4];
        InstalledRandom.fill_bytes(&mut bytes).expect("random");
        assert_eq!(bytes, [7; 4]);
    }
}
