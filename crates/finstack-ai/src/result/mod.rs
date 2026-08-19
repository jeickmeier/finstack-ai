//! Typed decoding of already committed structured results.

use finstack_ai_kernel::{Digest, FinalResultRecorded, RawJson, SchemaRef};
use serde::de::DeserializeOwned;
use thiserror::Error;

#[cfg(test)]
mod tests;

/// Stable code for a committed/resolved schema mismatch.
pub const RESULT_DECODE_SCHEMA_MISMATCH: &str = "result_decode_schema_mismatch";
/// Stable code for a path-aware typed decoding failure.
pub const RESULT_DECODE_INVALID_VALUE: &str = "result_decode_invalid_value";

/// Immutable view of a committed structured final result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    schema: SchemaRef,
    value: RawJson,
    value_digest: Digest,
}

impl RunResult {
    /// Construct a result only when the committed schema matches the resolved output spec.
    ///
    /// # Errors
    ///
    /// Returns [`ResultDecodeError::SchemaMismatch`] before exposing typed decoding
    /// if the exact schema reference or committed value digest differs.
    pub fn try_from_committed(
        expected_schema: &SchemaRef,
        committed: &FinalResultRecorded,
    ) -> Result<Self, ResultDecodeError> {
        if expected_schema != &committed.schema {
            return Err(ResultDecodeError::SchemaMismatch {
                expected: expected_schema.schema_digest,
                committed: committed.schema.schema_digest,
            });
        }
        if committed.value_digest != committed.value.digest() {
            return Err(ResultDecodeError::ValueDigestMismatch {
                expected: committed.value_digest,
                actual: committed.value.digest(),
            });
        }
        Ok(Self {
            schema: committed.schema.clone(),
            value: committed.value.clone(),
            value_digest: committed.value_digest,
        })
    }

    /// Borrow the exact schema reference used for validation and commitment.
    #[must_use]
    pub fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    /// Borrow the canonical committed JSON value.
    #[must_use]
    pub fn value(&self) -> &RawJson {
        &self.value
    }

    /// Return the committed value digest.
    #[must_use]
    pub const fn value_digest(&self) -> Digest {
        self.value_digest
    }

    /// Decode the already validated canonical JSON bytes with a stable error path.
    ///
    /// This method performs no model call and mutates no run state.
    ///
    /// # Errors
    ///
    /// Returns [`ResultDecodeError::InvalidValue`] with the Serde structural path.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, ResultDecodeError> {
        let mut decoder = serde_json::Deserializer::from_slice(self.value.as_bytes());
        serde_path_to_error::deserialize(&mut decoder).map_err(|error| {
            ResultDecodeError::InvalidValue {
                path: error.path().to_string(),
                message: error.inner().to_string(),
            }
        })
    }
}

/// Typed structured-result decoding failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResultDecodeError {
    /// Resolved and committed schemas differ.
    #[error(
        "{}: expected schema {expected}, committed schema {committed}",
        RESULT_DECODE_SCHEMA_MISMATCH
    )]
    SchemaMismatch {
        /// Resolved schema digest.
        expected: Digest,
        /// Committed schema digest.
        committed: Digest,
    },
    /// Committed value bytes no longer match their durable digest.
    #[error(
        "{}: expected value {expected}, actual value {actual}",
        RESULT_DECODE_INVALID_VALUE
    )]
    ValueDigestMismatch {
        /// Committed digest.
        expected: Digest,
        /// Digest recomputed from canonical bytes.
        actual: Digest,
    },
    /// Serde could not decode the already committed value.
    #[error("{} at {path}: {message}", RESULT_DECODE_INVALID_VALUE)]
    InvalidValue {
        /// Stable structural path.
        path: String,
        /// Serde diagnostic.
        message: String,
    },
}

impl ResultDecodeError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SchemaMismatch { .. } => RESULT_DECODE_SCHEMA_MISMATCH,
            Self::ValueDigestMismatch { .. } | Self::InvalidValue { .. } => {
                RESULT_DECODE_INVALID_VALUE
            }
        }
    }
}
