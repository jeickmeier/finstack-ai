//! Native monotonic waits derived from persisted wall-clock deadlines.

use std::time::Duration as StdDuration;

use finstack_ai_kernel::{Duration, Timestamp};
use thiserror::Error;
use tokio::time::{Instant, sleep_until};

use crate::{Clock, IdGenerationError};

/// Diagnostic classification produced while converting a durable deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineDiagnostic {
    /// Wall time was within the persisted scheduling interval.
    None,
    /// The deadline was already due when this process installed the wait.
    AlreadyDue,
    /// Wall time moved before the persisted scheduling time; remaining time was clamped.
    BackwardClockClamped,
}

/// One process-local monotonic wait derived from a persisted wall deadline.
///
/// The monotonic instant is deliberately private and is never serializable.
/// Once constructed, later wall-clock changes cannot extend the active wait.
#[derive(Debug, Clone)]
pub struct MonotonicDeadline {
    wall_deadline: Timestamp,
    original_maximum: Duration,
    remaining: Duration,
    diagnostic: DeadlineDiagnostic,
    due: Instant,
}

impl MonotonicDeadline {
    /// Convert one persisted scheduling interval to a process-local wait.
    ///
    /// `scheduled_at` is the semantic timestamp on the durable record that
    /// created `wall_deadline`. It provides the persisted maximum duration used
    /// to clamp backward wall-clock movement after restart.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid persisted interval, clock failure, or a
    /// monotonic-instant overflow.
    pub fn from_persisted(
        clock: &impl Clock,
        scheduled_at: Timestamp,
        wall_deadline: Timestamp,
    ) -> Result<Self, RuntimeTimeError> {
        let maximum_ms = wall_deadline
            .as_unix_ms()
            .checked_sub(scheduled_at.as_unix_ms())
            .ok_or(RuntimeTimeError::InvalidPersistedDeadline)?;
        let maximum_ms =
            u64::try_from(maximum_ms).map_err(|_| RuntimeTimeError::InvalidPersistedDeadline)?;
        let original_maximum = Duration::from_millis(maximum_ms);
        let wall_now = clock.now().map_err(RuntimeTimeError::Clock)?;
        let raw_remaining = wall_deadline
            .as_unix_ms()
            .checked_sub(wall_now.as_unix_ms())
            .ok_or(RuntimeTimeError::InvalidPersistedDeadline)?;
        let (remaining_ms, diagnostic) = if raw_remaining <= 0 {
            (0, DeadlineDiagnostic::AlreadyDue)
        } else {
            let raw_remaining = u64::try_from(raw_remaining)
                .map_err(|_| RuntimeTimeError::InvalidPersistedDeadline)?;
            if raw_remaining > maximum_ms {
                (maximum_ms, DeadlineDiagnostic::BackwardClockClamped)
            } else {
                (raw_remaining, DeadlineDiagnostic::None)
            }
        };
        let remaining = Duration::from_millis(remaining_ms);
        let due = Instant::now()
            .checked_add(StdDuration::from_millis(remaining_ms))
            .ok_or(RuntimeTimeError::MonotonicOverflow)?;
        Ok(Self {
            wall_deadline,
            original_maximum,
            remaining,
            diagnostic,
            due,
        })
    }

    /// Persisted wall-clock deadline.
    #[must_use]
    pub const fn wall_deadline(&self) -> Timestamp {
        self.wall_deadline
    }

    /// Persisted maximum duration used for restart clamping.
    #[must_use]
    pub const fn original_maximum(&self) -> Duration {
        self.original_maximum
    }

    /// Remaining monotonic duration fixed when the wait was installed.
    #[must_use]
    pub const fn remaining(&self) -> Duration {
        self.remaining
    }

    /// Conversion diagnostic.
    #[must_use]
    pub const fn diagnostic(&self) -> DeadlineDiagnostic {
        self.diagnostic
    }

    /// Wait without polling until the fixed monotonic instant.
    pub async fn wait(&self) {
        sleep_until(self.due).await;
    }
}

/// Runtime-supplied jitter for a semantic retry directive.
///
/// The kernel receives only the resulting bounded backoff and never reads a
/// random source itself.
pub trait RetryJitterSource {
    /// Produce a non-negative jitter no greater than `maximum`.
    ///
    /// # Errors
    ///
    /// Returns an error when the source cannot produce a bounded value.
    fn jitter(&self, attempt: u32, maximum: Duration) -> Result<Duration, RuntimeTimeError>;
}

/// Deterministic source that adds no jitter.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoRetryJitter;

impl RetryJitterSource for NoRetryJitter {
    fn jitter(&self, _attempt: u32, _maximum: Duration) -> Result<Duration, RuntimeTimeError> {
        Ok(Duration::ZERO)
    }
}

/// Add runtime-supplied bounded jitter to a semantic retry backoff.
///
/// # Errors
///
/// Returns an error if the source exceeds its bound or addition overflows.
pub fn retry_backoff_with_jitter(
    base: Duration,
    maximum_jitter: Duration,
    attempt: u32,
    source: &impl RetryJitterSource,
) -> Result<Duration, RuntimeTimeError> {
    let jitter = source.jitter(attempt, maximum_jitter)?;
    if jitter > maximum_jitter {
        return Err(RuntimeTimeError::JitterOutOfRange);
    }
    base.checked_add(jitter)
        .map_err(|_| RuntimeTimeError::DurationOverflow)
}

/// Deadline/timer conversion failure.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RuntimeTimeError {
    /// The runtime wall clock failed.
    #[error("runtime clock failed: {0}")]
    Clock(IdGenerationError),
    /// Persisted scheduling time was later than its deadline.
    #[error("persisted deadline interval is invalid")]
    InvalidPersistedDeadline,
    /// Process-local monotonic instant overflowed.
    #[error("monotonic deadline overflow")]
    MonotonicOverflow,
    /// Jitter source exceeded the configured upper bound.
    #[error("retry jitter exceeded its configured bound")]
    JitterOutOfRange,
    /// Base backoff plus jitter overflowed.
    #[error("retry backoff overflow")]
    DurationOverflow,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    struct MutableClock(AtomicI64);

    impl Clock for MutableClock {
        fn now(&self) -> Result<Timestamp, IdGenerationError> {
            Timestamp::from_unix_ms(self.0.load(Ordering::Acquire)).map_err(IdGenerationError::Time)
        }
    }

    struct MaximumJitter;

    impl RetryJitterSource for MaximumJitter {
        fn jitter(&self, _attempt: u32, maximum: Duration) -> Result<Duration, RuntimeTimeError> {
            Ok(maximum)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn active_deadline_is_monotonic_and_backward_restart_is_clamped() {
        let clock = MutableClock(AtomicI64::new(1_100));
        let wait = MonotonicDeadline::from_persisted(
            &clock,
            Timestamp::from_unix_ms(1_000).expect("scheduled"),
            Timestamp::from_unix_ms(1_500).expect("deadline"),
        )
        .expect("wait");
        assert_eq!(wait.remaining(), Duration::from_millis(400));
        clock.0.store(100, Ordering::Release);
        tokio::time::advance(StdDuration::from_millis(399)).await;
        assert!(
            tokio::time::timeout(StdDuration::ZERO, wait.wait())
                .await
                .is_err()
        );
        tokio::time::advance(StdDuration::from_millis(1)).await;
        wait.wait().await;

        let restarted = MonotonicDeadline::from_persisted(
            &clock,
            Timestamp::from_unix_ms(1_000).expect("scheduled"),
            Timestamp::from_unix_ms(1_500).expect("deadline"),
        )
        .expect("restart");
        assert_eq!(restarted.remaining(), Duration::from_millis(500));
        assert_eq!(
            restarted.diagnostic(),
            DeadlineDiagnostic::BackwardClockClamped
        );
    }

    #[test]
    fn overdue_deadlines_and_external_jitter_are_bounded() {
        let clock = MutableClock(AtomicI64::new(2_000));
        let wait = MonotonicDeadline::from_persisted(
            &clock,
            Timestamp::from_unix_ms(1_000).expect("scheduled"),
            Timestamp::from_unix_ms(1_500).expect("deadline"),
        )
        .expect("wait");
        assert_eq!(wait.remaining(), Duration::ZERO);
        assert_eq!(wait.diagnostic(), DeadlineDiagnostic::AlreadyDue);
        assert_eq!(
            retry_backoff_with_jitter(
                Duration::from_millis(100),
                Duration::from_millis(20),
                1,
                &MaximumJitter,
            )
            .expect("backoff"),
            Duration::from_millis(120)
        );
    }
}
