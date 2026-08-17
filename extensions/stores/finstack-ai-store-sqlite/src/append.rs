use finstack_ai_kernel::{
    AppendBatchId, AppendRequest, CommittedBatch, Digest, Metadata, RecordEnvelope,
};
use finstack_ai_protocol::{commit_records, encode};
use finstack_ai_runtime::StoreError;
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};

use crate::config::SqliteStoreLimits;
use crate::error::{i64_from_u64, map_sqlite_error, protocol_error};
use crate::load::{count_sessions, load_batch, load_session_row, lookup_record_batch};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AppendIdentity {
    batch_id: [u8; 16],
    session_id: [u8; 16],
    expected_sequence: u64,
    draft_cbor: Vec<Vec<u8>>,
}

pub(crate) fn append_in_transaction(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    limits: &SqliteStoreLimits,
) -> Result<CommittedBatch, StoreError> {
    let request_cbor = request_cbor(request)?;
    if let Some(existing) = load_batch(transaction, request.batch_id())? {
        return if existing.request_cbor == request_cbor {
            Ok(existing.committed)
        } else {
            Err(StoreError::Corruption {
                reason_code: "append_batch_id_reuse",
            })
        };
    }

    if request.records().is_empty() {
        return Err(StoreError::InvalidRequest {
            reason_code: "empty_append_batch",
        });
    }

    let mut reused = Vec::new();
    for record in request.records() {
        if let Some(batch_id) = lookup_record_batch(transaction, record.record_id())? {
            reused.push(batch_id);
        }
    }
    if !reused.is_empty() {
        if reused.len() != request.records().len() {
            return Err(StoreError::Corruption {
                reason_code: "mixed_record_id_reuse",
            });
        }
        let original_batch_id = reused[0];
        if reused.iter().any(|batch_id| *batch_id != original_batch_id) {
            return Err(StoreError::Corruption {
                reason_code: "mixed_record_batch_reuse",
            });
        }
        let existing =
            load_batch(transaction, original_batch_id)?.ok_or(StoreError::Integrity {
                reason_code: "missing_record_batch_index",
            })?;
        let incoming = request_identity(request)?;
        if existing.identity.session_id == incoming.session_id
            && existing.identity.expected_sequence == incoming.expected_sequence
            && existing.identity.draft_cbor == incoming.draft_cbor
        {
            return Ok(existing.committed);
        }
        return Err(StoreError::Corruption {
            reason_code: "record_id_reuse",
        });
    }

    let session = load_session_row(transaction, request.session_id())?;
    let current_head = session.as_ref().map_or(0, |row| row.current_sequence);
    let actual_next_sequence = current_head.checked_add(1).ok_or(StoreError::Integrity {
        reason_code: "sequence_exhausted",
    })?;
    if request.expected_sequence() != actual_next_sequence {
        return Err(StoreError::Conflict {
            expected_sequence: request.expected_sequence(),
            actual_next_sequence,
        });
    }

    let session_is_new = session.is_none();
    if session_is_new && count_sessions(transaction)? >= limits.sessions {
        return Err(StoreError::LimitExceeded {
            resource: "sessions",
            limit: limits.sessions,
        });
    }
    if let Some(row) = &session {
        if row.batches >= limits.batches_per_session {
            return Err(StoreError::LimitExceeded {
                resource: "batches_per_session",
                limit: limits.batches_per_session,
            });
        }
        if row.records + request.records().len() > limits.records_per_session {
            return Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: limits.records_per_session,
            });
        }
    } else if request.records().len() > limits.records_per_session {
        return Err(StoreError::LimitExceeded {
            resource: "records_per_session",
            limit: limits.records_per_session,
        });
    }

    let previous_checksum = session.as_ref().and_then(|row| row.head_checksum);
    let committed = build_committed_batch(request, previous_checksum)?;
    persist_committed_batch(
        transaction,
        request,
        &request_cbor,
        &committed,
        session_is_new,
    )?;
    Ok(committed)
}

fn request_identity(request: &AppendRequest) -> Result<AppendIdentity, StoreError> {
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

fn request_cbor(request: &AppendRequest) -> Result<Vec<u8>, StoreError> {
    encode(&request_identity(request)?).map_err(protocol_error)
}

fn build_committed_batch(
    request: &AppendRequest,
    previous_checksum: Option<Digest>,
) -> Result<CommittedBatch, StoreError> {
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

fn persist_committed_batch(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    request_cbor: &[u8],
    committed: &CommittedBatch,
    session_is_new: bool,
) -> Result<(), StoreError> {
    if session_is_new {
        transaction
            .execute(
                "INSERT INTO sessions
                 (session_id, current_sequence, head_checksum, snapshot_sequence, metadata)
                 VALUES (?1, 0, NULL, NULL, ?2)",
                params![
                    request.session_id().as_bytes().as_slice(),
                    Metadata::empty().as_bytes(),
                ],
            )
            .map_err(map_sqlite_error)?;
    }
    transaction
        .execute(
            "INSERT INTO batches
             (batch_id, session_id, first_sequence, last_sequence, expected_sequence, request_cbor)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                request.batch_id().as_bytes().as_slice(),
                request.session_id().as_bytes().as_slice(),
                i64_from_u64(committed.first_sequence, "first_sequence")?,
                i64_from_u64(committed.last_sequence, "last_sequence")?,
                i64_from_u64(request.expected_sequence(), "expected_sequence")?,
                request_cbor,
            ],
        )
        .map_err(map_sqlite_error)?;
    for envelope in committed.records.iter() {
        insert_record(transaction, request.batch_id(), envelope)?;
    }
    let head_checksum = committed
        .records
        .last()
        .map(RecordEnvelope::checksum)
        .ok_or(StoreError::Integrity {
            reason_code: "empty_committed_batch",
        })?;
    let previous_sequence =
        request
            .expected_sequence()
            .checked_sub(1)
            .ok_or(StoreError::Integrity {
                reason_code: "sequence_exhausted",
            })?;
    let updated = transaction
        .execute(
            "UPDATE sessions
             SET current_sequence = ?1, head_checksum = ?2
             WHERE session_id = ?3 AND current_sequence = ?4",
            params![
                i64_from_u64(committed.last_sequence, "last_sequence")?,
                head_checksum.as_bytes().as_slice(),
                request.session_id().as_bytes().as_slice(),
                i64_from_u64(previous_sequence, "previous_sequence")?,
            ],
        )
        .map_err(map_sqlite_error)?;
    if updated != 1 {
        return Err(StoreError::Integrity {
            reason_code: "sequence_cas_failed",
        });
    }
    Ok(())
}

fn insert_record(
    transaction: &Transaction<'_>,
    batch_id: AppendBatchId,
    envelope: &RecordEnvelope,
) -> Result<(), StoreError> {
    let payload_cbor = encode(envelope.body()).map_err(protocol_error)?;
    let derived_event_ids = encode(&envelope.derived_event_ids()).map_err(protocol_error)?;
    let run_id = envelope.run_id().map(|id| id.to_bytes());
    let previous = envelope
        .previous_checksum()
        .map(|digest| *digest.as_bytes());
    transaction
        .execute(
            "INSERT INTO records (
                session_id, sequence, record_id, batch_id, lane_id, run_id, kind,
                format_version, kind_version, payload_cbor, timestamp, committed_at,
                payload_digest, previous_checksum, envelope_checksum, derived_event_ids
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?13, ?14, ?15)",
            params![
                envelope.session_id().as_bytes().as_slice(),
                i64_from_u64(envelope.sequence(), "sequence")?,
                envelope.record_id().as_bytes().as_slice(),
                batch_id.as_bytes().as_slice(),
                envelope.lane_id().as_bytes().as_slice(),
                run_id.as_ref().map(<[u8; 16]>::as_slice),
                envelope.body().kind_name(),
                i64::from(envelope.format_version()),
                i64::from(envelope.kind_version()),
                payload_cbor,
                envelope.timestamp().as_unix_ms(),
                envelope.payload_digest().as_bytes().as_slice(),
                previous.as_ref().map(<[u8; 32]>::as_slice),
                envelope.checksum().as_bytes().as_slice(),
                derived_event_ids,
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}
