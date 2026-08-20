//! Snapshot-aligned prefix prune.
//!
//! ## Where the semantics live
//!
//! As in [`crate::snapshot`], the store-contract *decisions* live in
//! [`finstack_ai_store_common`]: prune admission
//! ([`admit_prune_snapshot`], requiring a nonzero snapshot sequence at or
//! before the journal head) and receipt counting
//! ([`outstanding_count`], [`tombstone_count`]). This module supplies the SQL
//! and transaction plumbing, called in exactly the order sqlite's
//! `SqliteJournalStore::prune` calls them in
//! `extensions/stores/finstack-ai-store-sqlite/src/store.rs`, so the two
//! backends cannot drift in reason code, check ordering, or deletion
//! predicate.
//!
//! ## Batch alignment
//!
//! `admit_prune_snapshot` only checks the snapshot sequence against the
//! journal head; it says nothing about whether that sequence lands on a
//! batch boundary. Alignment is a separate check, run here exactly as
//! sqlite's `prune` runs it over its `batches` table: a snapshot sequence
//! that is not some batch's `last_sequence` is
//! `InvalidRequest{prune_not_batch_aligned}`. (See the twin comment in
//! sqlite's `store.rs` and the memory store's `prune_sync`: change all three
//! together.)
//!
//! ## What gets deleted
//!
//! Mirroring sqlite exactly: records with `sequence < pruned_through` and
//! batches with `last_sequence < pruned_through` are deleted, where
//! `pruned_through` is the snapshot's sequence. The strict `<` (not `<=`)
//! retains the record *at* the snapshot boundary and the batch that produced
//! it — sqlite's snapshot payload only proves state through that record, so
//! keeping it (rather than the whole prefix up to and including it) is what
//! lets a post-prune load still chain-verify a `SnapshotPlusTail` window from
//! the retained boundary forward.
//!
//! ## Locking
//!
//! One transaction, under the session row's `FOR UPDATE` lock taken via
//! [`crate::session::lock_session`] — the same lock [`crate::append::append`]
//! and [`crate::snapshot::write_snapshot`] take — so a prune and a concurrent
//! append/snapshot-write/metadata-write of the same session serialize rather
//! than race. Prune never touches this process's [`crate::store::VerifiedCache`]
//! entry for the session: it only ever deletes an already-pruned prefix the
//! cache's suffix proof does not depend on, so the cached head stays valid
//! for the next load (spec D9).
//!
//! ## Connection disposition
//!
//! Identical to append/snapshot: every error carries a [`Failure`] poison
//! flag, and a connection that saw a wire-level failure (or a rollback that
//! could not be delivered) is discarded rather than returned to the pool
//! (spec D2). A `COMMIT` that fails without a SQLSTATE is the spec D5
//! ambiguous acknowledgement.

use finstack_ai_kernel::SessionId;
use finstack_ai_runtime::{PruneReceipt, PruneRequest, StoreError};
use finstack_ai_store_common::{
    accelerated_from, admit_prune_snapshot, outstanding_count, tombstone_count,
};
use tokio_postgres::{Client, Transaction};

use crate::error::{Failure, i64_from_u64};
use crate::load::load_snapshot;
use crate::pool::PooledClient;
use crate::session::lock_session;

/// Prune the snapshot-covered prefix of `request.session_id`.
///
/// The caller keeps ownership of the checkout; a connection that must not be
/// reused is marked with [`PooledClient::poison`] here rather than consumed,
/// exactly as [`crate::append::append`] does.
///
/// # Errors
///
/// Returns `InvalidRequest{prune_session_not_found}` when the session does
/// not exist, `InvalidRequest{prune_requires_snapshot}` when it has no stored
/// snapshot, the [`admit_prune_snapshot`] admission errors
/// (`InvalidRequest{prune_snapshot_not_aligned}`),
/// `InvalidRequest{prune_not_batch_aligned}` when the snapshot sequence does
/// not land on a batch boundary, `Integrity{prune_snapshot_undecodable}` when
/// the stored snapshot cannot be decoded into an
/// [`finstack_ai_runtime::AcceleratedRestore`], and otherwise the mapped
/// driver error.
pub(crate) async fn prune(
    client: &mut PooledClient<Client>,
    request: &PruneRequest,
    snapshot_bytes: usize,
) -> Result<PruneReceipt, StoreError> {
    let outcome = prune_on_connection(client, request, snapshot_bytes).await;
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

/// Drive one prune transaction to `COMMIT` or `ROLLBACK`.
async fn prune_on_connection(
    client: &mut Client,
    request: &PruneRequest,
    snapshot_bytes: usize,
) -> Result<PruneReceipt, Failure> {
    let transaction = client
        .transaction()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let receipt = match prune_in_transaction(&transaction, request, snapshot_bytes).await {
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

/// The prune body, inside the transaction: lock the session row, admit and
/// decode the covering snapshot, delete the covered prefix, and maintain the
/// session's committed footprint.
async fn prune_in_transaction(
    transaction: &Transaction<'_>,
    request: &PruneRequest,
    snapshot_bytes: usize,
) -> Result<PruneReceipt, Failure> {
    let session =
        lock_session(transaction, request.session_id)
            .await?
            .ok_or(StoreError::InvalidRequest {
                reason_code: "prune_session_not_found",
            })?;
    let snapshot = load_snapshot(transaction, request.session_id, snapshot_bytes)
        .await?
        .ok_or(StoreError::InvalidRequest {
            reason_code: "prune_requires_snapshot",
        })?;
    admit_prune_snapshot(snapshot.sequence(), session.current_sequence)?;

    let pruned_through = i64_from_u64(snapshot.sequence(), "snapshot_sequence")?;
    if !batch_boundary_exists(transaction, request.session_id, pruned_through).await? {
        return Err(StoreError::InvalidRequest {
            reason_code: "prune_not_batch_aligned",
        }
        .into());
    }

    let accelerated = accelerated_from(&snapshot).ok_or(StoreError::Integrity {
        reason_code: "prune_snapshot_undecodable",
    })?;
    let retained_outstanding = outstanding_count(&accelerated);
    let retained_tombstones = tombstone_count(&accelerated);

    let deleted_records = transaction
        .execute(
            "DELETE FROM records WHERE session_id = $1 AND sequence < $2",
            &[&request.session_id.as_bytes().as_slice(), &pruned_through],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let deleted_batches = transaction
        .execute(
            "DELETE FROM batches WHERE session_id = $1 AND last_sequence < $2",
            &[&request.session_id.as_bytes().as_slice(), &pruned_through],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let record_delta = i64::try_from(deleted_records).map_err(|_| StoreError::Integrity {
        reason_code: "prune_deleted_record_count_overflow",
    })?;
    let batch_delta = i64::try_from(deleted_batches).map_err(|_| StoreError::Integrity {
        reason_code: "prune_deleted_batch_count_overflow",
    })?;
    transaction
        .execute(
            "UPDATE sessions SET batch_count = batch_count - $1, record_count = record_count - $2 \
             WHERE session_id = $3",
            &[
                &batch_delta,
                &record_delta,
                &request.session_id.as_bytes().as_slice(),
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    Ok(PruneReceipt {
        pruned_through_sequence: snapshot.sequence(),
        retained_outstanding,
        retained_tombstones,
    })
}

/// `true` when some batch of `session_id` ends exactly at `sequence`.
///
/// Alignment rule twin: the sqlite store enforces the same
/// "snapshot ends exactly at a batch's `last_sequence`" predicate over SQL in
/// its own `prune` (`extensions/stores/finstack-ai-store-sqlite/src/store.rs`),
/// and the memory store enforces it over its in-memory batch list
/// (`extensions/stores/finstack-ai-store-memory/src/lib.rs::prune_sync`);
/// change all three together.
async fn batch_boundary_exists(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    sequence: i64,
) -> Result<bool, Failure> {
    let count: i64 = transaction
        .query_one(
            "SELECT COUNT(*) FROM batches WHERE session_id = $1 AND last_sequence = $2",
            &[&session_id.as_bytes().as_slice(), &sequence],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?
        .get(0);
    Ok(count > 0)
}
