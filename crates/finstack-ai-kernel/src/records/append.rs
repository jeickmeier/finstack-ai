//! Append requests with expected-sequence preconditions.

use serde::{Deserialize, Serialize};

use crate::primitives::{AppendBatchId, SessionId};

use super::APPEND_BATCH_MAX_RECORDS;
use super::draft::RecordDraft;
use super::error::RecordError;
use super::validate::validate_interaction_request_pairs;
use crate::primitives::BoundedVec;
use serde::de;
use std::sync::Arc;

/// Append request with expected-sequence precondition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppendRequest {
    batch_id: AppendBatchId,
    session_id: SessionId,
    expected_sequence: u64,
    records: Arc<[RecordDraft]>,
}

impl AppendRequest {
    /// Construct an append request enforcing the batch record-count ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError::BatchTooLarge`] when over [`APPEND_BATCH_MAX_RECORDS`].
    pub fn try_new(
        batch_id: AppendBatchId,
        session_id: SessionId,
        expected_sequence: u64,
        records: Vec<RecordDraft>,
    ) -> Result<Self, RecordError> {
        if records.len() > APPEND_BATCH_MAX_RECORDS {
            return Err(RecordError::BatchTooLarge {
                len: records.len(),
                max: APPEND_BATCH_MAX_RECORDS,
            });
        }
        if records
            .iter()
            .any(|record| record.session_id() != session_id)
        {
            return Err(RecordError::RecordSessionMismatch);
        }
        validate_interaction_request_pairs(&records)?;
        Ok(Self {
            batch_id,
            session_id,
            expected_sequence,
            records: records.into(),
        })
    }

    /// Batch id.
    #[must_use]
    pub fn batch_id(&self) -> AppendBatchId {
        self.batch_id
    }

    /// Session id.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Expected sequence precondition.
    #[must_use]
    pub fn expected_sequence(&self) -> u64 {
        self.expected_sequence
    }

    /// Records.
    #[must_use]
    pub fn records(&self) -> &[RecordDraft] {
        &self.records
    }
}

impl<'de> Deserialize<'de> for AppendRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            batch_id: AppendBatchId,
            session_id: SessionId,
            expected_sequence: u64,
            records: BoundedVec<RecordDraft, APPEND_BATCH_MAX_RECORDS>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.batch_id,
            wire.session_id,
            wire.expected_sequence,
            wire.records.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}
