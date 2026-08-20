//! Adapter-owned bounded export queues. These are operational, not a seventh port.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::ObserverError;

/// TDD observer export backpressure. Constructor config only; not a trait method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserverBackpressure {
    /// Wait up to `timeout` for queue space, then drop the item and diagnose.
    BlockBounded {
        /// Maximum wait before the push is classified as overflow.
        timeout: Duration,
    },
    /// Drop the newest item and keep the exporter connected.
    DropProgress,
    /// Disconnect the exporter after the first overflow.
    Disconnect,
}

/// Result of one bounded push that did not disconnect the exporter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserverQueuePush {
    /// The item was retained.
    Accepted,
    /// The item was dropped after overflow policy.
    Dropped,
}

/// Finite in-process export queue. Spill-to-disk is not a v1 mode.
#[derive(Debug)]
pub struct ObserverQueue<T> {
    inner: Mutex<VecDeque<T>>,
    capacity: usize,
    policy: ObserverBackpressure,
    dropped: AtomicU64,
    disconnected: AtomicBool,
}

impl<T> ObserverQueue<T> {
    /// Construct a finite queue.
    ///
    /// # Errors
    ///
    /// Returns `observer_configuration_invalid` for a zero or excessive bound.
    pub fn try_new(capacity: usize, policy: ObserverBackpressure) -> Result<Self, ObserverError> {
        if capacity == 0 || capacity > 1_000_000 {
            return Err(ObserverError::ConfigurationInvalid);
        }
        Ok(Self {
            inner: Mutex::new(VecDeque::new()),
            capacity,
            policy,
            dropped: AtomicU64::new(0),
            disconnected: AtomicBool::new(false),
        })
    }

    /// Push one item according to the configured overflow policy.
    ///
    /// # Errors
    ///
    /// Returns `observer_unavailable` when disconnected or the lock is poisoned,
    /// and `observer_capacity_exceeded` when [`ObserverBackpressure::Disconnect`]
    /// fires.
    pub fn push(&self, item: T) -> Result<ObserverQueuePush, ObserverError> {
        if self.disconnected.load(Ordering::Acquire) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return Err(ObserverError::Unavailable);
        }
        let deadline = match self.policy {
            ObserverBackpressure::BlockBounded { timeout } => Some(Instant::now() + timeout),
            ObserverBackpressure::DropProgress | ObserverBackpressure::Disconnect => None,
        };
        let mut pending = Some(item);
        loop {
            {
                let mut inner = self.inner.lock().map_err(|_| {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    ObserverError::Unavailable
                })?;
                if inner.len() < self.capacity {
                    inner.push_back(pending.take().ok_or(ObserverError::Unavailable)?);
                    return Ok(ObserverQueuePush::Accepted);
                }
            }
            match self.policy {
                ObserverBackpressure::DropProgress => {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return Ok(ObserverQueuePush::Dropped);
                }
                ObserverBackpressure::Disconnect => {
                    self.disconnected.store(true, Ordering::Release);
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return Err(ObserverError::CapacityExceeded);
                }
                ObserverBackpressure::BlockBounded { .. } => {
                    if deadline.is_some_and(|limit| Instant::now() >= limit) {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                        return Ok(ObserverQueuePush::Dropped);
                    }
                    std::thread::yield_now();
                }
            }
        }
    }

    /// Drain retained items in insertion order.
    ///
    /// # Errors
    ///
    /// Returns `observer_unavailable` when the lock is poisoned.
    pub fn drain(&self) -> Result<Vec<T>, ObserverError> {
        self.inner
            .lock()
            .map(|mut inner| inner.drain(..).collect())
            .map_err(|_| ObserverError::Unavailable)
    }

    /// Current retained depth.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.inner.lock().map_or(0, |inner| inner.len())
    }

    /// Cumulative dropped-item count.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Whether [`ObserverBackpressure::Disconnect`] has fired.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }
}

/// Non-semantic overflow diagnostic. Never a `RunEvent` kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserverDiagnostic {
    /// Stable diagnostic code.
    pub code: &'static str,
    /// Non-secret operator text.
    pub detail: &'static str,
}

/// Overflow diagnostic emitted when an adapter queue drops or disconnects.
pub const OBSERVER_QUEUE_OVERFLOW: ObserverDiagnostic = ObserverDiagnostic {
    code: "observer_queue_overflow",
    detail: "bounded observer export queue overflowed",
};
