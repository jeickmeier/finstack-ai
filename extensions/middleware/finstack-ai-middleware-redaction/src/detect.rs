//! Pure, deterministic secret/PII detection and marker substitution.

use crate::{RedactionConfig, RedactionError};

/// Compiled detector set. Built once at middleware construction; running a
/// detector never fails after that.
#[derive(Debug)]
pub(crate) struct Detectors {}

impl Detectors {
    /// Compile the detectors enabled by `config`.
    ///
    /// # Errors
    ///
    /// Rejects an all-disabled detector set.
    pub(crate) fn try_new(config: RedactionConfig) -> Result<Self, RedactionError> {
        if !config.detect_emails && !config.detect_api_keys && !config.detect_account_numbers {
            return Err(RedactionError::Configuration {
                reason: "all_detectors_disabled",
            });
        }
        Ok(Self {})
    }
}
