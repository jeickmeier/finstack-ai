//! Bounded in-memory append log using the same replay validator as `SQLite`.
use super::state::{MAX_EVENT_BYTES, MAX_LOG_BYTES, State};
use super::{EvalStore, Mutation, RunnerLease, StoreSnapshot, mutation_methods};
use crate::error::{invalid, unavailable};
use crate::{EVAL_RUNNER_BUSY, EvalError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Inner {
    state: State,
    log: Vec<Vec<u8>>,
    bytes: u64,
}

/// Append-only ephemeral experiment, bounded to 256 MiB of serialized mutations.
#[derive(Default)]
pub struct MemoryEvalStore {
    inner: Mutex<Inner>,
    running: Arc<AtomicBool>,
}
impl MemoryEvalStore {
    /// Construct an empty store. Retain the handle to resume within this process.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn append(&self, mutation: Mutation) -> Result<(), EvalError> {
        let mut inner = self.inner.lock().map_err(|_| unavailable())?;
        if !inner.state.check(&mutation)? {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&mutation).map_err(|_| invalid())?;
        let total = inner
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(invalid)?;
        if bytes.len() > MAX_EVENT_BYTES || total > MAX_LOG_BYTES {
            return Err(invalid());
        }
        inner.state.apply(mutation)?;
        inner.bytes = total;
        inner.log.push(bytes);
        Ok(())
    }
}
struct MemoryLease(Arc<AtomicBool>);
impl RunnerLease for MemoryLease {}
impl Drop for MemoryLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl EvalStore for MemoryEvalStore {
    fn acquire_runner(&self) -> Result<Box<dyn RunnerLease>, EvalError> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| EvalError::new(EVAL_RUNNER_BUSY, "experiment runner already active"))?;
        Ok(Box::new(MemoryLease(Arc::clone(&self.running))))
    }
    mutation_methods!();
    fn snapshot(&self) -> Result<StoreSnapshot, EvalError> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| unavailable())?
            .state
            .snapshot
            .clone())
    }
}
