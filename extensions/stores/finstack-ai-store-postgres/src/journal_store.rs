//! [`JournalStore`] implementation for [`PostgresJournalStore`].
//!
//! [`JournalStore::append`] (spec D4/D5, see [`crate::append`]) and
//! [`JournalStore::health`] are implemented. `load` and `write_snapshot`
//! are non-default methods on the trait (the port has no default body for
//! them), so they need *some* implementation to compile; Tasks 5–6 replace
//! these stubs with chain verification (spec D9) and store-common snapshot
//! wiring. Every
//! other trait method (`scan`, `write_metadata`, `write_state_snapshot`,
//! `prune`, `load_from`) keeps the port's own default implementation for
//! now.

use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch};
use finstack_ai_runtime::{
    JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth,
};

use crate::append::append;
use crate::config::PostgresDurability;
use crate::store::{DURABLE_DETAIL, PostgresJournalStore, RELAXED_DETAIL};

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

    /// Stub pending Task 5 (load + chain verification, spec D9).
    fn load(&self, _request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "postgres_not_implemented",
            })
        })
    }

    /// Stub pending Task 6 (snapshot writes).
    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "postgres_not_implemented",
            })
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
