//! [`JournalStore`] implementation for [`PostgresJournalStore`].
//!
//! [`JournalStore::append`] (spec D4/D5, see [`crate::append`]),
//! [`JournalStore::load`]/[`JournalStore::load_from`] (spec D9, see
//! [`crate::load`]), [`JournalStore::health`], and
//! [`JournalStore::write_snapshot`]/[`JournalStore::write_state_snapshot`]/
//! [`JournalStore::scan`]/[`JournalStore::write_metadata`] (see
//! [`crate::snapshot`]), and [`JournalStore::prune`] (see [`crate::prune`])
//! are all implemented.
//!
//! ## The verified-head cache
//!
//! `load`/`load_from` own the spec D9 cache lifecycle: read the cached proof
//! before the load, drop it on any `Integrity` result, and replace it with
//! the freshly verified head on success. The map is only ever touched by the
//! synchronous helpers in [`crate::store`], so its `std::sync::Mutex` is
//! never held across an `.await`.
//!
//! Loads run concurrently, so the read and the write-back are not atomic:
//! the cache read returns the *generation* it saw, and `remember_head`
//! discards a write whose generation is stale. Without that guard a load
//! that started before a concurrent load found corruption could write its
//! head afterwards and permanently resurrect the invalidated proof.

use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch, SessionId};
use finstack_ai_runtime::{
    JournalStore, LoadFromRequest, LoadRequest, LoadWindow, LoadedSession, MetadataReceipt,
    PortFuture, PruneReceipt, PruneRequest, ScanPage, ScanRequest, SnapshotReceipt,
    SnapshotRequest, StateSnapshotRequest, StoreError, StoreHealth, WriteMetadataRequest,
};

use crate::append::append;
use crate::config::PostgresDurability;
use crate::load::{VerifiedHead, load};
use crate::prune;
use crate::snapshot;
use crate::store::{
    DURABLE_DETAIL, PostgresJournalStore, RELAXED_DETAIL, VerifiedCache, cached_head,
    invalidate_head, remember_head,
};

impl JournalStore for PostgresJournalStore {
    /// Multi-writer append over one pooled connection (spec D4/D5).
    ///
    /// Never retries internally: a serialization failure or deadlock
    /// surfaces as `Unavailable{postgres_serialization}` and an
    /// interrupted `COMMIT` as
    /// [`StoreError::AmbiguousAcknowledgement`], both of which the caller
    /// resolves by retrying the same request — which the idempotency
    /// contract makes safe.
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let pool = self.pool.clone();
        let limits = self.config.limits;
        Box::pin(async move {
            let mut client = pool.get().await?;
            append(&mut client, &request, &limits).await
        })
    }

    /// Full, chain-verified load of one session journal (spec D9).
    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.verified_load(request.session_id, LoadWindow::Full)
    }

    /// Windowed load, implemented natively rather than through the port's
    /// load-and-trim default (spec D9).
    ///
    /// `FromSequence` and `SnapshotPlusTail` fetch only the requested range
    /// from SQL and verify it with store-common's tail verification, so the
    /// omitted prefix is never read — while still fail-closing on the
    /// window's gap/split/checksum codes.
    fn load_from(&self, request: LoadFromRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.verified_load(request.session_id, request.window)
    }

    /// Replace the disposable replay snapshot for one session, per
    /// [`crate::snapshot::write_snapshot`].
    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let pool = self.pool.clone();
        let limits = self.config.limits;
        Box::pin(async move {
            let mut client = pool.get().await?;
            snapshot::write_snapshot(&mut client, &request, &limits).await
        })
    }

    /// Encode and replace one session's disposable kernel-state snapshot,
    /// per [`crate::snapshot::write_state_snapshot`].
    fn write_state_snapshot(
        &self,
        request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let pool = self.pool.clone();
        let limits = self.config.limits;
        Box::pin(async move {
            let mut client = pool.get().await?;
            snapshot::write_state_snapshot(&mut client, &request, &limits).await
        })
    }

    /// Scan committed envelopes of one session, per [`crate::snapshot::scan`].
    fn scan(&self, request: ScanRequest) -> PortFuture<Result<ScanPage, StoreError>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let mut client = pool.get().await?;
            snapshot::scan(&mut client, request).await
        })
    }

    /// Compare-and-swap session metadata, per
    /// [`crate::snapshot::write_metadata`].
    fn write_metadata(
        &self,
        request: WriteMetadataRequest,
    ) -> PortFuture<Result<MetadataReceipt, StoreError>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let mut client = pool.get().await?;
            snapshot::write_metadata(&mut client, &request).await
        })
    }

    /// Snapshot-aligned prefix prune, per [`crate::prune::prune`].
    ///
    /// Never touches the verified-head cache: prune only ever deletes an
    /// already-pruned prefix a cached suffix proof does not depend on, so
    /// the cached head (if any) is left exactly as it was.
    fn prune(&self, request: PruneRequest) -> PortFuture<Result<PruneReceipt, StoreError>> {
        let pool = self.pool.clone();
        let snapshot_bytes = self.config.limits.snapshot_bytes;
        Box::pin(async move {
            let mut client = pool.get().await?;
            prune::prune(&mut client, &request, snapshot_bytes).await
        })
    }

    /// `SELECT 1` round-trip on a pooled connection (spec D6).
    ///
    /// A probe failure (cannot check out a connection, or the query itself
    /// fails) reports `ready: false` rather than propagating an `Err` — per
    /// the port contract, `health()` is a status report, not a fallible
    /// operation.
    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        let pool = self.pool.clone();
        let durable = matches!(self.config.durability, PostgresDurability::Durable);
        let detail: Arc<str> = Arc::from(if durable {
            DURABLE_DETAIL
        } else {
            RELAXED_DETAIL
        });

        Box::pin(async move {
            let ready = match pool.get().await {
                Ok(pooled) => match pooled.query_one("SELECT 1", &[]).await {
                    Ok(_row) => true,
                    Err(_error) => {
                        // The probe query failed on an otherwise checked-out
                        // connection; treat it as poisoned per spec D2
                        // rather than risk returning a broken connection to
                        // the idle queue for the next caller.
                        pooled.discard();
                        false
                    }
                },
                Err(_error) => false,
            };
            Ok(StoreHealth {
                ready,
                durable,
                detail,
            })
        })
    }
}

impl PostgresJournalStore {
    /// Shared body of [`JournalStore::load`] and [`JournalStore::load_from`]:
    /// run the load under the session's cached verified head and maintain
    /// that cache from the outcome (spec D9).
    fn verified_load(
        &self,
        session_id: SessionId,
        window: LoadWindow,
    ) -> PortFuture<Result<LoadedSession, StoreError>> {
        let pool = self.pool.clone();
        let snapshot_bytes = self.config.limits.snapshot_bytes;
        let cache: VerifiedCache = Arc::clone(&self.verified);
        Box::pin(async move {
            let mut client = pool.get().await?;
            // Read the cache as late as possible — after the (potentially
            // blocking) checkout — so the window in which another load can
            // invalidate between the read and the write-back is as small as
            // it can be. Correctness does not depend on that window being
            // small: `remember_head` rejects a write whose generation is
            // stale (see `crate::store`).
            let read = cached_head(&cache, session_id);
            let result = load(&mut client, session_id, snapshot_bytes, window, read.head).await;
            match &result {
                // Only a full load establishes a proof over the *whole*
                // chain. A window's tail was verified against a checksum the
                // caller supplied, which says nothing about the prefix it
                // omitted, so caching its head would let a later full load
                // skip records this process never verified.
                Ok(loaded) if matches!(window, LoadWindow::Full) => remember_head(
                    &cache,
                    session_id,
                    read,
                    VerifiedHead {
                        sequence: loaded.head_sequence,
                        checksum: loaded.head_checksum,
                    },
                ),
                Ok(_windowed) => {}
                // Spec D9(b): any integrity failure invalidates whatever this
                // process believed it had verified for this session.
                Err(StoreError::Integrity { .. }) => invalidate_head(&cache, session_id),
                Err(_other) => {}
            }
            result
        })
    }
}
