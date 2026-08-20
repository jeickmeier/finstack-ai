//! Tenant/policy instruction injection middleware (`prepare_context`).

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
#![doc(test(attr(allow(clippy::expect_used))))]

use serde::Serialize;
use thiserror::Error;

/// Maximum number of policy entries one middleware may inject.
pub const MAX_POLICY_ENTRIES: usize = 16;

/// Instructions-leaf construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstructionsError {
    /// Configuration is malformed.
    #[error("instructions_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// One policy instruction entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyEntry {
    /// Stable label; becomes the item's provenance source id (`policy:{label}`).
    pub label: String,
    /// Instruction text injected as a System message.
    pub text: String,
}

/// Ordered policy instruction set frozen at construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyInstructionsConfig {
    /// Entries injected in order at `prepare_context`.
    pub entries: Vec<PolicyEntry>,
}

impl PolicyInstructionsConfig {
    /// Validate entry count and per-entry label/text.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration reason for empty, oversized, or blank input.
    pub fn validate(&self) -> Result<(), InstructionsError> {
        if self.entries.is_empty() {
            return Err(InstructionsError::Configuration {
                reason: "entries_empty",
            });
        }
        if self.entries.len() > MAX_POLICY_ENTRIES {
            return Err(InstructionsError::Configuration {
                reason: "entries_exceed_maximum",
            });
        }
        for entry in &self.entries {
            if entry.label.trim().is_empty() {
                return Err(InstructionsError::Configuration {
                    reason: "entry_label_empty",
                });
            }
            if entry.text.trim().is_empty() {
                return Err(InstructionsError::Configuration {
                    reason: "entry_text_empty",
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
