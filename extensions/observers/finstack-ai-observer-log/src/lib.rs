//! Structured JSON log observer. Default payload mode is redacted.

#![warn(missing_docs)]

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_runtime::{
    ComponentId, ComponentRef, Metadata, OBSERVER_QUEUE_OVERFLOW, Observer, ObserverBackpressure,
    ObserverDescriptor, ObserverDiagnostic, ObserverError, ObserverPayloadMode, ObserverQueue,
    ObserverQueuePush, PortFuture, RunEvent, Version, journal_export_jsonl, observer_events_jsonl,
    support_bundle_versions,
};
use serde::Serialize;
use thiserror::Error;

/// Log-observer construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LogObserverError {
    /// Configuration is malformed.
    #[error("log_observer_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
    /// Support-bundle I/O failed.
    #[error("log_observer_bundle_unavailable")]
    BundleUnavailable,
}

/// Structured JSON observer with a bounded export queue.
pub struct LogObserver {
    descriptor: ObserverDescriptor,
    writer: Arc<Mutex<dyn Write + Send>>,
    queue: ObserverQueue<String>,
    diagnostic: Mutex<Option<ObserverDiagnostic>>,
}

impl LogObserver {
    /// Construct a redacted JSON observer over a caller-supplied writer.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity or a zero/excessive queue bound.
    pub fn try_redacted(
        writer: Arc<Mutex<dyn Write + Send>>,
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
    ) -> Result<Self, LogObserverError> {
        Self::try_new(
            ObserverPayloadMode::Redacted,
            writer,
            queue_capacity,
            backpressure,
        )
    }

    /// Construct a JSON observer with an explicit payload mode.
    ///
    /// # Errors
    ///
    /// Rejects an invalid identity or a zero/excessive queue bound.
    pub fn try_new(
        payload_mode: ObserverPayloadMode,
        writer: Arc<Mutex<dyn Write + Send>>,
        queue_capacity: usize,
        backpressure: ObserverBackpressure,
    ) -> Result<Self, LogObserverError> {
        Ok(Self {
            descriptor: ObserverDescriptor {
                component: ComponentRef::new(
                    ComponentId::parse("finstack.observer.log").map_err(|_| {
                        LogObserverError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    Some(Version {
                        major: 0,
                        minor: 0,
                        patch: 4,
                    }),
                ),
                payload_mode,
                metadata: Metadata::empty(),
            },
            writer,
            queue: ObserverQueue::try_new(queue_capacity, backpressure).map_err(|_| {
                LogObserverError::Configuration {
                    reason: "invalid_queue_capacity",
                }
            })?,
            diagnostic: Mutex::new(None),
        })
    }

    /// Cumulative dropped export lines.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.queue.dropped()
    }

    /// Last overflow or disconnect diagnostic.
    #[must_use]
    pub fn last_diagnostic(&self) -> Option<ObserverDiagnostic> {
        self.diagnostic.lock().ok().and_then(|slot| *slot)
    }

    fn record_overflow(&self) {
        if let Ok(mut slot) = self.diagnostic.lock() {
            *slot = Some(OBSERVER_QUEUE_OVERFLOW);
        }
    }
}

impl Observer for LogObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let jsonl = match observer_events_jsonl(batch.as_ref(), self.descriptor.payload_mode) {
            Ok(value) => value,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        for line in jsonl.lines() {
            match self.queue.push(line.to_owned()) {
                Ok(ObserverQueuePush::Accepted) => {}
                Ok(ObserverQueuePush::Dropped) => self.record_overflow(),
                Err(error) => {
                    self.record_overflow();
                    return Box::pin(async move { Err(error) });
                }
            }
        }
        let writer = Arc::clone(&self.writer);
        let drained = match self.queue.drain() {
            Ok(value) => value,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        Box::pin(async move {
            let Ok(mut sink) = writer.lock() else {
                return Err(ObserverError::Unavailable);
            };
            for line in drained {
                sink.write_all(line.as_bytes())
                    .map_err(|_| ObserverError::Unavailable)?;
                sink.write_all(b"\n")
                    .map_err(|_| ObserverError::Unavailable)?;
            }
            Ok(())
        })
    }
}

/// Write the default support bundle: redacted events, metadata-only journal
/// export, versions, and an optional metrics snapshot. No raw journal CBOR.
///
/// # Errors
///
/// Returns [`LogObserverError::BundleUnavailable`] when the directory cannot be
/// created or a file cannot be written.
pub fn write_support_bundle(
    root: &Path,
    events: &[RunEvent],
    records: &[impl Serialize],
    metrics_snapshot: Option<&str>,
) -> Result<(), LogObserverError> {
    std::fs::create_dir_all(root).map_err(|_| LogObserverError::BundleUnavailable)?;
    let events_jsonl = observer_events_jsonl(events, ObserverPayloadMode::Redacted)
        .map_err(|_| LogObserverError::BundleUnavailable)?;
    let journal_jsonl = journal_export_jsonl(records, ObserverPayloadMode::MetadataOnly)
        .map_err(|_| LogObserverError::BundleUnavailable)?;
    let versions = support_bundle_versions(env!("CARGO_PKG_VERSION"), &["finstack.observer.log"]);
    std::fs::write(root.join("events.jsonl"), events_jsonl)
        .map_err(|_| LogObserverError::BundleUnavailable)?;
    std::fs::write(root.join("journal-export.jsonl"), journal_jsonl)
        .map_err(|_| LogObserverError::BundleUnavailable)?;
    std::fs::write(
        root.join("versions.json"),
        serde_json::to_vec_pretty(&versions).map_err(|_| LogObserverError::BundleUnavailable)?,
    )
    .map_err(|_| LogObserverError::BundleUnavailable)?;
    if let Some(metrics) = metrics_snapshot {
        std::fs::write(root.join("metrics.txt"), metrics)
            .map_err(|_| LogObserverError::BundleUnavailable)?;
    }
    Ok(())
}

/// Default bounded-block timeout used by examples.
#[must_use]
pub const fn default_block_timeout() -> Duration {
    Duration::from_millis(20)
}

#[cfg(test)]
mod tests;
