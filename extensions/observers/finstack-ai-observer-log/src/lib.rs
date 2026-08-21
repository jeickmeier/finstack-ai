//! Structured JSON log observer. Default payload mode is redacted.

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

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, RunEvent, Version};
use finstack_ai_runtime::{
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode, PortFuture,
    journal_export_jsonl, observer_events_jsonl, support_bundle_versions,
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

/// Structured JSON observer over a caller-owned writer.
pub struct LogObserver {
    descriptor: ObserverDescriptor,
    writer: Arc<Mutex<dyn Write + Send>>,
}

impl LogObserver {
    /// Construct a redacted JSON observer over a caller-supplied writer.
    ///
    /// # Errors
    ///
    /// Rejects an invalid observer identity.
    pub fn try_redacted(writer: Arc<Mutex<dyn Write + Send>>) -> Result<Self, LogObserverError> {
        Self::try_new(ObserverPayloadMode::Redacted, writer)
    }

    /// Construct a JSON observer with an explicit payload mode.
    ///
    /// # Errors
    ///
    /// Rejects an invalid observer identity.
    pub fn try_new(
        payload_mode: ObserverPayloadMode,
        writer: Arc<Mutex<dyn Write + Send>>,
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
        })
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
        let writer = Arc::clone(&self.writer);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let Ok(mut sink) = writer.lock() else {
                    return Err(ObserverError::Unavailable);
                };
                sink.write_all(jsonl.as_bytes())
                    .map_err(|_| ObserverError::Unavailable)?;
                Ok(())
            })
            .await
            .map_err(|_| ObserverError::Unavailable)?
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

#[cfg(test)]
mod tests;
