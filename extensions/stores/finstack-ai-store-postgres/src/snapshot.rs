//! Snapshot writes, `scan`, and the `write_metadata` CAS.
//!
//! ## Where the semantics live
//!
//! As in [`crate::append`] and [`crate::load`], the store-contract
//! *decisions* live in [`finstack_ai_store_common`]: byte-ceiling
//! enforcement ([`check_snapshot_size`]), snapshot-sequence admission
//! ([`admit_snapshot_sequence`], covering both `snapshot_ahead_of_journal`
//! and `snapshot_sequence_regression`), state-snapshot encoding
//! ([`encode_state_request`]), and scan validation/paging
//! ([`validate_scan_limit`], [`scan_start`], [`scan_next_sequence`]). This
//! module supplies the SQL and transaction plumbing, called in the same
//! order sqlite's `WorkerCtx::write_snapshot`/`write_state_snapshot`/`scan`/
//! `write_metadata` call them in
//! `extensions/stores/finstack-ai-store-sqlite/src/store.rs` (and
//! `src/load.rs::scan_session` for scan), so the two backends cannot drift
//! in reason code or check ordering. `write_metadata`'s CAS-mismatch reason
//! code, `metadata_cas_mismatch`, is lifted verbatim from sqlite's
//! `WorkerCtx::write_metadata`.
//!
//! ## Locking
//!
//! `write_snapshot` and `write_metadata` both take the session row's
//! `FOR UPDATE` lock via [`crate::session::lock_session`] — the same lock
//! [`crate::append::append`] takes — so a snapshot write, a metadata write,
//! and an append to one session serialize against each other exactly as two
//! appends do. `scan` takes no write lock: it is a read-only range read over
//! `records`, run inside a `READ ONLY REPEATABLE READ` transaction so the
//! page it returns — and every anchoring row `verify_scan_page` reads
//! alongside it (the record before the page and the session row) — reflects
//! one consistent instant of the journal, mirroring [`crate::load::load`]'s
//! isolation. `scan` chain-verifies the page it returns with sqlite-exact
//! semantics; see `verify_scan_page` below.
//!
//! ## Connection disposition
//!
//! Identical to append and load: every error carries a [`Failure`] poison
//! flag, and a connection that saw a wire-level failure (or a rollback that
//! could not be delivered) is discarded rather than returned to the pool
//! (spec D2). A `COMMIT` that fails without a SQLSTATE is the spec D5
//! ambiguous acknowledgement for `write_snapshot`/`write_metadata`, exactly
//! as for append.

use std::sync::Arc;

use finstack_ai_kernel::{Digest, RecordEnvelope, SessionId};
use finstack_ai_protocol::verify_chain_from;
use finstack_ai_runtime::{
    MetadataReceipt, ScanPage, ScanRequest, SnapshotReceipt, SnapshotRequest, StateSnapshotRequest,
    StoreError, StoreLimits, WriteMetadataRequest,
};
use finstack_ai_store_common::{
    admit_snapshot_sequence, check_snapshot_size, encode_state_request, protocol_error,
    scan_next_sequence, scan_start, validate_scan_limit,
};
use tokio_postgres::{Client, IsolationLevel, Statement, Transaction};

use crate::error::{Failure, commit_or_ambiguous, i64_from_u64, settle};
use crate::load::{
    SELECT_ENVELOPE_CHECKSUM, SELECT_SESSION_ROW, SessionRow, load_envelope_checksum,
    load_session_row, prepare, reconstruct_envelope,
};
use crate::pool::PooledClient;
use crate::session::{LOCK_SESSION_SQL, lock_session};

/// Upsert one session's snapshot row.
const UPSERT_SNAPSHOT: &str = "INSERT INTO snapshots (session_id, sequence, payload_cbor, digest, timestamp) \
     VALUES ($1, $2, $3, $4, 0) \
     ON CONFLICT (session_id) DO UPDATE SET \
         sequence = EXCLUDED.sequence, payload_cbor = EXCLUDED.payload_cbor, \
         digest = EXCLUDED.digest, timestamp = EXCLUDED.timestamp";

/// Point the session row at the snapshot just written.
const UPDATE_SNAPSHOT_POINTER: &str =
    "UPDATE sessions SET snapshot_sequence = $1 WHERE session_id = $2";

/// Replace one session's metadata blob.
const UPDATE_METADATA: &str = "UPDATE sessions SET metadata = $1 WHERE session_id = $2";

/// One page of a scan: an indexed range read over `records`.
const SELECT_SCAN_PAGE: &str = concat!(
    "SELECT ",
    record_columns!(),
    " FROM records WHERE session_id = $1 AND sequence >= $2 ORDER BY sequence LIMIT $3"
);

/// Statements a `write_snapshot` prepares before opening its transaction.
struct SnapshotStatements {
    /// [`crate::session::LOCK_SESSION_SQL`].
    lock_session: Statement,
    /// [`UPSERT_SNAPSHOT`].
    upsert: Statement,
    /// [`UPDATE_SNAPSHOT_POINTER`].
    pointer: Statement,
}

/// Statements a `write_metadata` prepares before opening its transaction.
struct MetadataStatements {
    /// [`crate::session::LOCK_SESSION_SQL`].
    lock_session: Statement,
    /// [`UPDATE_METADATA`].
    update: Statement,
}

/// Statements a `scan` prepares before opening its transaction.
struct ScanStatements {
    /// [`crate::load::SELECT_SESSION_ROW`].
    session_row: Statement,
    /// [`SELECT_SCAN_PAGE`].
    page: Statement,
    /// [`crate::load::SELECT_ENVELOPE_CHECKSUM`].
    envelope_checksum: Statement,
}

/// Replace the disposable replay snapshot for one session (spec D6 storage,
/// D-common admission).
///
/// The caller keeps ownership of the checkout; a connection that must not be
/// reused is marked with [`PooledClient::poison`] here rather than consumed,
/// exactly as [`crate::append::append`] does.
///
/// # Errors
///
/// Returns [`StoreError::LimitExceeded`] (`snapshot_bytes`) when the
/// snapshot exceeds `limits.snapshot_bytes`,
/// `InvalidRequest{snapshot_session_not_found}` when the session does not
/// exist, `InvalidRequest{snapshot_ahead_of_journal}` /
/// `InvalidRequest{snapshot_sequence_regression}` from
/// [`admit_snapshot_sequence`], and otherwise the mapped driver error.
pub(crate) async fn write_snapshot(
    client: &mut PooledClient<Client>,
    request: &SnapshotRequest,
    limits: &StoreLimits,
) -> Result<SnapshotReceipt, StoreError> {
    check_snapshot_size(request.snapshot.bytes().len(), limits.snapshot_bytes)?;
    // Prepared before the transaction opens: `Client::transaction` borrows
    // the client, and the statement cache lives on the checkout.
    let outcome = match prepare_snapshot_statements(client).await {
        Ok(statements) => write_snapshot_on_connection(client, &statements, request).await,
        Err(failure) => Err(failure),
    };
    settle(outcome, client)
}

/// Encode a [`StateSnapshotRequest`] and write it through [`write_snapshot`].
///
/// Pure composition, exactly as sqlite's `WorkerCtx::write_state_snapshot`:
/// [`encode_state_request`] does the protocol-level encoding (and its own
/// `snapshot_bytes` enforcement), then the encoded [`SnapshotRequest`] takes
/// the same admission/storage path as a caller-supplied snapshot.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] (`snapshot_encode_failed`) if the state
/// cannot be canonically encoded, and otherwise the errors documented on
/// [`write_snapshot`].
pub(crate) async fn write_state_snapshot(
    client: &mut PooledClient<Client>,
    request: &StateSnapshotRequest,
    limits: &StoreLimits,
) -> Result<SnapshotReceipt, StoreError> {
    let snapshot = encode_state_request(request, limits.snapshot_bytes)?;
    write_snapshot(
        client,
        &SnapshotRequest {
            session_id: request.session_id,
            snapshot,
        },
        limits,
    )
    .await
}

/// Prepare (or reuse) the statements a snapshot write needs.
async fn prepare_snapshot_statements(
    client: &mut PooledClient<Client>,
) -> Result<SnapshotStatements, Failure> {
    Ok(SnapshotStatements {
        lock_session: prepare(client, LOCK_SESSION_SQL).await?,
        upsert: prepare(client, UPSERT_SNAPSHOT).await?,
        pointer: prepare(client, UPDATE_SNAPSHOT_POINTER).await?,
    })
}

/// Drive one `write_snapshot` transaction to `COMMIT` or `ROLLBACK`.
async fn write_snapshot_on_connection(
    client: &mut Client,
    statements: &SnapshotStatements,
    request: &SnapshotRequest,
) -> Result<SnapshotReceipt, Failure> {
    let transaction = client
        .transaction()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let receipt = match write_snapshot_in_transaction(&transaction, statements, request).await {
        Ok(receipt) => receipt,
        Err(mut failure) => {
            if transaction.rollback().await.is_err() {
                failure.poison = true;
            }
            return Err(failure);
        }
    };

    commit_or_ambiguous(transaction).await?;
    Ok(receipt)
}

/// The snapshot-write body, inside the transaction: lock the session row,
/// admit the sequence, then upsert the snapshot row and the session's
/// `snapshot_sequence` pointer.
async fn write_snapshot_in_transaction(
    transaction: &Transaction<'_>,
    statements: &SnapshotStatements,
    request: &SnapshotRequest,
) -> Result<SnapshotReceipt, Failure> {
    let session = lock_session(transaction, &statements.lock_session, request.session_id)
        .await?
        .ok_or(StoreError::InvalidRequest {
            reason_code: "snapshot_session_not_found",
        })?;
    admit_snapshot_sequence(
        request.snapshot.sequence(),
        session.current_sequence,
        session.snapshot_sequence,
    )?;

    let sequence = i64_from_u64(request.snapshot.sequence(), "snapshot_sequence")?;
    transaction
        .execute(
            &statements.upsert,
            &[
                &request.session_id.as_bytes().as_slice(),
                &sequence,
                &request.snapshot.bytes(),
                &request.snapshot.digest().as_bytes().as_slice(),
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    transaction
        .execute(
            &statements.pointer,
            &[&sequence, &request.session_id.as_bytes().as_slice()],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    Ok(SnapshotReceipt {
        session_id: request.session_id,
        sequence: request.snapshot.sequence(),
        digest: request.snapshot.digest(),
        bytes: request.snapshot.bytes().len(),
    })
}

/// Scan committed envelopes of one session, starting at `from_sequence`
/// (port contract: `from_sequence == 0` means the first committed record,
/// `limit == 0` is invalid).
///
/// An indexed range read over `records` (`(session_id, sequence)` is the
/// table's primary key, so this is index-optimal) plus the same page
/// verification sqlite's `scan_session`/`verify_scan_page` perform
/// (`extensions/stores/finstack-ai-store-sqlite/src/load.rs`): a page short
/// of a live session's head is `scan_sequence_gap`, a page whose first
/// record does not chain from the stored checkpoint before it is
/// `scan_checkpoint_mismatch`, a broken internal chain is whatever
/// [`finstack_ai_protocol::verify_chain_from`] reports, and a page that
/// reaches the session head but computes a different checksum than the one
/// stored there is `head_checksum_mismatch`.
///
/// This store is deliberately *stricter* than sqlite in two places where
/// sqlite masks a missing record — a record absent at the requested start,
/// and an absent checkpoint row before the page. Both are reported
/// (`scan_sequence_gap` / `scan_checkpoint_mismatch`) unless the missing
/// sequence falls inside the prefix a prune may legitimately have deleted;
/// see [`pruned_prefix_covers`]. Failing closed on a hole in a live journal
/// is the whole point of chain verification, so the divergence is intended
/// rather than drift.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] (`scan_limit_zero` /
/// `scan_limit_exceeded`) from [`validate_scan_limit`],
/// [`StoreError::Integrity`] (`scan_sequence_gap` / `scan_checkpoint_mismatch`
/// / `head_checksum_mismatch`, or a protocol chain-verification code) when
/// the returned page does not verify, and otherwise the mapped driver error.
pub(crate) async fn scan(
    client: &mut PooledClient<Client>,
    request: ScanRequest,
) -> Result<ScanPage, StoreError> {
    validate_scan_limit(request.limit)?;
    // Prepared before the transaction opens, as everywhere else.
    let outcome = match prepare_scan_statements(client).await {
        Ok(statements) => scan_on_connection(client, &statements, request).await,
        Err(failure) => Err(failure),
    };
    settle(outcome, client)
}

/// Run one scan inside a read-only, repeatable-read transaction, mirroring
/// [`crate::load::load_on_connection`]'s isolation choice.
async fn prepare_scan_statements(
    client: &mut PooledClient<Client>,
) -> Result<ScanStatements, Failure> {
    Ok(ScanStatements {
        session_row: prepare(client, SELECT_SESSION_ROW).await?,
        page: prepare(client, SELECT_SCAN_PAGE).await?,
        envelope_checksum: prepare(client, SELECT_ENVELOPE_CHECKSUM).await?,
    })
}

/// Run one scan inside a read-only, repeatable-read transaction.
async fn scan_on_connection(
    client: &mut Client,
    statements: &ScanStatements,
    request: ScanRequest,
) -> Result<ScanPage, Failure> {
    let transaction = client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let page = match scan_in_transaction(&transaction, statements, request).await {
        Ok(page) => page,
        Err(mut failure) => {
            if transaction.rollback().await.is_err() {
                failure.poison = true;
            }
            return Err(failure);
        }
    };

    match transaction.commit().await {
        Ok(()) => Ok(page),
        Err(error) => Err(Failure::from_driver(&error)),
    }
}

/// Fetch one page of records at or after `scan_start(request.from_sequence)`,
/// over-fetching by one row to determine `has_more` without a second round
/// trip, then verify it exactly as sqlite's `scan_session` does.
async fn scan_in_transaction(
    transaction: &Transaction<'_>,
    statements: &ScanStatements,
    request: ScanRequest,
) -> Result<ScanPage, Failure> {
    let start = scan_start(request.from_sequence);
    // A session that does not exist scans as empty, per the port contract —
    // sqlite's `scan_session` reaches the same outcome via an explicit
    // `session_exists` check; one round trip suffices here because the
    // whole scan runs inside one `READ ONLY REPEATABLE READ` transaction.
    let Some(session) =
        load_session_row(transaction, &statements.session_row, request.session_id).await?
    else {
        return Ok(ScanPage {
            session_id: request.session_id,
            records: Arc::from([]),
            next_sequence: None,
        });
    };

    let fetch_limit = i64::from(request.limit.saturating_add(1));
    let rows = transaction
        .query(
            &statements.page,
            &[
                &request.session_id.as_bytes().as_slice(),
                &i64_from_u64(start, "from_sequence")?,
                &fetch_limit,
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    if rows.is_empty() {
        // No record at or after `start`: legitimate only when `start` is
        // past the session's current head. A `start` at or before the head
        // with nothing there is a hole in the journal.
        if start <= session.current_sequence {
            return Err(StoreError::Integrity {
                reason_code: "scan_sequence_gap",
            }
            .into());
        }
        return Ok(ScanPage {
            session_id: request.session_id,
            records: Arc::from([]),
            next_sequence: None,
        });
    }

    let limit = usize::try_from(request.limit).unwrap_or(usize::MAX);
    let has_more = rows.len() > limit;
    let records = rows
        .iter()
        .take(limit)
        .map(reconstruct_envelope)
        .collect::<Result<Vec<_>, _>>()?;
    // A record missing *at* the requested start. The SQL asked for
    // `sequence >= start`, so the first row coming back above `start` means
    // the record at `start` is not stored. That is legitimate only when
    // `start` falls inside the pruned prefix: `crate::prune::prune` deletes
    // records with `sequence < snapshot_sequence`, so every sequence at or
    // above `snapshot_sequence` must still exist, and a session with no
    // snapshot has never been pruned at all. Anything else is a hole.
    //
    // This is deliberately stricter than sqlite, which masks a missing
    // record at the page start entirely; postgres fails closed instead.
    if let Some(first) = records.first()
        && first.sequence() > start
        && !pruned_prefix_covers(&session, start)
    {
        return Err(StoreError::Integrity {
            reason_code: "scan_sequence_gap",
        }
        .into());
    }
    verify_scan_page(
        transaction,
        statements,
        request.session_id,
        &records,
        &session,
    )
    .await?;
    let next_sequence = scan_next_sequence(&records, has_more);
    Ok(ScanPage {
        session_id: request.session_id,
        records: records.into(),
        next_sequence,
    })
}

/// `true` when `sequence` falls inside the prefix a prune may already have
/// deleted, so a record missing there is legitimate rather than a hole.
///
/// [`crate::prune::prune`] deletes records with
/// `sequence < snapshot_sequence` — strictly less, so the record *at* the
/// snapshot boundary is retained and every sequence at or above it must
/// still be stored. A session whose `snapshot_sequence` is `NULL` has never
/// been prunable at all, so nothing is covered.
fn pruned_prefix_covers(session: &SessionRow, sequence: u64) -> bool {
    session
        .snapshot_sequence
        .is_some_and(|boundary| sequence < boundary)
}

/// Chain-verify a scan page, mirroring sqlite's `verify_scan_page` — with
/// two deliberate tightenings where sqlite masks a missing record (see the
/// `None` arms below and the start-of-page check in `scan_in_transaction`):
/// anchor the page's first record against the stored checksum of the record
/// immediately before it (when one exists), walk the chain across the page,
/// and — only when the page reaches the session's current head — compare
/// the resulting checksum against the stored head checksum.
async fn verify_scan_page(
    transaction: &Transaction<'_>,
    statements: &ScanStatements,
    session_id: SessionId,
    records: &[RecordEnvelope],
    session: &SessionRow,
) -> Result<(), Failure> {
    let Some(first) = records.first() else {
        return Ok(());
    };
    let prior: Option<Digest> = if first.sequence() <= 1 {
        None
    } else {
        let checkpoint = first.sequence().saturating_sub(1);
        match load_envelope_checksum(
            transaction,
            &statements.envelope_checksum,
            session_id,
            checkpoint,
        )
        .await?
        {
            Some(stored) => {
                if first.previous_checksum() != Some(stored) {
                    return Err(StoreError::Integrity {
                        reason_code: "scan_checkpoint_mismatch",
                    }
                    .into());
                }
                Some(stored)
            }
            // No stored checkpoint before the page. Legitimate only when
            // that sequence sits inside the pruned prefix (same boundary
            // rule as `scan_in_transaction`: prune deletes
            // `sequence < snapshot_sequence`, so anything at or above the
            // boundary must still exist). There the fallback below is
            // sound — the anchor really is gone, and the page is verified
            // from its own first record forward, exactly as sqlite does.
            //
            // Otherwise the checkpoint row vanished from a live journal,
            // and falling back to the record's *self-reported*
            // `previous_checksum` would let the page anchor on a value
            // nothing corroborates. Fail closed instead — again stricter
            // than sqlite, which always falls back.
            None if pruned_prefix_covers(session, checkpoint) => first.previous_checksum(),
            None => {
                return Err(StoreError::Integrity {
                    reason_code: "scan_checkpoint_mismatch",
                }
                .into());
            }
        }
    };
    let head = verify_chain_from(records, prior, Some(first.sequence())).map_err(protocol_error)?;
    if records
        .last()
        .is_some_and(|record| record.sequence() == session.current_sequence)
        && head != session.head_checksum
    {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        }
        .into());
    }
    Ok(())
}

/// Compare-and-swap session metadata against the expected head checksum.
///
/// The caller keeps ownership of the checkout; a connection that must not be
/// reused is marked with [`PooledClient::poison`] here rather than consumed,
/// exactly as [`write_snapshot`] does.
///
/// # Errors
///
/// Returns `InvalidRequest{metadata_session_not_found}` when the session
/// does not exist, `InvalidRequest{metadata_cas_mismatch}` (sqlite's exact
/// reason code) when `request.expected_head_checksum` does not equal the
/// session's current head checksum under the row lock, and otherwise the
/// mapped driver error.
pub(crate) async fn write_metadata(
    client: &mut PooledClient<Client>,
    request: &WriteMetadataRequest,
) -> Result<MetadataReceipt, StoreError> {
    // Prepared before the transaction opens, as everywhere else.
    let outcome = match prepare_metadata_statements(client).await {
        Ok(statements) => write_metadata_on_connection(client, &statements, request).await,
        Err(failure) => Err(failure),
    };
    settle(outcome, client)
}

/// Prepare (or reuse) the statements a metadata CAS needs.
async fn prepare_metadata_statements(
    client: &mut PooledClient<Client>,
) -> Result<MetadataStatements, Failure> {
    Ok(MetadataStatements {
        lock_session: prepare(client, LOCK_SESSION_SQL).await?,
        update: prepare(client, UPDATE_METADATA).await?,
    })
}

/// Drive one `write_metadata` transaction to `COMMIT` or `ROLLBACK`.
async fn write_metadata_on_connection(
    client: &mut Client,
    statements: &MetadataStatements,
    request: &WriteMetadataRequest,
) -> Result<MetadataReceipt, Failure> {
    let transaction = client
        .transaction()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let receipt = match write_metadata_in_transaction(&transaction, statements, request).await {
        Ok(receipt) => receipt,
        Err(mut failure) => {
            if transaction.rollback().await.is_err() {
                failure.poison = true;
            }
            return Err(failure);
        }
    };

    commit_or_ambiguous(transaction).await?;
    Ok(receipt)
}

/// The metadata-CAS body, inside the transaction.
async fn write_metadata_in_transaction(
    transaction: &Transaction<'_>,
    statements: &MetadataStatements,
    request: &WriteMetadataRequest,
) -> Result<MetadataReceipt, Failure> {
    let session = lock_session(transaction, &statements.lock_session, request.session_id)
        .await?
        .ok_or(StoreError::InvalidRequest {
            reason_code: "metadata_session_not_found",
        })?;
    if session.head_checksum != request.expected_head_checksum {
        return Err(StoreError::InvalidRequest {
            reason_code: "metadata_cas_mismatch",
        }
        .into());
    }
    transaction
        .execute(
            &statements.update,
            &[
                &request.metadata.as_bytes(),
                &request.session_id.as_bytes().as_slice(),
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    Ok(MetadataReceipt {
        session_id: request.session_id,
        head_checksum: session.head_checksum,
        metadata: request.metadata.clone(),
    })
}
