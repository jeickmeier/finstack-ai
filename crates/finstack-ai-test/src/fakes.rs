//! Deterministic clock, random, and typed-ID sources for test scenarios.

use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};

use finstack_ai_kernel::{Duration, Id, IdTag, Timestamp};
use finstack_ai_runtime::{Clock, IdGenerationError, RandomSource, UuidV7Generator};

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
    /// Returns a source error for a poisoned clock or [`TimeError`] on overflow.
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

/// Deterministic non-repeating random-byte source backed by a shared counter.
#[derive(Debug, Clone)]
pub struct SequenceRandomSource {
    next: Arc<AtomicU64>,
}

impl SequenceRandomSource {
    /// Create a sequence source whose first block is derived from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            next: Arc::new(AtomicU64::new(seed)),
        }
    }
}

impl RandomSource for SequenceRandomSource {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        for chunk in buf.chunks_mut(8) {
            let value = self.next.fetch_add(1, Ordering::AcqRel).to_be_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
        Ok(())
    }
}

/// Deterministic typed `UUIDv7` source with a manually controlled clock.
#[derive(Debug, Clone)]
pub struct DeterministicIdSource {
    clock: ManualClock,
    generator: UuidV7Generator<ManualClock, SequenceRandomSource>,
}

impl DeterministicIdSource {
    /// Create a typed-ID source at `now` using `seed` for deterministic entropy.
    #[must_use]
    pub fn new(now: Timestamp, seed: u64) -> Self {
        let clock = ManualClock::new(now);
        let random = SequenceRandomSource::new(seed);
        let generator = UuidV7Generator::new(clock.clone(), random);
        Self { clock, generator }
    }

    /// Shared manual clock used by this source.
    #[must_use]
    pub fn clock(&self) -> ManualClock {
        self.clock.clone()
    }

    /// Allocate one deterministic typed `UUIDv7` value.
    ///
    /// # Errors
    ///
    /// Propagates explicit clock or entropy-source failures.
    pub fn generate<T: IdTag>(&self) -> Result<Id<T>, IdGenerationError> {
        self.generator.generate()
    }
}

/// Explicitly released asynchronous gate for deterministic slow-component tests.
#[derive(Debug, Clone, Default)]
pub struct ManualGate {
    state: Arc<ManualGateState>,
}

#[derive(Debug, Default)]
struct ManualGateState {
    released: AtomicBool,
    entries: AtomicUsize,
    waiters: Mutex<Vec<Waker>>,
}

impl ManualGate {
    /// Release all current and future waiters.
    pub fn release(&self) {
        self.state.released.store(true, Ordering::Release);
        if let Ok(mut waiters) = self.state.waiters.lock() {
            for waiter in waiters.drain(..) {
                waiter.wake();
            }
        }
    }

    /// Number of wait calls that reached this gate.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.state.entries.load(Ordering::Acquire)
    }

    /// Wait until the gate is explicitly released.
    pub async fn wait(&self) {
        self.state.entries.fetch_add(1, Ordering::AcqRel);
        poll_fn(|context| {
            if self.state.released.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            if let Ok(mut waiters) = self.state.waiters.lock()
                && !waiters
                    .iter()
                    .any(|waiter| waiter.will_wake(context.waker()))
            {
                waiters.push(context.waker().clone());
            }
            if self.state.released.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}
