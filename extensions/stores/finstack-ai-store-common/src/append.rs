//! Append admission and batch-commitment semantics shared by all backends.

use finstack_ai_kernel::{AppendBatchId, AppendRequest, CommittedBatch, Digest};
use finstack_ai_protocol::{commit_records, encode};
use finstack_ai_runtime::ports::journal::{StoreError, StoreLimits};
use serde::{Deserialize, Serialize};

use crate::error::protocol_error;

/// Durable identity of one append request, compared byte-for-byte on replay.
///
/// The CBOR encoding of this struct is a persisted format (sqlite stores it
/// in `batches.request_cbor`). Do not reorder, rename, or retype fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendIdentity {
    /// Append batch id bytes.
    pub batch_id: [u8; 16],
    /// Session id bytes.
    pub session_id: [u8; 16],
    /// Optimistic next-sequence precondition.
    pub expected_sequence: u64,
    /// Canonical CBOR of each record draft, in request order.
    #[serde(with = "byte_vecs")]
    pub draft_cbor: Vec<Vec<u8>>,
}

mod byte_vecs {
    use std::fmt;

    use serde::de::{SeqAccess, Visitor};
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S>(values: &[Vec<u8>], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        struct Bytes<'a>(&'a [u8]);

        impl Serialize for Bytes<'_> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                if self.0.len() > finstack_ai_protocol::CANONICAL_ARRAY_MAX_ITEMS {
                    return serializer.serialize_bytes(self.0);
                }
                let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
                for byte in self.0 {
                    sequence.serialize_element(byte)?;
                }
                sequence.end()
            }
        }

        let mut sequence = serializer.serialize_seq(Some(values.len()))?;
        for value in values {
            sequence.serialize_element(&Bytes(value))?;
        }
        sequence.end()
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<Vec<u8>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Bytes(Vec<u8>);

        impl<'de> Deserialize<'de> for Bytes {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct BytesVisitor;

                impl<'de> Visitor<'de> for BytesVisitor {
                    type Value = Bytes;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a byte string or an array of bytes")
                    }

                    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E> {
                        Ok(Bytes(value.to_vec()))
                    }

                    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                        Ok(Bytes(value))
                    }

                    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
                    where
                        A: SeqAccess<'de>,
                    {
                        let mut bytes = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
                        while let Some(byte) = sequence.next_element()? {
                            bytes.push(byte);
                        }
                        Ok(Bytes(bytes))
                    }
                }

                deserializer.deserialize_any(BytesVisitor)
            }
        }

        Vec::<Bytes>::deserialize(deserializer)
            .map(|values| values.into_iter().map(|value| value.0).collect())
    }
}

/// Compute the durable identity of one append request.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when a draft cannot be canonically encoded.
pub fn request_identity(request: &AppendRequest) -> Result<AppendIdentity, StoreError> {
    let draft_cbor = request
        .records()
        .iter()
        .map(|draft| encode(draft).map_err(protocol_error))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AppendIdentity {
        batch_id: request.batch_id().to_bytes(),
        session_id: request.session_id().to_bytes(),
        expected_sequence: request.expected_sequence(),
        draft_cbor,
    })
}

/// Canonical CBOR bytes of [`request_identity`].
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when encoding fails.
pub fn request_cbor(request: &AppendRequest) -> Result<Vec<u8>, StoreError> {
    encode(&request_identity(request)?).map_err(protocol_error)
}

/// Classify record-id reuse for one append.
///
/// `hits` holds the original batch id of every incoming record that already
/// exists, in any order; `total_records` is the incoming record count.
/// `Ok(None)` means a fresh append. `Ok(Some(batch))` means every record was
/// previously committed in `batch` — the caller must compare identities and
/// either replay or fail with `record_id_reuse`.
///
/// # Errors
///
/// Returns [`StoreError::Corruption`] for partial reuse
/// (`mixed_record_id_reuse`) or reuse spanning batches
/// (`mixed_record_batch_reuse`).
pub fn classify_record_reuse(
    hits: &[AppendBatchId],
    total_records: usize,
) -> Result<Option<AppendBatchId>, StoreError> {
    let Some(first) = hits.first() else {
        return Ok(None);
    };
    if hits.len() != total_records {
        return Err(StoreError::Corruption {
            reason_code: "mixed_record_id_reuse",
        });
    }
    if hits.iter().any(|batch_id| batch_id != first) {
        return Err(StoreError::Corruption {
            reason_code: "mixed_record_batch_reuse",
        });
    }
    Ok(Some(*first))
}

/// Enforce the optimistic append-sequence precondition.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] (`sequence_exhausted`) when the journal
/// head cannot advance, and [`StoreError::Conflict`] when
/// `expected_sequence` is not the next sequence.
pub fn check_append_sequence(current_head: u64, expected_sequence: u64) -> Result<(), StoreError> {
    let actual_next_sequence = current_head.checked_add(1).ok_or(StoreError::Integrity {
        reason_code: "sequence_exhausted",
    })?;
    if expected_sequence != actual_next_sequence {
        return Err(StoreError::Conflict {
            expected_sequence,
            actual_next_sequence,
        });
    }
    Ok(())
}

/// Current committed footprint of one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionUsage {
    /// Committed batches in the session.
    pub batches: usize,
    /// Committed records in the session.
    pub records: usize,
}

/// Enforce store resource ceilings for one append, in contract order:
/// sessions, then batches, then records.
///
/// `usage` is `None` for a session that does not exist yet; `sessions` is the
/// current distinct-session count.
///
/// # Errors
///
/// Returns [`StoreError::LimitExceeded`] naming the exhausted resource.
pub fn admit_append_limits(
    limits: StoreLimits,
    sessions: usize,
    usage: Option<SessionUsage>,
    incoming_records: usize,
) -> Result<(), StoreError> {
    match usage {
        None => {
            if sessions >= limits.sessions {
                return Err(StoreError::LimitExceeded {
                    resource: "sessions",
                    limit: limits.sessions,
                });
            }
            if incoming_records > limits.records_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "records_per_session",
                    limit: limits.records_per_session,
                });
            }
        }
        Some(usage) => {
            if usage.batches >= limits.batches_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "batches_per_session",
                    limit: limits.batches_per_session,
                });
            }
            if usage.records + incoming_records > limits.records_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "records_per_session",
                    limit: limits.records_per_session,
                });
            }
        }
    }
    Ok(())
}

/// Seal one frozen request into a committed, checksum-chained batch.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] (`empty_append_batch`) when
/// `request` has no records, and [`StoreError::Integrity`] when commitment
/// or chaining fails.
pub fn build_committed_batch(
    request: &AppendRequest,
    previous_checksum: Option<Digest>,
) -> Result<CommittedBatch, StoreError> {
    if request.records().is_empty() {
        return Err(StoreError::InvalidRequest {
            reason_code: "empty_append_batch",
        });
    }
    let records = commit_records(
        request.records(),
        request.expected_sequence(),
        previous_checksum,
        None,
    )
    .map_err(protocol_error)?;
    let last_sequence = request
        .expected_sequence()
        .checked_add(
            u64::try_from(records.len() - 1).map_err(|_| StoreError::Integrity {
                reason_code: "record_count_overflow",
            })?,
        )
        .ok_or(StoreError::Integrity {
            reason_code: "sequence_exhausted",
        })?;
    CommittedBatch::try_new(
        request.batch_id(),
        request.expected_sequence(),
        last_sequence,
        records,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "committed_batch_invalid",
    })
}

#[cfg(test)]
mod tests {
    use finstack_ai_runtime::ports::journal::StoreLimits;

    use super::*;
    use finstack_ai_test::store_fixtures::{draft, id, request};

    fn limits() -> StoreLimits {
        StoreLimits {
            sessions: 2,
            batches_per_session: 2,
            records_per_session: 3,
            snapshot_bytes: 1024,
        }
    }

    #[test]
    fn record_reuse_classification_matches_store_contract() {
        // No hits: fresh append.
        assert_eq!(classify_record_reuse(&[], 2).expect("fresh"), None);
        // All records previously committed in one batch: replay candidate.
        let batch = id(9);
        assert_eq!(
            classify_record_reuse(&[batch, batch], 2).expect("replay"),
            Some(batch)
        );
        // Partial reuse is corruption.
        assert!(matches!(
            classify_record_reuse(&[batch], 2),
            Err(StoreError::Corruption {
                reason_code: "mixed_record_id_reuse"
            })
        ));
        // Reuse spanning two original batches is corruption.
        assert!(matches!(
            classify_record_reuse(&[batch, id(10)], 2),
            Err(StoreError::Corruption {
                reason_code: "mixed_record_batch_reuse"
            })
        ));
    }

    #[test]
    fn sequence_admission_reports_conflict_and_exhaustion() {
        check_append_sequence(0, 1).expect("genesis");
        check_append_sequence(41, 42).expect("next");
        assert!(matches!(
            check_append_sequence(41, 41),
            Err(StoreError::Conflict {
                expected_sequence: 41,
                actual_next_sequence: 42
            })
        ));
        assert!(matches!(
            check_append_sequence(u64::MAX, 1),
            Err(StoreError::Integrity {
                reason_code: "sequence_exhausted"
            })
        ));
    }

    #[test]
    fn limit_admission_orders_sessions_then_batches_then_records() {
        // New session over the session ceiling.
        assert!(matches!(
            admit_append_limits(limits(), 2, None, 1),
            Err(StoreError::LimitExceeded {
                resource: "sessions",
                limit: 2
            })
        ));
        // Existing session over the batch ceiling.
        let full_batches = SessionUsage {
            batches: 2,
            records: 2,
        };
        assert!(matches!(
            admit_append_limits(limits(), 1, Some(full_batches), 1),
            Err(StoreError::LimitExceeded {
                resource: "batches_per_session",
                limit: 2
            })
        ));
        // Existing session over the record ceiling.
        let full_records = SessionUsage {
            batches: 1,
            records: 3,
        };
        assert!(matches!(
            admit_append_limits(limits(), 1, Some(full_records), 1),
            Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: 3
            })
        ));
        // New session whose first batch already exceeds the record ceiling.
        assert!(matches!(
            admit_append_limits(limits(), 0, None, 4),
            Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: 3
            })
        ));
        admit_append_limits(
            limits(),
            1,
            Some(SessionUsage {
                batches: 1,
                records: 2,
            }),
            1,
        )
        .expect("admitted");
    }

    #[test]
    fn committed_batches_chain_from_the_previous_checksum() {
        let first =
            build_committed_batch(&request(1, 1, 1, vec![draft(1, 1)]), None).expect("first batch");
        assert_eq!((first.first_sequence, first.last_sequence), (1, 1));
        let prior = first.records[0].checksum();
        let second = build_committed_batch(
            &request(2, 1, 2, vec![draft(2, 1), draft(3, 1)]),
            Some(prior),
        )
        .expect("second batch");
        assert_eq!((second.first_sequence, second.last_sequence), (2, 3));
        assert_eq!(second.records[0].previous_checksum(), Some(prior));
    }

    #[test]
    fn committed_batch_rejects_an_empty_append_request() {
        assert!(matches!(
            build_committed_batch(&request(1, 1, 1, Vec::new()), None),
            Err(StoreError::InvalidRequest {
                reason_code: "empty_append_batch"
            })
        ));
    }

    #[test]
    fn identity_round_trips_and_is_deterministic() {
        let frozen = request(7, 3, 5, vec![draft(70, 3), draft(71, 3)]);
        let identity = request_identity(&frozen).expect("identity");
        assert_eq!(identity.expected_sequence, 5);
        assert_eq!(identity.draft_cbor.len(), 2);
        assert_eq!(
            request_cbor(&frozen).expect("bytes"),
            request_cbor(&frozen).expect("bytes again")
        );
    }

    #[test]
    fn identity_encodes_large_drafts_as_byte_strings() {
        let identity = AppendIdentity {
            batch_id: [1; 16],
            session_id: [2; 16],
            expected_sequence: 3,
            draft_cbor: vec![vec![7; 4_097]],
        };

        let encoded = encode(&identity).expect("large draft identity");
        let decoded = finstack_ai_protocol::decode::<AppendIdentity>(&encoded).expect("decode");
        assert_eq!(decoded, identity);
    }

    #[test]
    fn identity_decodes_legacy_byte_arrays() {
        #[derive(Serialize)]
        struct LegacyIdentity {
            batch_id: [u8; 16],
            session_id: [u8; 16],
            expected_sequence: u64,
            draft_cbor: Vec<Vec<u8>>,
        }

        let legacy = LegacyIdentity {
            batch_id: [1; 16],
            session_id: [2; 16],
            expected_sequence: 3,
            draft_cbor: vec![vec![4, 5, 6]],
        };
        let encoded = encode(&legacy).expect("legacy identity");
        let decoded = finstack_ai_protocol::decode::<AppendIdentity>(&encoded).expect("decode");
        assert_eq!(decoded.draft_cbor, legacy.draft_cbor);
    }
}
