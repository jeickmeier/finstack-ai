//! Sensitivity and diagnostic records.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};

use super::refs_error::{RefsError, validated_label};
use crate::primitives::Metadata;

/// Sensitivity classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Public.
    Public,
    /// Internal.
    Internal,
    /// Confidential.
    Confidential,
    /// Secret.
    Secret,
    /// Credential material class (never place secrets in records/events).
    Credential,
}

/// Decision-local diagnostic (never a [`crate::events::RunEvent`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    code: Arc<str>,
    message: Arc<str>,
    severity: DiagnosticSeverity,
    metadata: Metadata,
}

impl Diagnostic {
    /// Construct a diagnostic.
    ///
    /// # Arguments
    ///
    /// * `code` - Stable diagnostic code label.
    /// * `message` - Human-readable diagnostic text (non-empty, no NUL).
    /// * `severity` - Diagnostic severity.
    /// * `metadata` - Non-authoritative diagnostic metadata.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `code` fails label rules, or when
    /// `message` is empty/oversized/NUL-bearing.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{Diagnostic, DiagnosticSeverity, Metadata};
    ///
    /// let diagnostic = Diagnostic::try_new(
    ///     "duplicate_decision",
    ///     "input already applied",
    ///     DiagnosticSeverity::Info,
    ///     Metadata::empty(),
    /// )
    /// .expect("diagnostic");
    /// assert_eq!(diagnostic.code(), "duplicate_decision");
    /// ```
    pub fn try_new(
        code: impl AsRef<str>,
        message: impl AsRef<str>,
        severity: DiagnosticSeverity,
        metadata: Metadata,
    ) -> Result<Self, RefsError> {
        let message = message.as_ref();
        if message.is_empty()
            || message.len() > crate::content::TEXT_MAX_BYTES
            || message.as_bytes().contains(&0)
        {
            return Err(RefsError::InvalidLabel { field: "message" });
        }
        Ok(Self {
            code: validated_label(code.as_ref(), "code")?,
            message: Arc::<str>::from(message),
            severity,
            metadata,
        })
    }

    /// Borrow the code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Borrow the message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Borrow severity.
    #[must_use]
    pub fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    /// Borrow metadata.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

impl<'de> Deserialize<'de> for Diagnostic {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            code: BoundedString<LABEL_MAX_BYTES>,
            message: BoundedString<TEXT_MAX_BYTES>,
            severity: DiagnosticSeverity,
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.code.into_inner(),
            wire.message.into_inner(),
            wire.severity,
            wire.metadata,
        )
        .map_err(de::Error::custom)
    }
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Debug.
    Debug,
    /// Info.
    Info,
    /// Warning.
    Warning,
    /// Error.
    Error,
}
