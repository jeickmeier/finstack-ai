//! Local owner parking. The SDK controller retains the accepted request while
//! parked; no new run or user message is admitted when its owner is rebuilt.

use finstack_ai_kernel::Digest;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver as driver;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::native_driver as driver;
use std::sync::atomic::{AtomicU8, Ordering};

const RUNNING: u8 = 0;
const PARKING: u8 = 1;
const PARKED: u8 = 2;
const RESUMING: u8 = 3;
const FINISHED: u8 = 4;

pub(super) struct ExecutionLifecycle {
    state: AtomicU8,
    changed: driver::Signal,
    pub(super) lock_digest: Option<Digest>,
}

impl ExecutionLifecycle {
    pub(super) fn new(lock_digest: Option<Digest>) -> Self {
        Self {
            state: AtomicU8::new(RUNNING),
            changed: driver::Signal::new(),
            lock_digest,
        }
    }

    pub(super) fn request_park(&self) -> bool {
        self.state
            .compare_exchange(RUNNING, PARKING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(super) fn parking(&self) -> bool {
        self.state.load(Ordering::Acquire) == PARKING
    }

    pub(super) async fn wait_parked(&self) {
        self.wait_while(PARKING).await;
    }

    pub(super) async fn park(&self) {
        self.state.store(PARKED, Ordering::Release);
        self.changed.notify_waiters();
        self.wait_while(PARKED).await;
    }

    pub(super) fn resume(&self) -> bool {
        let resumed = self
            .state
            .compare_exchange(PARKED, RESUMING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        self.changed.notify_waiters();
        resumed
    }

    pub(super) async fn wait_resumed(&self) {
        self.wait_while(RESUMING).await;
    }

    pub(super) fn started(&self) {
        let _ = self
            .state
            .compare_exchange(RESUMING, RUNNING, Ordering::AcqRel, Ordering::Acquire);
        self.changed.notify_waiters();
    }

    pub(super) fn finished(&self) {
        self.state.store(FINISHED, Ordering::Release);
        self.changed.notify_waiters();
    }

    async fn wait_while(&self, state: u8) {
        loop {
            let notified = self.changed.notified();
            if self.state.load(Ordering::Acquire) != state {
                return;
            }
            notified.await;
        }
    }
}
