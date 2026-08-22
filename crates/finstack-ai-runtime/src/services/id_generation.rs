//! Injected clock/random interfaces and `UUIDv7` allocation.
//!
//! The kernel never reads ambient clocks or OS randomness. Runtime code supplies
//! timestamps and entropy through these ports; tests inject deterministic fakes.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use finstack_ai_kernel::{Id, IdTag, TimeError, Timestamp};
use thiserror::Error;
use uuid::Builder;

/// Provides semantic wall-clock timestamps.
pub trait Clock {
    /// Current semantic timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`IdGenerationError`] when the clock cannot produce a valid
    /// [`Timestamp`].
    fn now(&self) -> Result<Timestamp, IdGenerationError>;
}

impl<T: Clock + ?Sized> Clock for &T {
    fn now(&self) -> Result<Timestamp, IdGenerationError> {
        (**self).now()
    }
}

/// Provides cryptographic-quality or test entropy for `UUIDv7` construction.
pub trait RandomSource {
    /// Fill `buf` with random bytes.
    ///
    /// # Errors
    ///
    /// Returns [`IdGenerationError`] when entropy is unavailable.
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError>;
}

impl<T: RandomSource + ?Sized> RandomSource for &T {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        (**self).fill_bytes(buf)
    }
}

/// Failure allocating a `UUIDv7` identifier.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum IdGenerationError {
    /// Clock/random source failed.
    #[error("id generation source failure: {0}")]
    Source(String),
    /// Produced timestamp was outside the canonical range.
    #[error(transparent)]
    Time(#[from] TimeError),
}

/// `UUIDv7` generator over explicit clock and random ports.
#[derive(Debug, Clone, Copy)]
pub struct UuidV7Generator<C, R> {
    clock: C,
    random: R,
}

impl<C, R> UuidV7Generator<C, R> {
    /// Create a generator.
    #[must_use]
    pub const fn new(clock: C, random: R) -> Self {
        Self { clock, random }
    }
}

impl<C: Clock, R: RandomSource> UuidV7Generator<C, R> {
    /// Allocate a typed `UUIDv7` identifier.
    ///
    /// # Errors
    ///
    /// Returns [`IdGenerationError`] when the clock/random source fails or the
    /// timestamp is out of range.
    pub fn generate<T: IdTag>(&self) -> Result<Id<T>, IdGenerationError> {
        let now = self.clock.now()?;
        let millis = u64::try_from(now.as_unix_ms()).map_err(|_| {
            IdGenerationError::Source(
                "timestamp before Unix epoch is not supported for UUIDv7".into(),
            )
        })?;
        let mut counter_random = [0_u8; 10];
        self.random.fill_bytes(&mut counter_random)?;
        let uuid = Builder::from_unix_timestamp_millis(millis, &counter_random).into_uuid();
        Ok(Id::from_bytes(*uuid.as_bytes()))
    }
}

/// Injectable wall clock for tests and durable workflow hosts.
///
/// This is not a seventh port and not a timer. The kernel never reads it; the
/// runtime supplies [`Clock::now`] when building a [`finstack_ai_kernel::TransitionEnv`].
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::Timestamp;
/// use finstack_ai_runtime::ids::{Clock, ExternalClock};
///
/// let clock = ExternalClock::new(Timestamp::from_unix_ms(1_000).expect("ts"));
/// clock.jump(250).expect("forward");
/// assert_eq!(clock.now().expect("now").as_unix_ms(), 1_250);
/// clock.set(Timestamp::from_unix_ms(800).expect("ts"));
/// assert_eq!(clock.now().expect("now").as_unix_ms(), 800);
/// ```
#[derive(Debug, Clone)]
pub struct ExternalClock {
    now_ms: Arc<AtomicI64>,
}

impl ExternalClock {
    /// Create a clock fixed at `now`.
    #[must_use]
    pub fn new(now: Timestamp) -> Self {
        Self {
            now_ms: Arc::new(AtomicI64::new(now.as_unix_ms())),
        }
    }

    /// Replace the current wall time.
    pub fn set(&self, now: Timestamp) {
        self.now_ms.store(now.as_unix_ms(), Ordering::Release);
    }

    /// Shift the current wall time by `delta_ms` milliseconds.
    ///
    /// Negative values move backward. The resulting timestamp must stay in the
    /// canonical range.
    ///
    /// # Errors
    ///
    /// Returns [`IdGenerationError::Time`] when the shifted value is out of range.
    pub fn jump(&self, delta_ms: i64) -> Result<(), IdGenerationError> {
        let next = self
            .now_ms
            .load(Ordering::Acquire)
            .checked_add(delta_ms)
            .ok_or(IdGenerationError::Time(TimeError::Overflow))?;
        let timestamp = Timestamp::from_unix_ms(next)?;
        self.now_ms.store(timestamp.as_unix_ms(), Ordering::Release);
        Ok(())
    }
}

impl Clock for ExternalClock {
    fn now(&self) -> Result<Timestamp, IdGenerationError> {
        Ok(Timestamp::from_unix_ms(
            self.now_ms.load(Ordering::Acquire),
        )?)
    }
}

/// System clock backed by `std::time::SystemTime`.
///
/// Available only with the `native-tokio` feature so browser WASM graphs do not
/// pull ambient OS time through this adapter.
#[cfg(feature = "native-tokio")]
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

#[cfg(feature = "native-tokio")]
impl Clock for SystemClock {
    fn now(&self) -> Result<Timestamp, IdGenerationError> {
        use std::time::{SystemTime, UNIX_EPOCH};
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| IdGenerationError::Source(error.to_string()))?;
        let ms = i64::try_from(duration.as_millis())
            .map_err(|_| IdGenerationError::Source("system time overflow".into()))?;
        Ok(Timestamp::from_unix_ms(ms)?)
    }
}

/// Bytes drawn from the OS per refill.
///
/// A decision allocates several identifiers and each needs only 10 bytes, so
/// an unbuffered source makes one syscall-shaped call per identifier. Batching
/// amortizes that. The pool holds raw OS entropy — it is not a userspace
/// PRNG, so the entropy quality is exactly `getrandom`'s.
#[cfg(feature = "native-tokio")]
const ENTROPY_POOL_BYTES: usize = 1024;

#[cfg(feature = "native-tokio")]
thread_local! {
    /// Per-thread entropy pool and cursor into the unread remainder.
    ///
    /// The cursor starts exhausted so the first request refills from the OS.
    static ENTROPY_POOL: core::cell::RefCell<([u8; ENTROPY_POOL_BYTES], usize)> =
        const { core::cell::RefCell::new(([0; ENTROPY_POOL_BYTES], ENTROPY_POOL_BYTES)) };
}

/// OS entropy source via `getrandom`, buffered per thread.
#[cfg(feature = "native-tokio")]
#[derive(Debug, Default, Clone, Copy)]
pub struct OsRandomSource;

#[cfg(feature = "native-tokio")]
impl RandomSource for OsRandomSource {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        // Requests at or above the pool size go straight to the OS rather than
        // cycling the pool repeatedly.
        if buf.len() >= ENTROPY_POOL_BYTES {
            return getrandom::fill(buf)
                .map_err(|error| IdGenerationError::Source(error.to_string()));
        }
        ENTROPY_POOL.with(|cell| {
            let mut pool = cell.borrow_mut();
            let (bytes, cursor) = &mut *pool;
            if *cursor + buf.len() > ENTROPY_POOL_BYTES {
                getrandom::fill(bytes)
                    .map_err(|error| IdGenerationError::Source(error.to_string()))?;
                *cursor = 0;
            }
            let end = *cursor + buf.len();
            buf.copy_from_slice(&bytes[*cursor..end]);
            // Consumed entropy is zeroed so it can never be handed out twice.
            bytes[*cursor..end].fill(0);
            *cursor = end;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::RunId;

    #[derive(Debug)]
    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Result<Timestamp, IdGenerationError> {
            Ok(self.0)
        }
    }

    #[derive(Debug)]
    struct FixedRandom([u8; 10]);

    impl RandomSource for FixedRandom {
        fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
            buf.copy_from_slice(&self.0[..buf.len().min(10)]);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingRandom;

    impl RandomSource for FailingRandom {
        fn fill_bytes(&self, _buf: &mut [u8]) -> Result<(), IdGenerationError> {
            Err(IdGenerationError::Source("entropy unavailable".into()))
        }
    }

    #[derive(Debug)]
    struct FailingClock(IdGenerationError);

    impl Clock for FailingClock {
        fn now(&self) -> Result<Timestamp, IdGenerationError> {
            Err(self.0.clone())
        }
    }

    #[test]
    fn deterministic_uuidv7_from_injected_sources() {
        let clock = FixedClock(Timestamp::from_unix_ms(1_704_067_200_000).expect("ts"));
        let random = FixedRandom([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let generator = UuidV7Generator::new(clock, random);
        let first: RunId = generator.generate().expect("id");
        let second: RunId = generator.generate().expect("id");
        assert_eq!(first, second);
        let expected_uuid = Builder::from_unix_timestamp_millis(
            1_704_067_200_000,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        )
        .into_uuid();
        assert_eq!(expected_uuid.get_version_num(), 7);
        assert_eq!(first.as_bytes(), expected_uuid.as_bytes());
        assert_eq!(
            first.to_canonical_string(),
            expected_uuid.as_hyphenated().to_string()
        );
    }

    #[test]
    fn uuidv7_propagates_random_source_failure() {
        let clock = FixedClock(Timestamp::from_unix_ms(1_704_067_200_000).expect("ts"));
        let generator = UuidV7Generator::new(clock, FailingRandom);
        let err = generator
            .generate::<finstack_ai_kernel::RunTag>()
            .expect_err("random");
        assert!(matches!(err, IdGenerationError::Source(_)));
    }

    #[test]
    fn uuidv7_rejects_pre_epoch_and_out_of_range_timestamps() {
        let pre_epoch = FixedClock(Timestamp::from_unix_ms(-1).expect("pre-epoch in range"));
        let generator = UuidV7Generator::new(pre_epoch, FixedRandom([0; 10]));
        let err = generator
            .generate::<finstack_ai_kernel::RunTag>()
            .expect_err("pre-epoch");
        assert!(matches!(err, IdGenerationError::Source(_)));

        let clock = FailingClock(IdGenerationError::Time(TimeError::OutOfRange {
            value: i64::MAX,
        }));
        let generator = UuidV7Generator::new(clock, FixedRandom([0; 10]));
        let err = generator
            .generate::<finstack_ai_kernel::RunTag>()
            .expect_err("range");
        assert!(matches!(err, IdGenerationError::Time(_)));
    }

    /// The buffered pool must never hand the same bytes out twice, including
    /// across the refill boundary.
    #[cfg(feature = "native-tokio")]
    #[test]
    fn buffered_os_entropy_never_repeats_across_refills() {
        use std::collections::BTreeSet;

        let source = OsRandomSource;
        // More than one pool's worth of 10-byte draws forces several refills.
        let draws = (ENTROPY_POOL_BYTES / 10) * 3;
        let mut seen = BTreeSet::new();
        for _ in 0..draws {
            let mut buf = [0_u8; 10];
            source.fill_bytes(&mut buf).expect("entropy");
            assert!(seen.insert(buf), "entropy pool repeated a draw");
        }
        assert_eq!(seen.len(), draws);
    }

    /// Requests at or beyond the pool size bypass the pool entirely.
    #[cfg(feature = "native-tokio")]
    #[test]
    fn oversized_requests_bypass_the_pool() {
        let source = OsRandomSource;
        let mut large = vec![0_u8; ENTROPY_POOL_BYTES * 2];
        source.fill_bytes(&mut large).expect("entropy");
        assert!(large.iter().any(|byte| *byte != 0), "large draw was filled");

        let mut small = [0_u8; 10];
        source.fill_bytes(&mut small).expect("entropy");
        assert!(small.iter().any(|byte| *byte != 0), "pool still usable");
    }

    #[test]
    fn external_clock_jumps_forward_and_backward() {
        let clock = ExternalClock::new(Timestamp::from_unix_ms(1_000).expect("ts"));
        clock.jump(250).expect("forward");
        assert_eq!(clock.now().expect("now").as_unix_ms(), 1_250);
        clock.set(Timestamp::from_unix_ms(800).expect("ts"));
        assert_eq!(clock.now().expect("now").as_unix_ms(), 800);
        clock.jump(-100).expect("backward");
        assert_eq!(clock.now().expect("now").as_unix_ms(), 700);
    }
}
