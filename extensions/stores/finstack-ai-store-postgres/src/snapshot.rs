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
use tokio_postgres::{Client, IsolationLevel, Transaction};

use crate::error::{Failure, i64_from_u64};
use crate::load::{
    RECORD_COLUMNS, SessionRow, load_envelope_checksum, load_session_row, reconstruct_envelope,
};
use crate::pool::PooledClient;
use crate::session::lock_session;

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
    let outcome = write_snapshot_on_connection(client, request).await;
    match outcome {
        Ok(receipt) => Ok(receipt),
        Err(failure) => {
            if failure.poison {
                client.poison();
            }
            Err(failure.error)
        }
    }
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

/// Drive one `write_snapshot` transaction to `COMMIT` or `ROLLBACK`.
async fn write_snapshot_on_connection(
    client: &mut Client,
    request: &SnapshotRequest,
) -> Result<SnapshotReceipt, Failure> {
    let transaction = client
        .transaction()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let receipt = match write_snapshot_in_transaction(&transaction, request).await {
        Ok(receipt) => receipt,
        Err(mut failure) => {
            if transaction.rollback().await.is_err() {
                failure.poison = true;
            }
            return Err(failure);
        }
    };

    match transaction.commit().await {
        Ok(()) => Ok(receipt),
        // Spec D5: no SQLSTATE means the server never reported an outcome.
        Err(error) if error.code().is_none() => Err(Failure {
            error: StoreError::AmbiguousAcknowledgement,
            poison: true,
        }),
        Err(error) => Err(Failure::from_driver(&error)),
    }
}

/// The snapshot-write body, inside the transaction: lock the session row,
/// admit the sequence, then upsert the snapshot row and the session's
/// `snapshot_sequence` pointer.
async fn write_snapshot_in_transaction(
    transaction: &Transaction<'_>,
    request: &SnapshotRequest,
) -> Result<SnapshotReceipt, Failure> {
    let session =
        lock_session(transaction, request.session_id)
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
            "INSERT INTO snapshots (session_id, sequence, payload_cbor, digest, timestamp) \
             VALUES ($1, $2, $3, $4, 0) \
             ON CONFLICT (session_id) DO UPDATE SET \
                 sequence = EXCLUDED.sequence, payload_cbor = EXCLUDED.payload_cbor, \
                 digest = EXCLUDED.digest, timestamp = EXCLUDED.timestamp",
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
            "UPDATE sessions SET snapshot_sequence = $1 WHERE session_id = $2",
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
    let outcome = scan_on_connection(client, request).await;
    match outcome {
        Ok(page) => Ok(page),
        Err(failure) => {
            if failure.poison {
                client.poison();
            }
            Err(failure.error)
        }
    }
}

/// Run one scan inside a read-only, repeatable-read transaction, mirroring
/// [`crate::load::load_on_connection`]'s isolation choice.
async fn scan_on_connection(
    client: &mut Client,
    request: ScanRequest,
) -> Result<ScanPage, Failure> {
    let transaction = client
        .build_transaction()
        .isolation_level(IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let page = match scan_in_transaction(&transaction, request).await {
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
    request: ScanRequest,
) -> Result<ScanPage, Failure> {
    let start = scan_start(request.from_sequence);
    // A session that does not exist scans as empty, per the port contract —
    // sqlite's `scan_session` reaches the same outcome via an explicit
    // `session_exists` check; one round trip suffices here because the
    // whole scan runs inside one `READ ONLY REPEATABLE READ` transaction.
    let Some(session) = load_session_row(transaction, request.session_id).await? else {
        return Ok(ScanPage {
            session_id: request.session_id,
            records: Arc::from([]),
            next_sequence: None,
        });
    };

    let fetch_limit = i64::from(request.limit.saturating_add(1));
    let rows = transaction
        .query(
            &format!(
                "SELECT {RECORD_COLUMNS} FROM records \
                 WHERE session_id = $1 AND sequence >= $2 ORDER BY sequence LIMIT $3"
            ),
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
    if records
        .first()
        .is_some_and(|record| record.sequence() < start)
    {
        return Err(StoreError::Integrity {
            reason_code: "scan_sequence_gap",
        }
        .into());
    }
    verify_scan_page(transaction, request.session_id, &records, &session).await?;
    let next_sequence = scan_next_sequence(&records, has_more);
    Ok(ScanPage {
        session_id: request.session_id,
        records: records.into(),
        next_sequence,
    })
}

/// Chain-verify a scan page, mirroring sqlite's `verify_scan_page` exactly:
/// anchor the page's first record against the stored checksum of the record
/// immediately before it (when one exists), walk the chain across the page,
/// and — only when the page reaches the session's current head — compare
/// the resulting checksum against the stored head checksum.
async fn verify_scan_page(
    transaction: &Transaction<'_>,
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
        match load_envelope_checksum(transaction, session_id, checkpoint).await? {
            Some(stored) => {
                if first.previous_checksum() != Some(stored) {
                    return Err(StoreError::Integrity {
                        reason_code: "scan_checkpoint_mismatch",
                    }
                    .into());
                }
                Some(stored)
            }
            None => first.previous_checksum(),
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
    let outcome = write_metadata_on_connection(client, request).await;
    match outcome {
        Ok(receipt) => Ok(receipt),
        Err(failure) => {
            if failure.poison {
                client.poison();
            }
            Err(failure.error)
        }
    }
}

/// Drive one `write_metadata` transaction to `COMMIT` or `ROLLBACK`.
async fn write_metadata_on_connection(
    client: &mut Client,
    request: &WriteMetadataRequest,
) -> Result<MetadataReceipt, Failure> {
    let transaction = client
        .transaction()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let receipt = match write_metadata_in_transaction(&transaction, request).await {
        Ok(receipt) => receipt,
        Err(mut failure) => {
            if transaction.rollback().await.is_err() {
                failure.poison = true;
            }
            return Err(failure);
        }
    };

    match transaction.commit().await {
        Ok(()) => Ok(receipt),
        // Spec D5: no SQLSTATE means the server never reported an outcome.
        Err(error) if error.code().is_none() => Err(Failure {
            error: StoreError::AmbiguousAcknowledgement,
            poison: true,
        }),
        Err(error) => Err(Failure::from_driver(&error)),
    }
}

/// The metadata-CAS body, inside the transaction.
async fn write_metadata_in_transaction(
    transaction: &Transaction<'_>,
    request: &WriteMetadataRequest,
) -> Result<MetadataReceipt, Failure> {
    let session =
        lock_session(transaction, request.session_id)
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
            "UPDATE sessions SET metadata = $1 WHERE session_id = $2",
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
