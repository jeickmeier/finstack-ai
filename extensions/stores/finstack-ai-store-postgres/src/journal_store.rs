//! [`JournalStore`] implementation for [`PostgresJournalStore`].
//!
//! Only [`JournalStore::health`] is implemented in this task. `append`,
//! `load`, and `write_snapshot` are non-default methods on the trait (the
//! port has no default body for them), so they need *some* implementation
//! to compile; Tasks 4–6 replace these stubs with the real multi-writer
//! transaction logic (spec D4/D9) and store-common snapshot wiring. Every
//! other trait method (`scan`, `write_metadata`, `write_state_snapshot`,
//! `prune`, `load_from`) keeps the port's own default implementation for
//! now.

use std::sync::Arc;

use finstack_ai_kernel::{AppendRequest, CommittedBatch};
use finstack_ai_runtime::{
    JournalStore, LoadRequest, LoadedSession, PortFuture, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth,
};

use crate::config::PostgresDurability;
use crate::store::{DURABLE_DETAIL, PostgresJournalStore, RELAXED_DETAIL};

impl JournalStore for PostgresJournalStore {
    /// Stub pending Task 4 (multi-writer append transaction, spec D4).
    fn append(&self, _request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "postgres_not_implemented",
            })
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
