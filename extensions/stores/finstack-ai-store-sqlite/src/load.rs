use std::sync::Arc;

use finstack_ai_kernel::{
    AppendBatchId, CommittedBatch, Digest, EventId, Id, IdTag, Metadata, RecordBody,
    RecordEnvelope, RecordId, SessionId, Timestamp,
};
use finstack_ai_protocol::{ChainAnchor, decode, verify_chain_from};
use finstack_ai_runtime::ports::journal::{
    LoadWindow, LoadedSession, OpaqueSnapshot, ScanPage, ScanRequest, StoreError,
};
use finstack_ai_store_common::{
    AppendIdentity, FROM_SEQUENCE_WINDOW, SNAPSHOT_WINDOW, SessionUsage, VerifiedHead, WindowCodes,
    accelerated_from, check_batch_alignment, scan_next_sequence, scan_start, validate_scan_limit,
    verify_head_against_cache, verify_tail_records,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{
    i64_from_u64, map_sqlite_error, protocol_error, u16_from_i64, u64_from_i64, usize_from_i64,
};

pub(crate) struct LoadedBatch {
    pub(crate) identity: AppendIdentity,
    pub(crate) committed: Option<CommittedBatch>,
}

pub(crate) struct SessionRow {
    pub(crate) current_sequence: u64,
    pub(crate) head_checksum: Option<Digest>,
    pub(crate) snapshot_sequence: Option<u64>,
    pub(crate) chain_anchor_sequence: u64,
    pub(crate) chain_anchor_checksum: Option<Digest>,
}

pub(crate) struct StoredRecord {
    pub(crate) batch_id: AppendBatchId,
    pub(crate) envelope: RecordEnvelope,
}

pub(crate) fn load_batch(
    connection: &Connection,
    batch_id: AppendBatchId,
) -> Result<Option<LoadedBatch>, StoreError> {
    let Some((request_cbor, first_sequence, last_sequence)) = connection
        .query_row(
            "SELECT request_cbor, first_sequence, last_sequence FROM batches WHERE batch_id = ?1",
            params![batch_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
    else {
        return Ok(None);
    };
    let identity = decode::<AppendIdentity>(&request_cbor).map_err(protocol_error)?;
    let records = load_batch_records(connection, batch_id)?;
    let first_sequence = u64_from_i64(first_sequence, "first_sequence")?;
    let last_sequence = u64_from_i64(last_sequence, "last_sequence")?;
    let committed = if records
        .first()
        .is_some_and(|record| record.sequence() > first_sequence)
        && records
            .last()
            .is_some_and(|record| record.sequence() == last_sequence)
    {
        None
    } else {
        Some(
            CommittedBatch::try_new(batch_id, first_sequence, last_sequence, records).map_err(
                |_| StoreError::Integrity {
                    reason_code: "committed_batch_invalid",
                },
            )?,
        )
    };
    Ok(Some(LoadedBatch {
        identity,
        committed,
    }))
}

fn load_batch_records(
    connection: &Connection,
    batch_id: AppendBatchId,
) -> Result<Vec<RecordEnvelope>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT session_id, sequence, record_id, lane_id, run_id, kind,
                    format_version, kind_version, payload_cbor, timestamp,
                    payload_digest, previous_checksum, envelope_checksum, derived_event_ids
             FROM records WHERE batch_id = ?1 ORDER BY sequence",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![batch_id.as_bytes().as_slice()], row_to_envelope)
        .map_err(map_sqlite_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
}

/// Every record of `session_id` at or after `from_sequence`, in sequence
/// order, capped at `limit` rows when given.
fn load_session_records(
    connection: &Connection,
    session_id: SessionId,
    from_sequence: u64,
    limit: Option<u32>,
) -> Result<Vec<StoredRecord>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT session_id, sequence, record_id, lane_id, run_id, kind,
                    format_version, kind_version, payload_cbor, timestamp,
                    payload_digest, previous_checksum, envelope_checksum, derived_event_ids,
                    batch_id
             FROM records
             WHERE session_id = ?1 AND sequence >= ?2
             ORDER BY sequence
             LIMIT ?3",
        )
        .map_err(map_sqlite_error)?;
    // A negative `LIMIT` is sqlite's "no upper bound".
    let rows = statement
        .query_map(
            params![
                session_id.as_bytes().as_slice(),
                i64_from_u64(from_sequence, "from_sequence")?,
                limit.map_or(-1, i64::from),
            ],
            |row| Ok((row.get::<_, Vec<u8>>(14)?, row_to_envelope(row)?)),
        )
        .map_err(map_sqlite_error)?;
    let mut stored = Vec::new();
    for row in rows {
        let (batch_bytes, envelope) = row.map_err(map_sqlite_error)?;
        stored.push(StoredRecord {
            batch_id: id_from_blob(&batch_bytes)?,
            envelope: envelope?,
        });
    }
    Ok(stored)
}

pub(crate) fn load_envelope_checksum(
    connection: &Connection,
    session_id: SessionId,
    sequence: u64,
) -> Result<Option<Digest>, StoreError> {
    let bytes = connection
        .query_row(
            "SELECT envelope_checksum FROM records WHERE session_id = ?1 AND sequence = ?2",
            params![
                session_id.as_bytes().as_slice(),
                i64_from_u64(sequence, "sequence")?
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    bytes.as_deref().map(digest_from_blob).transpose()
}

/// Rebuild a [`RecordEnvelope`] from a stored row.
///
/// The outer `Result` carries column-read failures (mapped by the caller with
/// `map_sqlite_error`); the inner one carries integrity failures of the
/// stored bytes.
fn row_to_envelope(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<RecordEnvelope, StoreError>> {
    let session_id = row.get::<_, Vec<u8>>(0)?;
    let sequence = row.get::<_, i64>(1)?;
    let record_id = row.get::<_, Vec<u8>>(2)?;
    let lane_id = row.get::<_, Vec<u8>>(3)?;
    let run_id = row.get::<_, Option<Vec<u8>>>(4)?;
    let kind = row.get::<_, String>(5)?;
    let format_version = row.get::<_, i64>(6)?;
    let kind_version = row.get::<_, i64>(7)?;
    let payload_cbor = row.get::<_, Vec<u8>>(8)?;
    let timestamp = row.get::<_, i64>(9)?;
    let payload_digest = row.get::<_, Vec<u8>>(10)?;
    let previous_checksum = row.get::<_, Option<Vec<u8>>>(11)?;
    let envelope_checksum = row.get::<_, Vec<u8>>(12)?;
    let derived_event_ids = row.get::<_, Vec<u8>>(13)?;
    Ok((|| -> Result<RecordEnvelope, StoreError> {
        let body = decode::<RecordBody>(&payload_cbor).map_err(protocol_error)?;
        if body.kind_name() != kind {
            return Err(StoreError::Integrity {
                reason_code: "sqlite_kind_mismatch",
            });
        }
        let events = decode::<Vec<EventId>>(&derived_event_ids).map_err(protocol_error)?;
        RecordEnvelope::try_new(
            u16_from_i64(format_version, "format_version")?,
            u16_from_i64(kind_version, "kind_version")?,
            id_from_blob(&record_id)?,
            id_from_blob(&session_id)?,
            id_from_blob(&lane_id)?,
            run_id.as_deref().map(id_from_blob).transpose()?,
            u64_from_i64(sequence, "sequence")?,
            Timestamp::from_unix_ms(timestamp).map_err(|_| StoreError::Integrity {
                reason_code: "sqlite_timestamp",
            })?,
            None,
            digest_from_blob(&payload_digest)?,
            previous_checksum
                .as_deref()
                .map(digest_from_blob)
                .transpose()?,
            digest_from_blob(&envelope_checksum)?,
            events,
            body,
        )
        .map_err(|_| StoreError::Integrity {
            reason_code: "sqlite_envelope_invalid",
        })
    })())
}

fn load_session(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
    cached: Option<VerifiedHead>,
) -> Result<LoadedSession, StoreError> {
    let Some(session) = load_session_row(connection, session_id)? else {
        return Ok(LoadedSession::empty(session_id));
    };
    let stored = load_session_records(connection, session_id, 1, None)?;
    let (metadata, snapshot) =
        load_session_extras(connection, session_id, &session, snapshot_bytes)?;
    // The cache is a pure optimization: `verify_head_against_cache` falls
    // back to a full verification whenever the cached anchor is not usable,
    // so the accept/reject decision and its reason code are exactly those of
    // an uncached load.
    let head_checksum = {
        let records = stored
            .iter()
            .map(|row| row.envelope.clone())
            .collect::<Vec<_>>();
        verify_head_against_cache(
            ChainAnchor::try_new(
                session_id,
                session.chain_anchor_sequence,
                session.chain_anchor_checksum,
            )
            .map_err(protocol_error)?,
            &records,
            session.current_sequence,
            session.head_checksum,
            cached,
        )?
    };
    Ok(LoadedSession {
        session_id,
        head_sequence: session.current_sequence,
        head_checksum,
        metadata,
        committed_batches: group_batches(connection, &stored)?.into(),
        accelerated: snapshot.as_ref().and_then(accelerated_from),
        snapshot,
    })
}

pub(crate) fn load_session_window(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
    window: LoadWindow,
    cached: Option<VerifiedHead>,
) -> Result<LoadedSession, StoreError> {
    match window {
        LoadWindow::Full => load_session(connection, session_id, snapshot_bytes, cached),
        LoadWindow::FromSequence {
            from_sequence,
            prior_checksum,
        } => load_session_from_sequence(
            connection,
            session_id,
            snapshot_bytes,
            from_sequence,
            prior_checksum,
        ),
        LoadWindow::SnapshotPlusTail => {
            load_session_snapshot_plus_tail(connection, session_id, snapshot_bytes)
        }
    }
}

fn load_session_from_sequence(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
    from_sequence: u64,
    prior_checksum: Digest,
) -> Result<LoadedSession, StoreError> {
    if from_sequence <= 1 {
        return load_session(connection, session_id, snapshot_bytes, None);
    }
    let Some(session) = load_session_row(connection, session_id)? else {
        return Err(StoreError::Integrity {
            reason_code: "load_from_sequence_gap",
        });
    };
    let stored = load_session_records(connection, session_id, from_sequence, None)?;
    loaded_tail(
        connection,
        session_id,
        snapshot_bytes,
        &session,
        &stored,
        prior_checksum,
        from_sequence,
        FROM_SEQUENCE_WINDOW,
    )
}

fn load_session_snapshot_plus_tail(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
) -> Result<LoadedSession, StoreError> {
    let Some(session) = load_session_row(connection, session_id)? else {
        return Ok(LoadedSession::empty(session_id));
    };
    let Some(snapshot) = load_snapshot(connection, session_id, snapshot_bytes)? else {
        return load_session(connection, session_id, snapshot_bytes, None);
    };
    let accelerated = accelerated_from(&snapshot).ok_or(StoreError::Integrity {
        reason_code: "snapshot_state_invalid",
    })?;
    let start = accelerated.sequence.saturating_add(1);
    let stored = load_session_records(connection, session_id, start, None)?;
    loaded_tail(
        connection,
        session_id,
        snapshot_bytes,
        &session,
        &stored,
        accelerated.head_checksum,
        start,
        SNAPSHOT_WINDOW,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "each argument is a distinct part of the window"
)]
fn loaded_tail(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
    session: &SessionRow,
    stored: &[StoredRecord],
    prior_checksum: Digest,
    start: u64,
    codes: WindowCodes,
) -> Result<LoadedSession, StoreError> {
    let (metadata, snapshot) =
        load_session_extras(connection, session_id, session, snapshot_bytes)?;
    if let Some(first) = stored.first() {
        let prior_batch = lookup_sequence_batch(connection, session_id, start.saturating_sub(1))?;
        check_batch_alignment(first.batch_id, prior_batch, codes)?;
    }
    let records = stored
        .iter()
        .map(|row| row.envelope.clone())
        .collect::<Vec<_>>();
    verify_tail_records(
        session_id,
        &records,
        start,
        prior_checksum,
        session.current_sequence,
        session.head_checksum,
        codes,
    )?;
    Ok(LoadedSession {
        session_id,
        head_sequence: session.current_sequence,
        head_checksum: session.head_checksum,
        metadata,
        committed_batches: group_batches(connection, stored)?.into(),
        accelerated: snapshot.as_ref().and_then(accelerated_from),
        snapshot,
    })
}

fn lookup_sequence_batch(
    connection: &Connection,
    session_id: SessionId,
    sequence: u64,
) -> Result<Option<AppendBatchId>, StoreError> {
    if sequence == 0 {
        return Ok(None);
    }
    let bytes = connection
        .query_row(
            "SELECT batch_id FROM records WHERE session_id = ?1 AND sequence = ?2",
            params![
                session_id.as_bytes().as_slice(),
                i64_from_u64(sequence, "sequence")?
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    bytes.as_deref().map(id_from_blob).transpose()
}

/// The session's metadata, plus its stored snapshot when the row claims one.
fn load_session_extras(
    connection: &Connection,
    session_id: SessionId,
    session: &SessionRow,
    snapshot_bytes: usize,
) -> Result<(Metadata, Option<OpaqueSnapshot>), StoreError> {
    let metadata = connection
        .query_row(
            "SELECT metadata FROM sessions WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(map_sqlite_error)?;
    let metadata = Metadata::parse(metadata).map_err(|_| StoreError::Integrity {
        reason_code: "sqlite_metadata",
    })?;
    let snapshot = if session.snapshot_sequence.is_some() {
        load_snapshot(connection, session_id, snapshot_bytes)?
    } else {
        None
    };
    Ok((metadata, snapshot))
}

pub(crate) fn load_snapshot(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
) -> Result<Option<OpaqueSnapshot>, StoreError> {
    let Some((sequence, payload, digest)) = connection
        .query_row(
            "SELECT sequence, payload_cbor, digest FROM snapshots WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
    else {
        return Ok(None);
    };
    Ok(Some(OpaqueSnapshot::try_new(
        u64_from_i64(sequence, "snapshot_sequence")?,
        digest_from_blob(&digest)?,
        payload,
        snapshot_bytes,
    )?))
}

pub(crate) fn scan_session(
    connection: &Connection,
    request: ScanRequest,
) -> Result<ScanPage, StoreError> {
    validate_scan_limit(request.limit)?;
    let Some(session) = load_session_row(connection, request.session_id)? else {
        return Ok(ScanPage {
            session_id: request.session_id,
            records: Arc::from([]),
            next_sequence: None,
        });
    };
    let start = scan_start(request.from_sequence);
    let fetch_limit = request.limit.saturating_add(1);
    let fetched = load_session_records(connection, request.session_id, start, Some(fetch_limit))?;
    if fetched.is_empty() {
        if start <= session.current_sequence {
            return Err(StoreError::Integrity {
                reason_code: "scan_sequence_gap",
            });
        }
        return Ok(ScanPage {
            session_id: request.session_id,
            records: Arc::from([]),
            next_sequence: None,
        });
    }
    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    let has_more = fetched.len() > limit;
    let records = fetched
        .iter()
        .take(limit)
        .map(|row| row.envelope.clone())
        .collect::<Vec<_>>();
    if records.first().is_some_and(|record| {
        record.sequence() != start
            && !session
                .snapshot_sequence
                .is_some_and(|boundary| start < boundary && record.sequence() == boundary)
    }) {
        return Err(StoreError::Integrity {
            reason_code: "scan_sequence_gap",
        });
    }
    verify_scan_page(connection, request.session_id, &records, &session)?;
    let next_sequence = scan_next_sequence(&records, has_more);
    Ok(ScanPage {
        session_id: request.session_id,
        records: records.into(),
        next_sequence,
    })
}

fn verify_scan_page(
    connection: &Connection,
    session_id: SessionId,
    records: &[RecordEnvelope],
    session: &SessionRow,
) -> Result<(), StoreError> {
    let Some(first) = records.first() else {
        return Ok(());
    };
    let prior = if first.sequence() <= 1 {
        None
    } else {
        let checkpoint = first.sequence().saturating_sub(1);
        if let Some(stored) = load_envelope_checksum(connection, session_id, checkpoint)? {
            if first.previous_checksum() != Some(stored) {
                return Err(StoreError::Integrity {
                    reason_code: "scan_checkpoint_mismatch",
                });
            }
            Some(stored)
        } else {
            if session
                .snapshot_sequence
                .is_none_or(|boundary| checkpoint >= boundary)
            {
                return Err(StoreError::Integrity {
                    reason_code: "scan_checkpoint_mismatch",
                });
            }
            first.previous_checksum()
        }
    };
    let anchor =
        ChainAnchor::try_new(session_id, first.sequence(), prior).map_err(protocol_error)?;
    let head = verify_chain_from(records, anchor).map_err(protocol_error)?;
    if records
        .last()
        .is_some_and(|record| record.sequence() == session.current_sequence)
        && head != session.head_checksum
    {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    Ok(())
}

fn group_batches(
    connection: &Connection,
    stored: &[StoredRecord],
) -> Result<Vec<CommittedBatch>, StoreError> {
    let mut batches = Vec::new();
    for run in stored.chunk_by(|left, right| left.batch_id == right.batch_id) {
        let Some(first) = run.first() else {
            continue;
        };
        let batch_id = first.batch_id;
        let first_sequence = first.envelope.sequence();
        let last_sequence = run
            .last()
            .map_or(first_sequence, |row| row.envelope.sequence());
        let (declared_first, declared_last) = connection
            .query_row(
                "SELECT first_sequence, last_sequence FROM batches WHERE batch_id = ?1",
                params![batch_id.as_bytes().as_slice()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(map_sqlite_error)?;
        let declared_first = u64_from_i64(declared_first, "first_sequence")?;
        let declared_last = u64_from_i64(declared_last, "last_sequence")?;
        if first_sequence > declared_first && last_sequence == declared_last {
            continue;
        }
        if first_sequence != declared_first || last_sequence != declared_last {
            return Err(StoreError::Integrity {
                reason_code: "committed_batch_invalid",
            });
        }
        let records = run.iter().map(|row| row.envelope.clone()).collect();
        batches.push(
            CommittedBatch::try_new(batch_id, declared_first, declared_last, records).map_err(
                |_| StoreError::Integrity {
                    reason_code: "committed_batch_invalid",
                },
            )?,
        );
    }
    Ok(batches)
}

pub(crate) fn load_session_row(
    connection: &Connection,
    session_id: SessionId,
) -> Result<Option<SessionRow>, StoreError> {
    let Some((
        current_sequence,
        head_checksum,
        snapshot_sequence,
        chain_anchor_sequence,
        chain_anchor_checksum,
    )) = connection
        .query_row(
            "SELECT current_sequence, head_checksum, snapshot_sequence,
                    chain_anchor_sequence, chain_anchor_checksum
             FROM sessions WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
    else {
        return Ok(None);
    };
    Ok(Some(SessionRow {
        current_sequence: u64_from_i64(current_sequence, "current_sequence")?,
        head_checksum: head_checksum.as_deref().map(digest_from_blob).transpose()?,
        snapshot_sequence: snapshot_sequence
            .map(|value| u64_from_i64(value, "snapshot_sequence"))
            .transpose()?,
        chain_anchor_sequence: u64_from_i64(chain_anchor_sequence, "chain_anchor_sequence")?,
        chain_anchor_checksum: chain_anchor_checksum
            .as_deref()
            .map(digest_from_blob)
            .transpose()?,
    }))
}

/// The committed footprint of `session_id`, for append admission.
pub(crate) fn session_usage(
    connection: &Connection,
    session_id: SessionId,
) -> Result<SessionUsage, StoreError> {
    Ok(SessionUsage {
        batches: count_where(
            connection,
            "SELECT COUNT(*) FROM batches WHERE session_id = ?1",
            session_id,
        )?,
        records: count_where(
            connection,
            "SELECT COUNT(*) FROM records WHERE session_id = ?1",
            session_id,
        )?,
    })
}

pub(crate) fn count_sessions(connection: &Connection) -> Result<usize, StoreError> {
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .map_err(map_sqlite_error)?;
    usize_from_i64(count, "sessions")
}

fn count_where(
    connection: &Connection,
    sql: &str,
    session_id: SessionId,
) -> Result<usize, StoreError> {
    let count: i64 = connection
        .query_row(sql, params![session_id.as_bytes().as_slice()], |row| {
            row.get(0)
        })
        .map_err(map_sqlite_error)?;
    usize_from_i64(count, "count")
}

pub(crate) fn lookup_record_batch(
    connection: &Connection,
    record_id: RecordId,
) -> Result<Option<AppendBatchId>, StoreError> {
    let bytes = connection
        .query_row(
            "SELECT batch_id FROM records WHERE record_id = ?1",
            params![record_id.as_bytes().as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    bytes.as_deref().map(id_from_blob).transpose()
}

fn id_from_blob<T: IdTag>(bytes: &[u8]) -> Result<Id<T>, StoreError> {
    let value: [u8; 16] = bytes.try_into().map_err(|_| StoreError::Integrity {
        reason_code: "sqlite_id_width",
    })?;
    Ok(Id::from_bytes(value))
}

const DIGEST_HEX: &[u8; 16] = b"0123456789abcdef";

fn digest_from_blob(bytes: &[u8]) -> Result<Digest, StoreError> {
    if bytes.len() != 32 {
        return Err(StoreError::Integrity {
            reason_code: "sqlite_digest_width",
        });
    }
    let mut hex = [0_u8; 64];
    for (index, byte) in bytes.iter().enumerate() {
        hex[index * 2] = DIGEST_HEX[usize::from(byte >> 4)];
        hex[index * 2 + 1] = DIGEST_HEX[usize::from(byte & 0x0f)];
    }
    let Ok(hex) = core::str::from_utf8(&hex) else {
        return Err(StoreError::Integrity {
            reason_code: "sqlite_digest_hex",
        });
    };
    Digest::from_hex(hex).map_err(|_| StoreError::Integrity {
        reason_code: "sqlite_digest_hex",
    })
}
