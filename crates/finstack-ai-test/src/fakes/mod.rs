//! Deterministic clock and random sources plus the shared release gate.

use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use finstack_ai_kernel::{Duration, Timestamp};
use finstack_ai_runtime::ids::{Clock, IdGenerationError, RandomSource};

/// Clock that always returns a fixed semantic timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedClock {
    now: Timestamp,
}

impl FixedClock {
    /// Create a fixed clock.
    #[must_use]
    pub const fn new(now: Timestamp) -> Self {
        Self { now }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Result<Timestamp, IdGenerationError> {
        Ok(self.now)
    }
}

/// Shared manually advanced semantic clock.
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<Mutex<Timestamp>>,
}

impl ManualClock {
    /// Create a manual clock at `now`.
    #[must_use]
    pub fn new(now: Timestamp) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    /// Replace the current semantic timestamp.
    ///
    /// # Errors
    ///
    /// Returns a source error when the shared clock lock is unavailable.
    pub fn set(&self, now: Timestamp) -> Result<(), IdGenerationError> {
        *self
            .now
            .lock()
            .map_err(|_| IdGenerationError::Source("manual clock is unavailable".into()))? = now;
        Ok(())
    }

    /// Advance the current semantic timestamp by a non-negative duration.
    ///
    /// # Errors
    ///
    /// Returns a source error for a poisoned clock or time-range overflow.
    pub fn advance(&self, duration: Duration) -> Result<Timestamp, IdGenerationError> {
        let mut now = self
            .now
            .lock()
            .map_err(|_| IdGenerationError::Source("manual clock is unavailable".into()))?;
        *now = now.checked_add(duration).map_err(IdGenerationError::from)?;
        Ok(*now)
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Result<Timestamp, IdGenerationError> {
        self.now
            .lock()
            .map(|value| *value)
            .map_err(|_| IdGenerationError::Source("manual clock is unavailable".into()))
    }
}

/// Random source that repeats a fixed byte pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternRandomSource {
    pattern: Vec<u8>,
}

impl PatternRandomSource {
    /// Create a pattern source. Empty patterns fail on fill.
    #[must_use]
    pub fn new(pattern: impl Into<Vec<u8>>) -> Self {
        Self {
            pattern: pattern.into(),
        }
    }
}

impl RandomSource for PatternRandomSource {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        if self.pattern.is_empty() {
            return Err(IdGenerationError::Source(
                "pattern random source has no entropy".into(),
            ));
        }
        for (idx, slot) in buf.iter_mut().enumerate() {
            *slot = self.pattern[idx % self.pattern.len()];
        }
        Ok(())
    }
}

/// Poll-based release gate shared by [`ManualGate`] and the scripted streams.
#[derive(Debug, Default)]
pub(crate) struct Gate {
    released: AtomicBool,
    entries: AtomicUsize,
    waiters: Mutex<Vec<Waker>>,
}

impl Gate {
    /// Release all current and future waiters.
    pub(crate) fn release(&self) {
        self.released.store(true, Ordering::Release);
        if let Ok(mut waiters) = self.waiters.lock() {
            for waiter in waiters.drain(..) {
                waiter.wake();
            }
        }
    }

    /// Count one arrival at the gate.
    pub(crate) fn enter(&self) {
        self.entries.fetch_add(1, Ordering::AcqRel);
    }

    /// Number of arrivals counted so far.
    pub(crate) fn entries(&self) -> usize {
        self.entries.load(Ordering::Acquire)
    }

    /// Ready once released; otherwise registers `cx` and stays pending.
    pub(crate) fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
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

/// Explicitly released asynchronous gate for deterministic slow-component tests.
#[derive(Debug, Clone, Default)]
pub struct ManualGate {
    state: Arc<Gate>,
}

impl ManualGate {
    /// Release all current and future waiters.
    pub fn release(&self) {
        self.state.release();
    }

    /// Number of wait calls that reached this gate.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.state.entries()
    }

    /// Wait until the gate is explicitly released.
    pub async fn wait(&self) {
        self.state.enter();
        poll_fn(|cx| self.state.poll(cx)).await;
    }
}
