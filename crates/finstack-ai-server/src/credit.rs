//! Credit window configuration and mutable flow-control state.

use std::time::Duration;

use crate::ServerError;

/// Credit window configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditLimits {
    /// Initial item credits.
    pub items: u32,
    /// Initial byte credits.
    pub bytes: u32,
    /// How long the server waits for an ack before disconnecting.
    pub ack_deadline: Duration,
}

impl Default for CreditLimits {
    fn default() -> Self {
        Self {
            items: 8,
            bytes: 64 * 1024,
            ack_deadline: Duration::from_millis(200),
        }
    }
}

/// Mutable credit window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CreditWindow {
    items: u32,
    bytes: u32,
    limits: CreditLimits,
}

impl CreditWindow {
    /// Open a window with `limits` already granted.
    #[must_use]
    pub const fn new(limits: CreditLimits) -> Self {
        Self {
            items: limits.items,
            bytes: limits.bytes,
            limits,
        }
    }

    /// Remaining item credits.
    #[must_use]
    pub const fn items(&self) -> u32 {
        self.items
    }

    /// Remaining byte credits.
    #[must_use]
    pub const fn bytes(&self) -> u32 {
        self.bytes
    }

    /// Configured limits.
    #[must_use]
    pub const fn limits(&self) -> CreditLimits {
        self.limits
    }

    /// Consume credits for one outbound batch.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::CreditTimeout`] when the window cannot cover the
    /// batch (same stable code as an ack-deadline disconnect). The caller
    /// disconnects and the client resumes from its cursor.
    pub fn consume(&mut self, items: u32, bytes: u32) -> Result<(), ServerError> {
        if self.items < items || self.bytes < bytes {
            return Err(ServerError::CreditTimeout);
        }
        self.items -= items;
        self.bytes -= bytes;
        Ok(())
    }

    /// Restore credits from a client ack.
    pub fn ack(&mut self, items: u32, bytes: u32) {
        self.items = self.items.saturating_add(items).min(self.limits.items);
        self.bytes = self.bytes.saturating_add(bytes).min(self.limits.bytes);
    }
}
