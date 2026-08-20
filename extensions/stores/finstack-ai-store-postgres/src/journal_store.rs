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
//! the freshly verified head on success. The cache container itself lives in
//! [`finstack_ai_store_common`]; its `std::sync::Mutex` is only ever taken by
//! synchronous calls, never across an `.await`.
//!
//! Loads run concurrently, so the read and the write-back are not atomic:
//! the cache read returns the *generation* it saw, and
//! [`finstack_ai_store_common::VerifiedHeadCache::remember`] discards a write
//! whose generation is stale. Without that guard a load that started before a
//! concurrent load found corruption could write its head afterwards and
//! permanently resurrect the invalidated proof.

use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch, SessionId};
use finstack_ai_runtime::{
    JournalStore, LoadFromRequest, LoadRequest, LoadWindow, LoadedSession, MetadataReceipt,
    PortFuture, PruneReceipt, PruneRequest, ScanPage, ScanRequest, SnapshotReceipt,
    SnapshotRequest, StateSnapshotRequest, StoreError, StoreHealth, WriteMetadataRequest,
};

use finstack_ai_store_common::{VerifiedHead, VerifiedHeadCache};

use crate::append::append;
use crate::config::PostgresDurability;
use crate::load::load;
use crate::prune;
use crate::snapshot;
use crate::store::{DURABLE_DETAIL, PostgresJournalStore, RELAXED_DETAIL};

impl JournalStore for PostgresJournalStore {
    /// Multi-writer append over one pooled connection (spec D4/D5).
    ///
    /// Never retries internally: a serialization failure or deadlock
    /// surfaces as the transient `Unavailable{postgres_serialization}` and
    /// an interrupted `COMMIT` as
    /// [`StoreError::AmbiguousAcknowledgement`]. The runtime's
    /// `CommitCoordinator` retries the latter (along with `Conflict`) but
    /// *not* `Unavailable`, which propagates as a hard error just as
    /// sqlite's `sqlite_busy` does; whether to retry it is the embedding
    /// application's policy. Retrying the same request is safe either way —
    /// the idempotency contract guarantees it.
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
    /// A probe failure (cannot check out a connection, the checkout itself
    /// timing out, or the query failing) reports `ready: false` rather than
    /// propagating an `Err` — per the port contract, `health()` is a status
    /// report, not a fallible operation.
    ///
    /// Both halves of the probe are bounded by
    /// [`crate::config::PostgresStoreConfig::connect_timeout`]. The checkout
    /// needs it because on an exhausted pool (every permit checked out and
    /// none returned) `Pool::get` would otherwise wait on the semaphore
    /// indefinitely, turning one stuck caller into a `health()` that never
    /// resolves. The `SELECT 1` round trip needs it because a connection
    /// whose backend is black-holed (a dropped network path with no RST, a
    /// wedged server) never returns an error either — it simply never
    /// answers. A probe that times out reports `ready: false` *and* discards
    /// the connection: it may still deliver its answer later, so it must
    /// never go back to the idle queue (spec D2).
    ///
    /// This only bounds the health path — every other [`JournalStore`]
    /// method still awaits `Pool::get` and its statements without a timeout,
    /// so their semantics are unchanged.
    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        let pool = self.pool.clone();
        let durable = matches!(self.config.durability, PostgresDurability::Durable);
        let detail: Arc<str> = Arc::from(if durable {
            DURABLE_DETAIL
        } else {
            RELAXED_DETAIL
        });
        let checkout_bound = self.config.connect_timeout;

        Box::pin(async move {
            let ready = match tokio::time::timeout(checkout_bound, pool.get()).await {
                Ok(Ok(pooled)) => {
                    let probe =
                        tokio::time::timeout(checkout_bound, pooled.query_one("SELECT 1", &[]))
                            .await;
                    if matches!(probe, Ok(Ok(_))) {
                        true
                    } else {
                        // The probe query failed — or never answered within
                        // `checkout_bound` — on an otherwise checked-out
                        // connection; treat it as poisoned per spec D2
                        // rather than risk returning a broken (or
                        // black-holed, with an unread `SELECT 1` reply still
                        // in flight) connection to the idle queue for the
                        // next caller.
                        pooled.discard();
                        false
                    }
                }
                // Either the checkout itself failed, or it did not resolve
                // within `checkout_bound` (pool exhausted) — both report a
                // not-ready store rather than propagating an `Err` or
                // hanging.
                Ok(Err(_)) | Err(_) => false,
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
        let cache: Arc<VerifiedHeadCache> = Arc::clone(&self.verified);
        Box::pin(async move {
            let mut client = pool.get().await?;
            // Read the cache as late as possible — after the (potentially
            // blocking) checkout — so the window in which another load can
            // invalidate between the read and the write-back is as small as
            // it can be. Correctness does not depend on that window being
            // small: `VerifiedHeadCache::remember` rejects a write whose
            // generation is stale.
            let read = cache.read(session_id);
            let result = load(&mut client, session_id, snapshot_bytes, window, read.head).await;
            match &result {
                // Only a full load establishes a proof over the *whole*
                // chain. A window's tail was verified against a checksum the
                // caller supplied, which says nothing about the prefix it
                // omitted, so caching its head would let a later full load
                // skip records this process never verified.
                Ok(loaded) if matches!(window, LoadWindow::Full) => cache.remember(
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
                Err(StoreError::Integrity { .. }) => cache.invalidate(session_id),
                Err(_other) => {}
            }
            result
        })
    }
}
