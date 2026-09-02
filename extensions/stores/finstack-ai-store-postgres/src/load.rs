//! Verified loads: full journals, the `VerifiedHead` suffix cache, and the
//! `load_from` windows (spec D9).
//!
//! ## Where the semantics live
//!
//! As in [`crate::append`], none of the store-contract *decisions* are made
//! here. Chain verification, tail verification, batch alignment and the
//! window reason codes are all [`finstack_ai_store_common`] functions
//! ([`verify_full_head`], [`verify_tail_records`], [`check_batch_alignment`],
//! [`FROM_SEQUENCE_WINDOW`]/[`SNAPSHOT_WINDOW`]), called in the same order
//! sqlite calls them in
//! `extensions/stores/finstack-ai-store-sqlite/src/load.rs`. This module only
//! supplies the SQL, the row → [`RecordEnvelope`] rehydration, and the batch
//! regrouping.
//!
//! ## Snapshot consistency
//!
//! One load issues several statements (session row, records, snapshot,
//! alignment probe). Under `READ COMMITTED` each of those takes its own
//! snapshot, so a *concurrent* append landing between two of them would make
//! the records disagree with the session head and fail verification
//! spuriously. Every load therefore runs inside one `READ ONLY REPEATABLE
//! READ` transaction: all statements observe one instant of the journal, and
//! an `Integrity` result means the stored data really is inconsistent.
//!
//! ## The verified-head cache
//!
//! [`VerifiedHead`] records that this process already verified a session's
//! chain through some sequence. Because the journal is append-only, that
//! proof stays valid no matter how many other writers extend the chain, so a
//! later load only has to verify the records *after* the cached head. Both
//! the decision (suffix versus full verification) and the cache container
//! live in [`finstack_ai_store_common`] — see
//! [`verify_head_against_cache`] and
//! [`finstack_ai_store_common::VerifiedHeadCache`] — so all three backends
//! share one mechanism. The cache is never allowed to change an outcome: any
//! suffix verification failure falls back to a full verification, which owns
//! the reason codes. This crate keeps only the call-site policy: the store
//! drops the entry on any `Integrity` result and overwrites it after every
//! successful *full* load (spec D9).
//!
//! ## Connection disposition
//!
//! Identical to append: errors carry a [`Failure`] poison flag, and a
//! connection that saw a wire-level failure (or a rollback that could not be
//! delivered) is discarded rather than returned to the pool (spec D2).

use finstack_ai_kernel::{
    AppendBatchId, CommittedBatch, Digest, EventId, Id, IdTag, Metadata, RecordBody,
    RecordEnvelope, SessionId, Timestamp,
};
use finstack_ai_protocol::{ChainAnchor, decode};
use finstack_ai_runtime::ports::journal::{LoadWindow, LoadedSession, OpaqueSnapshot, StoreError};
use finstack_ai_store_common::{
    AppendIdentity, FROM_SEQUENCE_WINDOW, SNAPSHOT_WINDOW, VerifiedHead, WindowCodes,
    accelerated_from, check_batch_alignment, protocol_error, verify_head_against_cache,
    verify_tail_records,
};
use tokio_postgres::{Client, Statement, Transaction};

use crate::error::{Failure, i64_from_u64, read_op, u64_from_i64};
use crate::pool::PooledClient;

/// Read the session row.
pub(crate) const SELECT_SESSION_ROW: &str = "SELECT current_sequence, head_checksum, \
     snapshot_sequence, metadata, chain_anchor_sequence, chain_anchor_checksum \
     FROM sessions WHERE session_id = $1";

/// Read every record of a session at or after a sequence, with its batch id.
const SELECT_RECORDS_FROM: &str = concat!(
    "SELECT ",
    record_columns!(),
    ", batch_id FROM records WHERE session_id = $1 AND sequence >= $2 ORDER BY sequence"
);

/// Read one record's stored envelope checksum.
pub(crate) const SELECT_ENVELOPE_CHECKSUM: &str =
    "SELECT envelope_checksum FROM records WHERE session_id = $1 AND sequence = $2";

/// Read the batch id that committed one sequence.
const SELECT_SEQUENCE_BATCH: &str =
    "SELECT batch_id FROM records WHERE session_id = $1 AND sequence = $2";

/// Read a session's stored snapshot row.
pub(crate) const SELECT_SNAPSHOT: &str =
    "SELECT sequence, payload_cbor, digest FROM snapshots WHERE session_id = $1";

/// Read one committed batch's header.
pub(crate) const SELECT_BATCH: &str =
    "SELECT request_cbor, first_sequence, last_sequence FROM batches WHERE batch_id = $1";

/// Read the records of one committed batch, in sequence order.
pub(crate) const SELECT_BATCH_RECORDS: &str = concat!(
    "SELECT ",
    record_columns!(),
    " FROM records WHERE batch_id = $1 ORDER BY sequence"
);

/// The statements one [`load`] prepares up front, before opening its
/// transaction (see [`PooledClient::prepared`] for why that ordering is
/// forced).
///
/// All four are prepared even though `sequence_batch` is only used by the
/// windowed loads: they are prepared once per physical connection, and
/// splitting the bundle per window would buy one saved `PREPARE` at the cost
/// of two more code paths.
pub(crate) struct LoadStatements {
    /// [`SELECT_SESSION_ROW`].
    session_row: Statement,
    /// [`SELECT_RECORDS_FROM`].
    records_from: Statement,
    /// [`SELECT_SNAPSHOT`].
    snapshot: Statement,
    /// [`SELECT_SEQUENCE_BATCH`].
    sequence_batch: Statement,
}

impl LoadStatements {
    /// Prepare (or reuse) every statement the load path needs.
    async fn prepare(client: &mut PooledClient<Client>) -> Result<Self, Failure> {
        Ok(Self {
            session_row: prepare(client, SELECT_SESSION_ROW).await?,
            records_from: prepare(client, SELECT_RECORDS_FROM).await?,
            snapshot: prepare(client, SELECT_SNAPSHOT).await?,
            sequence_batch: prepare(client, SELECT_SEQUENCE_BATCH).await?,
        })
    }
}

/// Fetch one cached statement, classifying a `PREPARE` failure exactly as
/// any other statement failure on the connection.
pub(crate) async fn prepare(
    client: &mut PooledClient<Client>,
    sql: &'static str,
) -> Result<Statement, Failure> {
    client
        .prepared(sql)
        .await
        .map_err(|error| Failure::from_driver(&error))
}

/// A stored record plus the batch it was committed in.
struct StoredRecord {
    /// Batch that committed this record, used to rebuild batch boundaries.
    batch_id: AppendBatchId,
    /// The rehydrated envelope.
    envelope: RecordEnvelope,
}

/// The session row as the load path (and [`crate::snapshot::scan`]) needs it.
pub(crate) struct SessionRow {
    /// Sequence of the journal head (0 for a session with no records).
    pub(crate) current_sequence: u64,
    /// Checksum of the head record, `None` before the first append.
    pub(crate) head_checksum: Option<Digest>,
    /// Sequence covered by the stored snapshot, when one exists.
    ///
    /// Doubles as the *prune boundary*: [`crate::prune::prune`] deletes
    /// records with `sequence < snapshot_sequence`, so a record missing
    /// below this value may have been legitimately pruned, while a record
    /// missing at or above it is a hole in the journal. See
    /// [`crate::snapshot::scan`], which is the only reader that needs the
    /// distinction.
    pub(crate) snapshot_sequence: Option<u64>,
    /// Next retained sequence anchored by trusted prune metadata.
    pub(crate) chain_anchor_sequence: u64,
    /// Trusted checksum immediately before `chain_anchor_sequence`.
    pub(crate) chain_anchor_checksum: Option<Digest>,
    /// Session metadata. Never grants authority.
    metadata: Metadata,
}

/// A previously committed batch, rehydrated for an idempotent replay.
pub(crate) struct LoadedBatch {
    /// Decoded identity, used for idempotent replay comparison.
    pub(crate) identity: AppendIdentity,
    /// The batch as it was returned when first committed.
    ///
    /// `None` when remaining records no longer cover `first_sequence`
    /// (a snapshot-aligned prefix prune deleted the start of the batch).
    pub(crate) committed: Option<CommittedBatch>,
}

/// Load `window` of `session_id` on `client`, verifying the chain (spec D9).
///
/// The caller keeps ownership of the checkout; a connection that must not be
/// reused is marked with [`PooledClient::poison`] here rather than consumed,
/// exactly as [`crate::append::append`] does.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when the stored chain does not verify
/// (the store-common chain codes, or the window's gap/split/checksum codes),
/// and otherwise the mapped driver error.
pub(crate) async fn load(
    client: &mut PooledClient<Client>,
    session_id: SessionId,
    snapshot_bytes: usize,
    window: LoadWindow,
    cached: Option<VerifiedHead>,
) -> Result<LoadedSession, StoreError> {
    read_op(
        client,
        LoadStatements::prepare,
        async |transaction, statements| {
            load_session_window(
                transaction,
                statements,
                session_id,
                snapshot_bytes,
                window,
                cached,
            )
            .await
        },
    )
    .await
}

/// Dispatch on the requested window (mirrors sqlite's `load_session_window`).
async fn load_session_window(
    transaction: &Transaction<'_>,
    statements: &LoadStatements,
    session_id: SessionId,
    snapshot_bytes: usize,
    window: LoadWindow,
    cached: Option<VerifiedHead>,
) -> Result<LoadedSession, Failure> {
    match window {
        LoadWindow::Full => {
            load_session(transaction, statements, session_id, snapshot_bytes, cached).await
        }
        LoadWindow::FromSequence {
            from_sequence,
            prior_checksum,
        } => {
            load_session_from_sequence(
                transaction,
                statements,
                session_id,
                snapshot_bytes,
                from_sequence,
                prior_checksum,
            )
            .await
        }
        LoadWindow::SnapshotPlusTail => {
            load_session_snapshot_plus_tail(transaction, statements, session_id, snapshot_bytes)
                .await
        }
    }
}

/// Load and verify a whole session journal.
async fn load_session(
    transaction: &Transaction<'_>,
    statements: &LoadStatements,
    session_id: SessionId,
    snapshot_bytes: usize,
    cached: Option<VerifiedHead>,
) -> Result<LoadedSession, Failure> {
    let Some(session) = load_session_row(transaction, &statements.session_row, session_id).await?
    else {
        return Ok(LoadedSession::empty(session_id));
    };
    let stored = load_records_from(transaction, &statements.records_from, session_id, 1).await?;
    // The verification copy is scoped so it is dropped before
    // `loaded_session` moves the originals out of `stored`: store-common's
    // verification takes `&[RecordEnvelope]`, so one owned copy is
    // unavoidable, but only one is ever alive at a time.
    let head_checksum = {
        let records = envelopes(&stored);
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
    let snapshot = load_session_snapshot(
        transaction,
        &statements.snapshot,
        session_id,
        &session,
        snapshot_bytes,
    )
    .await?;
    Ok(loaded_session(
        session_id,
        &session,
        head_checksum,
        stored,
        snapshot,
    )?)
}

/// Load the batch-aligned tail from `from_sequence` (spec D9,
/// [`LoadWindow::FromSequence`]).
async fn load_session_from_sequence(
    transaction: &Transaction<'_>,
    statements: &LoadStatements,
    session_id: SessionId,
    snapshot_bytes: usize,
    from_sequence: u64,
    prior_checksum: Digest,
) -> Result<LoadedSession, Failure> {
    if from_sequence <= 1 {
        // The window covers the whole journal: no prefix is omitted, so
        // there is nothing to chain from and this is a plain full load. The
        // cache is deliberately not consulted — the caller asked for a
        // window, and this path is not on the hot restore loop.
        return load_session(transaction, statements, session_id, snapshot_bytes, None).await;
    }
    let Some(session) = load_session_row(transaction, &statements.session_row, session_id).await?
    else {
        return Err(StoreError::Integrity {
            reason_code: FROM_SEQUENCE_WINDOW.gap,
        }
        .into());
    };
    let stored = load_records_from(
        transaction,
        &statements.records_from,
        session_id,
        from_sequence,
    )
    .await?;
    loaded_tail(
        transaction,
        statements,
        session_id,
        snapshot_bytes,
        &session,
        stored,
        prior_checksum,
        from_sequence,
        FROM_SEQUENCE_WINDOW,
    )
    .await
}

/// Load the snapshot plus the tail after it, falling back to a full load when
/// the session has no snapshot ([`LoadWindow::SnapshotPlusTail`]).
async fn load_session_snapshot_plus_tail(
    transaction: &Transaction<'_>,
    statements: &LoadStatements,
    session_id: SessionId,
    snapshot_bytes: usize,
) -> Result<LoadedSession, Failure> {
    let Some(session) = load_session_row(transaction, &statements.session_row, session_id).await?
    else {
        return Ok(LoadedSession::empty(session_id));
    };
    let Some(snapshot) = load_snapshot(
        transaction,
        &statements.snapshot,
        session_id,
        snapshot_bytes,
    )
    .await?
    else {
        return load_session(transaction, statements, session_id, snapshot_bytes, None).await;
    };
    let accelerated = accelerated_from(&snapshot).ok_or(StoreError::Integrity {
        reason_code: "snapshot_state_invalid",
    })?;
    let start = accelerated.sequence.saturating_add(1);
    let stored =
        load_records_from(transaction, &statements.records_from, session_id, start).await?;
    loaded_tail(
        transaction,
        statements,
        session_id,
        snapshot_bytes,
        &session,
        stored,
        accelerated.head_checksum,
        start,
        SNAPSHOT_WINDOW,
    )
    .await
}

/// Verify a tail window and assemble its [`LoadedSession`].
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors sqlite's `loaded_tail`; each argument is a distinct part of the window"
)]
async fn loaded_tail(
    transaction: &Transaction<'_>,
    statements: &LoadStatements,
    session_id: SessionId,
    snapshot_bytes: usize,
    session: &SessionRow,
    stored: Vec<StoredRecord>,
    prior_checksum: Digest,
    start: u64,
    codes: WindowCodes,
) -> Result<LoadedSession, Failure> {
    if let Some(first) = stored.first() {
        let prior_batch = lookup_sequence_batch(
            transaction,
            &statements.sequence_batch,
            session_id,
            start.saturating_sub(1),
        )
        .await?;
        check_batch_alignment(first.batch_id, prior_batch, codes)?;
    }
    // Same ownership rule as `load_session`: the verification copy lives in
    // its own scope and is dropped before `loaded_session` moves the
    // originals.
    {
        let records = envelopes(&stored);
        verify_tail_records(
            session_id,
            &records,
            start,
            prior_checksum,
            session.current_sequence,
            session.head_checksum,
            codes,
        )?;
    }
    let snapshot = load_session_snapshot(
        transaction,
        &statements.snapshot,
        session_id,
        session,
        snapshot_bytes,
    )
    .await?;
    Ok(loaded_session(
        session_id,
        session,
        session.head_checksum,
        stored,
        snapshot,
    )?)
}

/// Assemble the port's [`LoadedSession`] from verified rows.
fn loaded_session(
    session_id: SessionId,
    session: &SessionRow,
    head_checksum: Option<Digest>,
    stored: Vec<StoredRecord>,
    snapshot: Option<OpaqueSnapshot>,
) -> Result<LoadedSession, StoreError> {
    Ok(LoadedSession {
        session_id,
        head_sequence: session.current_sequence,
        head_checksum,
        metadata: session.metadata.clone(),
        committed_batches: group_batches(stored)?.into(),
        accelerated: snapshot.as_ref().and_then(accelerated_from),
        snapshot,
    })
}

/// Rebuild the stored batch boundaries from a contiguous run of records.
///
/// Takes the rows by value and *moves* each envelope into the batch it
/// belongs to: the envelopes are already owned by the caller's `Vec` and
/// nothing needs them afterwards, so cloning them here would double the
/// per-record cost of every load.
fn group_batches(stored: Vec<StoredRecord>) -> Result<Vec<CommittedBatch>, StoreError> {
    let mut batches = Vec::new();
    // The batch currently being accumulated: its id, its first sequence, and
    // the envelopes moved into it so far.
    let mut current: Option<(AppendBatchId, u64, Vec<RecordEnvelope>)> = None;
    for row in stored {
        let sequence = row.envelope.sequence();
        let continues = current
            .as_ref()
            .is_some_and(|(batch_id, _, _)| *batch_id == row.batch_id);
        if !continues && let Some((batch_id, first_sequence, records)) = current.take() {
            batches.push(committed_batch(batch_id, first_sequence, records)?);
        }
        if let Some((_, _, records)) = current.as_mut() {
            records.push(row.envelope);
        } else {
            current = Some((row.batch_id, sequence, vec![row.envelope]));
        }
    }
    if let Some((batch_id, first_sequence, records)) = current {
        batches.push(committed_batch(batch_id, first_sequence, records)?);
    }
    Ok(batches)
}

/// Assemble one rebuilt [`CommittedBatch`] from the records grouped into it.
fn committed_batch(
    batch_id: AppendBatchId,
    first_sequence: u64,
    records: Vec<RecordEnvelope>,
) -> Result<CommittedBatch, StoreError> {
    let last_sequence = records
        .last()
        .map_or(first_sequence, RecordEnvelope::sequence);
    CommittedBatch::try_new(batch_id, first_sequence, last_sequence, records).map_err(|_| {
        StoreError::Integrity {
            reason_code: "committed_batch_invalid",
        }
    })
}

/// Clone the envelopes out of their stored rows, for the verification
/// helpers in [`finstack_ai_store_common`] — which take
/// `&[RecordEnvelope]`, so a contiguous owned slice has to exist somewhere.
///
/// Every caller scopes the result so it is dropped before [`group_batches`]
/// moves the originals: one copy per record is alive at a time, never two.
fn envelopes(stored: &[StoredRecord]) -> Vec<RecordEnvelope> {
    stored
        .iter()
        .map(|row| row.envelope.clone())
        .collect::<Vec<_>>()
}

/// Read the session row, or `None` when the session does not exist.
pub(crate) async fn load_session_row(
    transaction: &Transaction<'_>,
    statement: &Statement,
    session_id: SessionId,
) -> Result<Option<SessionRow>, Failure> {
    let row = transaction
        .query_opt(statement, &[&session_id.as_bytes().as_slice()])
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let head_checksum: Option<Vec<u8>> = row.get(1);
    let snapshot_sequence: Option<i64> = row.get(2);
    let metadata: Vec<u8> = row.get(3);
    let chain_anchor_checksum: Option<Vec<u8>> = row.get(5);
    Ok(Some(SessionRow {
        current_sequence: u64_from_i64(row.get(0), "current_sequence")?,
        head_checksum: head_checksum
            .as_deref()
            .map(digest_from_bytes)
            .transpose()?,
        snapshot_sequence: snapshot_sequence
            .map(|value| u64_from_i64(value, "snapshot_sequence"))
            .transpose()?,
        chain_anchor_sequence: u64_from_i64(row.get(4), "chain_anchor_sequence")?,
        chain_anchor_checksum: chain_anchor_checksum
            .as_deref()
            .map(digest_from_bytes)
            .transpose()?,
        metadata: Metadata::parse(metadata).map_err(|_| StoreError::Integrity {
            reason_code: "postgres_metadata",
        })?,
    }))
}

/// Read the envelope checksum stored for `sequence` in `session_id`, or
/// `None` when no such record exists.
///
/// Used by [`crate::snapshot::scan`] to anchor a scan page against the
/// record immediately before it, mirroring sqlite's
/// `load_envelope_checksum`.
pub(crate) async fn load_envelope_checksum(
    transaction: &Transaction<'_>,
    statement: &Statement,
    session_id: SessionId,
    sequence: u64,
) -> Result<Option<Digest>, Failure> {
    let row = transaction
        .query_opt(
            statement,
            &[
                &session_id.as_bytes().as_slice(),
                &i64_from_u64(sequence, "sequence")?,
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let bytes: Vec<u8> = row.get(0);
    Ok(Some(digest_from_bytes(&bytes)?))
}

/// Read every record of `session_id` at or after `from_sequence`, in sequence
/// order, with the batch id each was committed in.
async fn load_records_from(
    transaction: &Transaction<'_>,
    statement: &Statement,
    session_id: SessionId,
    from_sequence: u64,
) -> Result<Vec<StoredRecord>, Failure> {
    let rows = transaction
        .query(
            statement,
            &[
                &session_id.as_bytes().as_slice(),
                &i64_from_u64(from_sequence, "from_sequence")?,
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let mut stored = Vec::with_capacity(rows.len());
    for row in &rows {
        let batch_id: Vec<u8> = row.get(14);
        stored.push(StoredRecord {
            batch_id: id_from_bytes(&batch_id)?,
            envelope: reconstruct_envelope(row)?,
        });
    }
    Ok(stored)
}

/// The batch holding `sequence`, or `None` for sequence 0 / a missing record.
async fn lookup_sequence_batch(
    transaction: &Transaction<'_>,
    statement: &Statement,
    session_id: SessionId,
    sequence: u64,
) -> Result<Option<AppendBatchId>, Failure> {
    if sequence == 0 {
        return Ok(None);
    }
    let row = transaction
        .query_opt(
            statement,
            &[
                &session_id.as_bytes().as_slice(),
                &i64_from_u64(sequence, "sequence")?,
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let batch_id: Vec<u8> = row.get(0);
    Ok(Some(id_from_bytes(&batch_id)?))
}

/// Read the stored snapshot only when the session row claims one, mirroring
/// sqlite's `load_session_extras`.
async fn load_session_snapshot(
    transaction: &Transaction<'_>,
    statement: &Statement,
    session_id: SessionId,
    session: &SessionRow,
    snapshot_bytes: usize,
) -> Result<Option<OpaqueSnapshot>, Failure> {
    if session.snapshot_sequence.is_none() {
        return Ok(None);
    }
    load_snapshot(transaction, statement, session_id, snapshot_bytes).await
}

/// Read the stored snapshot row, if any.
///
/// Reused by [`crate::prune::prune`], which needs the same row inside its own
/// (write) transaction to admit and decode the prune-covering snapshot.
pub(crate) async fn load_snapshot(
    transaction: &Transaction<'_>,
    statement: &Statement,
    session_id: SessionId,
    snapshot_bytes: usize,
) -> Result<Option<OpaqueSnapshot>, Failure> {
    let row = transaction
        .query_opt(statement, &[&session_id.as_bytes().as_slice()])
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let payload: Vec<u8> = row.get(1);
    let digest: Vec<u8> = row.get(2);
    Ok(Some(OpaqueSnapshot::try_new(
        u64_from_i64(row.get(0), "snapshot_sequence")?,
        digest_from_bytes(&digest)?,
        payload,
        snapshot_bytes,
    )?))
}

/// Load a committed batch by id, with its records in sequence order.
pub(crate) async fn load_batch(
    transaction: &Transaction<'_>,
    header: &Statement,
    records: &Statement,
    batch_id: AppendBatchId,
) -> Result<Option<LoadedBatch>, Failure> {
    let row = transaction
        .query_opt(header, &[&batch_id.as_bytes().as_slice()])
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let request_cbor: Vec<u8> = row.get(0);
    let first_sequence = u64_from_i64(row.get(1), "first_sequence")?;
    let last_sequence = u64_from_i64(row.get(2), "last_sequence")?;
    let identity = decode::<AppendIdentity>(&request_cbor).map_err(protocol_error)?;

    let record_rows = transaction
        .query(records, &[&batch_id.as_bytes().as_slice()])
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let envelopes = record_rows
        .iter()
        .map(reconstruct_envelope)
        .collect::<Result<Vec<_>, _>>()?;

    let committed = if envelopes
        .first()
        .is_some_and(|record| record.sequence() > first_sequence)
        && envelopes
            .last()
            .is_some_and(|record| record.sequence() == last_sequence)
    {
        None
    } else {
        Some(
            CommittedBatch::try_new(batch_id, first_sequence, last_sequence, envelopes).map_err(
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

/// Rebuild a [`RecordEnvelope`] from a stored row.
///
/// Column order must match `record_columns!`.
pub(crate) fn reconstruct_envelope(
    row: &tokio_postgres::Row,
) -> Result<RecordEnvelope, StoreError> {
    let session_id: Vec<u8> = row.get(0);
    let sequence: i64 = row.get(1);
    let record_id: Vec<u8> = row.get(2);
    let lane_id: Vec<u8> = row.get(3);
    let run_id: Option<Vec<u8>> = row.get(4);
    let kind: String = row.get(5);
    let format_version: i32 = row.get(6);
    let kind_version: i32 = row.get(7);
    let payload_cbor: Vec<u8> = row.get(8);
    let timestamp: i64 = row.get(9);
    let payload_digest: Vec<u8> = row.get(10);
    let previous_checksum: Option<Vec<u8>> = row.get(11);
    let envelope_checksum: Vec<u8> = row.get(12);
    let derived_event_ids: Vec<u8> = row.get(13);

    let body = decode::<RecordBody>(&payload_cbor).map_err(protocol_error)?;
    if body.kind_name() != kind {
        return Err(StoreError::Integrity {
            reason_code: "postgres_kind_mismatch",
        });
    }
    let events = decode::<Vec<EventId>>(&derived_event_ids).map_err(protocol_error)?;
    RecordEnvelope::try_new(
        u16_from_i32(format_version, "format_version")?,
        u16_from_i32(kind_version, "kind_version")?,
        id_from_bytes(&record_id)?,
        id_from_bytes(&session_id)?,
        id_from_bytes(&lane_id)?,
        run_id.as_deref().map(id_from_bytes).transpose()?,
        u64_from_i64(sequence, "sequence")?,
        Timestamp::from_unix_ms(timestamp).map_err(|_| StoreError::Integrity {
            reason_code: "postgres_timestamp",
        })?,
        None,
        digest_from_bytes(&payload_digest)?,
        previous_checksum
            .as_deref()
            .map(digest_from_bytes)
            .transpose()?,
        digest_from_bytes(&envelope_checksum)?,
        events,
        body,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "postgres_envelope_invalid",
    })
}

/// Convert a stored `INTEGER` version column back to `u16`.
fn u16_from_i32(value: i32, reason_code: &'static str) -> Result<u16, StoreError> {
    u16::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

/// Rebuild a typed id from its stored 16-byte representation.
pub(crate) fn id_from_bytes<T: IdTag>(bytes: &[u8]) -> Result<Id<T>, StoreError> {
    let value: [u8; 16] = bytes.try_into().map_err(|_| StoreError::Integrity {
        reason_code: "postgres_id_width",
    })?;
    Ok(Id::from_bytes(value))
}

/// Hex alphabet used to rebuild a [`Digest`] from its stored bytes.
const DIGEST_HEX: &[u8; 16] = b"0123456789abcdef";

/// Rebuild a [`Digest`] from its stored 32-byte representation.
///
/// [`Digest`] is constructed from hex (it has no from-bytes constructor), so
/// the stored bytes are re-hexed here — the same round trip sqlite's
/// `digest_from_blob` performs.
pub(crate) fn digest_from_bytes(bytes: &[u8]) -> Result<Digest, StoreError> {
    if bytes.len() != 32 {
        return Err(StoreError::Integrity {
            reason_code: "postgres_digest_width",
        });
    }
    let mut hex = [0_u8; 64];
    for (index, byte) in bytes.iter().enumerate() {
        hex[index * 2] = DIGEST_HEX[usize::from(byte >> 4)];
        hex[index * 2 + 1] = DIGEST_HEX[usize::from(byte & 0x0f)];
    }
    let Ok(hex) = core::str::from_utf8(&hex) else {
        return Err(StoreError::Integrity {
            reason_code: "postgres_digest_hex",
        });
    };
    Digest::from_hex(hex).map_err(|_| StoreError::Integrity {
        reason_code: "postgres_digest_hex",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_round_trips_through_its_stored_bytes() {
        let digest = Digest::raw_json(b"{}");
        let restored = digest_from_bytes(digest.as_bytes()).expect("round trip");
        assert_eq!(restored, digest);
    }

    #[test]
    fn digest_rejects_a_wrong_width_column() {
        let error = digest_from_bytes(&[0_u8; 16]).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Integrity {
                reason_code: "postgres_digest_width"
            }
        ));
    }

    #[test]
    fn id_rejects_a_wrong_width_column() {
        let error = id_from_bytes::<finstack_ai_kernel::SessionTag>(&[0_u8; 8]).unwrap_err();
        assert!(matches!(
            error,
            StoreError::Integrity {
                reason_code: "postgres_id_width"
            }
        ));
    }
}
