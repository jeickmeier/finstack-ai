use finstack_ai_kernel::{AppendBatchId, AppendRequest, CommittedBatch, Metadata, RecordEnvelope};
use finstack_ai_protocol::encode;
use finstack_ai_runtime::ports::journal::StoreError;
pub(crate) use finstack_ai_store_common::AppendIdentity;
#[cfg(test)]
pub(crate) use finstack_ai_store_common::request_cbor;
use finstack_ai_store_common::{
    SessionUsage, admit_append_limits, build_committed_batch, check_append_sequence,
    classify_record_reuse, request_identity,
};
use rusqlite::{Transaction, params};

use crate::config::SqliteStoreLimits;
use crate::error::{i64_from_u64, map_sqlite_error, protocol_error};
use crate::load::{count_sessions, load_batch, load_session_row, lookup_record_batch};

pub(crate) fn append_in_transaction(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    limits: &SqliteStoreLimits,
) -> Result<CommittedBatch, StoreError> {
    let incoming_identity = request_identity(request)?;
    if let Some(existing) = load_batch(transaction, request.batch_id())? {
        let Some(committed) = existing.committed else {
            return if existing.identity == incoming_identity {
                Err(StoreError::InvalidRequest {
                    reason_code: "append_history_pruned",
                })
            } else {
                Err(StoreError::Corruption {
                    reason_code: "append_batch_id_reuse",
                })
            };
        };
        return if existing.identity == incoming_identity {
            Ok(committed)
        } else {
            Err(StoreError::Corruption {
                reason_code: "append_batch_id_reuse",
            })
        };
    }
    let request_cbor = encode(&incoming_identity).map_err(protocol_error)?;

    if request.records().is_empty() {
        return Err(StoreError::InvalidRequest {
            reason_code: "empty_append_batch",
        });
    }

    let mut hits = Vec::new();
    for record in request.records() {
        if let Some(batch_id) = lookup_record_batch(transaction, record.record_id())? {
            hits.push(batch_id);
        }
    }
    if let Some(original_batch_id) = classify_record_reuse(&hits, request.records().len())? {
        let existing =
            load_batch(transaction, original_batch_id)?.ok_or(StoreError::Integrity {
                reason_code: "missing_record_batch_index",
            })?;
        let Some(committed) = existing.committed else {
            return Err(StoreError::InvalidRequest {
                reason_code: "append_history_pruned",
            });
        };
        if existing.identity.session_id == incoming_identity.session_id
            && existing.identity.expected_sequence == incoming_identity.expected_sequence
            && existing.identity.draft_cbor == incoming_identity.draft_cbor
        {
            return Ok(committed);
        }
        return Err(StoreError::Corruption {
            reason_code: "record_id_reuse",
        });
    }

    let session = load_session_row(transaction, request.session_id())?;
    let current_head = session.as_ref().map_or(0, |row| row.current_sequence);
    check_append_sequence(current_head, request.expected_sequence())?;

    let session_is_new = session.is_none();
    let usage = session.as_ref().map(|row| SessionUsage {
        batches: row.batches,
        records: row.records,
    });
    admit_append_limits(
        *limits,
        if session_is_new {
            count_sessions(transaction)?
        } else {
            0
        },
        usage,
        request.records().len(),
    )?;

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
