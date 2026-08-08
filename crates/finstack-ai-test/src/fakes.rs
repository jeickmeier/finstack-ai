//! Deterministic clock and random sources for injectable `UUIDv7` tests.

use finstack_ai_kernel::Timestamp;
use finstack_ai_runtime::{Clock, IdGenerationError, RandomSource};

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
