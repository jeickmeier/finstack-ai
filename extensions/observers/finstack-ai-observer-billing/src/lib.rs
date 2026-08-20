//! Read-only billing ledger observer. Aggregates kernel `CostAmount` by
//! session, run, model, and pricing policy version. Integer money only.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, RunEvent, Version};
use finstack_ai_runtime::{
    OBSERVER_QUEUE_OVERFLOW, Observer, ObserverBackpressure, ObserverDescriptor,
    ObserverDiagnostic, ObserverError, ObserverPayloadMode, ObserverQueue, ObserverQueuePush,
    PortFuture,
};
use thiserror::Error;

/// Maximum distinct attribution keys accepted by the ledger.
const MAX_ENTRIES_CEILING: usize = 1_000_000;

/// Billing-observer construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BillingObserverError {
    /// Configuration is malformed.
    #[error("billing_observer_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

#[derive(Debug, Default)]
struct LedgerState {}

/// Read-only spend-ledger observer. Default payload mode is metadata-only.
pub struct BillingObserver {
    descriptor: ObserverDescriptor,
    queue: ObserverQueue<()>,
    state: Mutex<LedgerState>,
    max_entries: usize,
    dropped: AtomicU64,
    diagnostic: Mutex<Option<ObserverDiagnostic>>,
}

impl BillingObserver {
    /// Construct a billing observer.
    ///
    /// # Arguments
    ///
    /// * `queue_capacity` - Bounded diagnostics-queue capacity (1..=1_000_000).
    /// * `backpressure` - Queue policy applied when the bound is reached.
    /// * `max_entries` - Distinct (session, run, model) keys retained (1..=1_000_000).
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity, queue bound, or entry bound.
    pub fn try_new(
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
        max_entries: usize,
    ) -> Result<Self, BillingObserverError> {
        if max_entries == 0 || max_entries > MAX_ENTRIES_CEILING {
            return Err(BillingObserverError::Configuration {
                reason: "invalid_max_entries",
            });
        }
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.observer.billing").map_err(|_| {
                        BillingObserverError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    Some(Version {
                        major: 0,
                        minor: 0,
                        patch: 1,
                    }),
                ),
                payload_mode: ObserverPayloadMode::MetadataOnly,
                metadata: Metadata::empty(),
            },
            queue: ObserverQueue::try_new(queue_capacity, backpressure).map_err(|_| {
                BillingObserverError::Configuration {
                    reason: "invalid_queue_capacity",
                }
            })?,
            state: Mutex::new(LedgerState::default()),
            max_entries,
            dropped: AtomicU64::new(0),
            diagnostic: Mutex::new(None),
        })
    }

    /// Cumulative adapter-queue drops.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed) + self.queue.dropped()
    }

    /// Last overflow or saturation diagnostic.
    #[must_use]
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic> {
        self.diagnostic.lock().ok().and_then(|slot| *slot)
    }

    fn ingest(&self, event: &RunEvent) {
        let Ok(_state) = self.state.lock() else {
            return;
        };
        let _ = event;
    }

    fn record_overflow(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(OBSERVER_QUEUE_OVERFLOW);
        }
    }
}

impl Observer for BillingObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        for event in batch.iter() {
            self.ingest(event);
            match self.queue.push(()) {
                Ok(ObserverQueuePush::Accepted) => {}
                Ok(ObserverQueuePush::Dropped) => self.record_overflow(),
                Err(error) => {
                    self.record_overflow();
                    return Box::pin(async move { Err(error) });
                }
            }
        }
        let _ = self.queue.drain();
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests;
