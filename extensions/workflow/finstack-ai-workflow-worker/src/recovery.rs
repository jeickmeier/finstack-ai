//! Immutable host reconstruction metadata, separate from journal authority.

use std::sync::Arc;

use finstack_ai_kernel::{OperationLocator, RunId};

use crate::WorkerError;

/// Maximum serialized host descriptor size.
pub const MAX_RECOVERY_DESCRIPTOR_BYTES: usize = 64 * 1024;
/// Maximum registrations returned by one bounded tenant scan.
pub const MAX_RECOVERY_SCAN: usize = 256;

/// Opaque, immutable host metadata bound to one exact run locator.
///
/// Hosts own the versioned payload schema. Credentials and values already
/// authoritative in committed history must not be copied into this payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRegistration {
    /// Exact tenant, session, lane and run binding.
    pub locator: OperationLocator,
    /// Versioned reconstruction inputs serialized by the host.
    pub descriptor: Arc<[u8]>,
}

impl RecoveryRegistration {
    pub(crate) fn validate(&self) -> Result<(), WorkerError> {
        if self.descriptor.is_empty() || self.descriptor.len() > MAX_RECOVERY_DESCRIPTOR_BYTES {
            return Err(WorkerError::InvalidConfiguration {
                code: "recovery_descriptor_size",
            });
        }
        Ok(())
    }
}

/// Adapter storage for immutable host metadata. This is not a primary port.
pub trait RecoveryStore: Send + Sync {
    /// Insert once, accepting an identical repeat and rejecting conflicting bytes.
    ///
    /// # Errors
    /// Returns a conflict, size-bound or storage error.
    fn insert_recovery(&self, registration: &RecoveryRegistration) -> Result<(), WorkerError>;

    /// Load the exact locator; a differing lane/session binding fails closed.
    ///
    /// # Errors
    /// Returns an integrity or storage error.
    fn load_recovery(
        &self,
        locator: &OperationLocator,
    ) -> Result<Option<RecoveryRegistration>, WorkerError>;

    /// Enumerate only the host-authorized tenant, in ascending run-id order.
    /// Continue after the last returned run ID; the limit is capped at
    /// [`MAX_RECOVERY_SCAN`]. An empty page finishes the scan.
    ///
    /// # Errors
    /// Returns an invalid bound, integrity or storage error.
    fn scan_recovery(
        &self,
        tenant_scope: &str,
        after: Option<RunId>,
        limit: usize,
    ) -> Result<Vec<RecoveryRegistration>, WorkerError>;
}
