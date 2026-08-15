//! In-process host import doubles for the generated logging and blob interfaces.

use crate::generated::{BlobRef, CallContext, HostBlobs, HostLogging, Level, PluginError};
use crate::limits::{MAX_STRING_BYTES, reject_before_allocation, reject_declared_len};

/// Recording logger that stores only messages that pass the string ceiling.
#[derive(Debug, Default)]
pub struct RecordingLogger {
    messages: std::cell::RefCell<Vec<(Level, String)>>,
}

impl RecordingLogger {
    /// Borrow recorded messages.
    #[must_use]
    pub fn messages(&self) -> Vec<(Level, String)> {
        self.messages.borrow().clone()
    }
}

impl HostLogging for RecordingLogger {
    fn log(&self, _context: &CallContext, level: &Level, message: &str) -> Result<(), PluginError> {
        reject_before_allocation(message.as_bytes(), MAX_STRING_BYTES, "log.message").map_err(
            |error| PluginError {
                code: error.code().to_owned(),
                message: error.to_string(),
                retryable: false,
            },
        )?;
        self.messages
            .borrow_mut()
            .push((*level, message.to_owned()));
        Ok(())
    }
}

/// Blob reader that enforces the per-call byte ceiling before allocating.
#[derive(Debug)]
pub struct CeilingBlobStore {
    bytes: Vec<u8>,
}

impl CeilingBlobStore {
    /// Store one blob body for later authorized reads.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

impl HostBlobs for CeilingBlobStore {
    fn read(
        &self,
        _context: &CallContext,
        _reference: &BlobRef,
        offset: u64,
        max_bytes: u64,
    ) -> Result<Vec<u8>, PluginError> {
        reject_declared_len(max_bytes, MAX_STRING_BYTES, "blobs.read").map_err(|error| {
            PluginError {
                code: error.code().to_owned(),
                message: error.to_string(),
                retryable: false,
            }
        })?;
        let start = usize::try_from(offset).unwrap_or(self.bytes.len());
        let take = usize::try_from(max_bytes).unwrap_or(0);
        let end = start.saturating_add(take).min(self.bytes.len());
        if start >= self.bytes.len() {
            return Ok(Vec::new());
        }
        Ok(self.bytes[start..end].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::{CeilingBlobStore, RecordingLogger};
    use crate::generated::{BlobRef, CallContext, HostBlobs, HostLogging, Level};
    use crate::limits::MAX_STRING_BYTES;

    fn context() -> CallContext {
        CallContext {
            effect_id: "e".to_owned(),
            session_id: "s".to_owned(),
            lane_id: "l".to_owned(),
            run_id: "r".to_owned(),
            tenant_scope: "t".to_owned(),
            principal_issuer: "i".to_owned(),
            principal_subject: "p".to_owned(),
            authorization_decision_id: "d".to_owned(),
            permitted_scopes: Vec::new(),
            budget_scope_id: None,
            deadline_unix_ms: None,
        }
    }

    #[test]
    fn host_imports_compile_and_enforce_ceilings() {
        let logger = RecordingLogger::default();
        logger.log(&context(), &Level::Info, "ok").expect("log");
        assert_eq!(logger.messages().len(), 1);
        let store = CeilingBlobStore::new(b"abcdef".to_vec());
        let read = store
            .read(
                &context(),
                &BlobRef {
                    id: "blob".to_owned(),
                    media_type: "text/plain".to_owned(),
                    length: 6,
                    digest: None,
                },
                1,
                2,
            )
            .expect("read");
        assert_eq!(read, b"bc");
        let error = store
            .read(
                &context(),
                &BlobRef {
                    id: "blob".to_owned(),
                    media_type: "text/plain".to_owned(),
                    length: 6,
                    digest: None,
                },
                0,
                u64::try_from(MAX_STRING_BYTES).expect("fits") + 1,
            )
            .expect_err("ceiling");
        assert_eq!(error.code, "plugin_payload_too_large");
    }
}
