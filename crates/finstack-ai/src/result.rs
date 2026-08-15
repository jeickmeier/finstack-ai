//! Typed decoding of already committed structured results.

use finstack_ai_runtime::{Digest, FinalResultRecorded, RawJson, SchemaRef};
use serde::de::DeserializeOwned;
use thiserror::Error;

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_runtime::{
        EffectId, JsonSchemaDraft, MessageId, ModelRequestId, OutputEndStrategy,
        StructuredResultSource, ToolCallId, TurnId,
    };
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    struct Answer {
        nested: Nested,
    }

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    struct Nested {
        count: u32,
    }

    fn schema(bytes: &[u8]) -> SchemaRef {
        SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: Digest::raw_json(bytes),
        }
    }

    fn committed(value: RawJson, schema: SchemaRef) -> FinalResultRecorded {
        FinalResultRecorded {
            cycle: 1,
            turn_id: TurnId::from_bytes([1; 16]),
            model_request_id: ModelRequestId::from_bytes([2; 16]),
            effect_id: EffectId::from_bytes([3; 16]),
            message_id: MessageId::from_bytes([4; 16]),
            schema,
            value_digest: value.digest(),
            value,
            source: StructuredResultSource::InternalTool {
                tool_call_id: ToolCallId::from_bytes([5; 16]),
            },
            end_strategy: OutputEndStrategy::Exhaustive,
            skipped_tool_call_ids: Arc::from([]),
        }
    }

    #[test]
    fn decodes_exact_committed_bytes() {
        let schema = schema(b"answer-schema");
        let record = committed(
            RawJson::parse(br#"{"nested":{"count":3}}"#).expect("raw JSON"),
            schema.clone(),
        );
        let result = RunResult::try_from_committed(&schema, &record).expect("result");
        assert_eq!(
            result.decode::<Answer>().expect("typed result"),
            Answer {
                nested: Nested { count: 3 }
            }
        );
    }

    #[test]
    fn reports_schema_and_nested_type_mismatches() {
        let expected_schema = schema(b"answer-schema");
        let record = committed(
            RawJson::parse(br#"{"nested":{"count":"three"}}"#).expect("raw JSON"),
            expected_schema.clone(),
        );
        let mismatch = RunResult::try_from_committed(&schema(b"different"), &record)
            .expect_err("schema mismatch");
        assert!(matches!(mismatch, ResultDecodeError::SchemaMismatch { .. }));

        let result = RunResult::try_from_committed(&expected_schema, &record).expect("result");
        let error = result.decode::<Answer>().expect_err("type mismatch");
        match error {
            ResultDecodeError::InvalidValue { path, .. } => {
                assert_eq!(path, "nested.count");
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
