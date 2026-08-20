use std::path::{Path, PathBuf};

use finstack_ai_kernel::{AppendRequest, CommittedBatch, SessionId};
use finstack_ai_runtime::{
    LoadFromRequest, LoadRequest, LoadedSession, MetadataReceipt, PruneReceipt, PruneRequest,
    ScanPage, ScanRequest, SnapshotReceipt, SnapshotRequest, StateSnapshotRequest, StoreError,
    WriteMetadataRequest,
};
use finstack_ai_store_common::{admit_prune_snapshot, admit_snapshot_sequence, check_snapshot_size};
use rusqlite::{Transaction, TransactionBehavior, params};

use crate::append::append_in_transaction;
use crate::config::{SqliteStoreConfig, health_label, is_memory_path};
use crate::error::{i64_from_u64, map_sqlite_error};
use crate::load::{
    VerifiedHead, accelerated_from, encode_state_request, load_session, load_session_row,
    load_session_window, load_snapshot, outstanding_count, scan_session, tombstone_count,
};
use crate::worker::{WorkerCtx, WorkerHandle};

/// File-backed or in-memory sqlite journal with one dedicated worker thread.
///
/// The worker owns the connection and applies jobs in submit order. Records
/// are append-only and authoritative. Optional
/// [`JournalStore::prune`](finstack_ai_runtime::JournalStore::prune) deletes
/// snapshot-covered prefix records while retaining the snapshot-boundary
/// record, outstanding tail, and settlement indexes.
pub struct SqliteJournalStore {
    path: PathBuf,
    pub(crate) durable: bool,
    pub(crate) detail: &'static str,
    pub(crate) worker: WorkerHandle,
}

impl SqliteJournalStore {
    /// Open or create a store, apply one-way v1 schema, and verify the file.
    ///
    /// Copy `*.sqlite`, `*-wal`, and `*-shm` together before any later
    /// migrator. See the crate README for backup guidance.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] for zero limits, Durable on an
    /// in-memory path, or a zero busy timeout. Integrity failures include
    /// unsupported `user_version` and `PRAGMA quick_check` errors. Worker
    /// spawn or open failures return [`StoreError::Unavailable`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use std::time::Duration;
    ///
    /// use finstack_ai_store_sqlite::{
    ///     SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
    ///     SqliteSynchronous,
    /// };
    ///
    /// let store = SqliteJournalStore::try_open(SqliteStoreConfig {
    ///     path: PathBuf::from(":memory:"),
    ///     durability: SqliteDurability::Relaxed {
    ///         synchronous: SqliteSynchronous::Normal,
    ///     },
    ///     limits: SqliteStoreLimits {
    ///         sessions: 4,
    ///         batches_per_session: 8,
    ///         records_per_session: 16,
    ///         snapshot_bytes: 1024,
    ///     },
    ///     busy_timeout: Duration::from_millis(1_000),
    /// })
    /// .expect("open");
    /// drop(store);
    /// ```
    pub fn try_open(config: SqliteStoreConfig) -> Result<Self, StoreError> {
        let limits = config.limits.validate()?;
        if config.busy_timeout.is_zero() {
            return Err(StoreError::InvalidRequest {
                reason_code: "zero_sqlite_busy_timeout",
            });
        }
        let memory = is_memory_path(&config.path);
        let (durable, detail) = health_label(config.durability, memory)?;
        let path = config.path.clone();
        let mut opened = config;
        opened.limits = limits;
        Ok(Self {
            path,
            durable,
            detail,
            worker: WorkerHandle::spawn(opened)?,
        })
    }

    /// Configured database path, including `:memory:`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current database page count on the owned connection. Used by PR-040 disk-full tests.
    #[doc(hidden)]
    pub fn page_count(&self) -> Result<i64, StoreError> {
        self.worker.call(|ctx| {
            ctx.connection
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .map_err(map_sqlite_error)
        })
    }

    /// Cap the database page count on the owned connection. Used by PR-040 disk-full tests.
    #[doc(hidden)]
    pub fn set_max_page_count(&self, pages: i64) -> Result<(), StoreError> {
        self.worker.call(move |ctx| {
            ctx.connection
                .pragma_update(None, "max_page_count", pages)
                .map_err(map_sqlite_error)
        })
    }

    /// Switch off WAL so `max_page_count` can raise `SQLITE_FULL` on the next write.
    #[doc(hidden)]
    pub fn use_rollback_journal_for_test(&self) -> Result<(), StoreError> {
        self.worker.call(|ctx| {
            ctx.connection
                .pragma_update(None, "journal_mode", "DELETE")
                .map_err(map_sqlite_error)?;
            let _ = ctx
                .connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
            Ok(())
        })
    }

    /// Insert a padding blob on the owned connection. Used to exhaust free pages.
    #[doc(hidden)]
    pub fn insert_padding_blob(&self, bytes: usize) -> Result<(), StoreError> {
        self.worker.call(move |ctx| {
            ctx.connection
                .execute_batch("CREATE TABLE IF NOT EXISTS padding (blob BLOB)")
                .map_err(map_sqlite_error)?;
            ctx.connection
                .execute(
                    "INSERT INTO padding (blob) VALUES (?1)",
                    params![vec![0_u8; bytes]],
                )
                .map_err(map_sqlite_error)?;
            Ok(())
        })
    }

    /// Persist a batch's rows, then `ROLLBACK`. Used by PR-040-A01.
    #[doc(hidden)]
    pub fn append_then_rollback(&self, request: &AppendRequest) -> Result<(), StoreError> {
        let request = request.clone();
        self.worker.call(move |ctx| {
            let transaction = ctx
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_error)?;
            append_in_transaction(&transaction, &request, &ctx.limits)?;
            transaction.rollback().map_err(map_sqlite_error)
        })
    }

    /// Drop the disposable snapshot cache for one session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] when the session is missing.
    pub fn discard_snapshot(&self, session_id: SessionId) -> Result<(), StoreError> {
        self.worker.call(move |ctx| {
            ctx.with_immediate(|transaction| {
                if load_session_row(transaction, session_id)?.is_none() {
                    return Err(StoreError::InvalidRequest {
                        reason_code: "snapshot_session_not_found",
                    });
                }
                transaction
                    .execute(
                        "DELETE FROM snapshots WHERE session_id = ?1",
                        params![session_id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;
                transaction
                    .execute(
                        "UPDATE sessions SET snapshot_sequence = NULL WHERE session_id = ?1",
                        params![session_id.as_bytes().as_slice()],
                    )
                    .map_err(map_sqlite_error)?;
                Ok(())
            })
        })
    }
}

impl WorkerCtx {
    pub(crate) fn with_immediate<T>(
        &mut self,
        body: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_error)?;
        let value = body(&transaction)?;
        transaction.commit().map_err(map_sqlite_error)?;
        Ok(value)
    }

    pub(crate) fn remember(&mut self, session_id: SessionId, head: VerifiedHead) {
        self.verified.insert(session_id, head);
    }

    pub(crate) fn invalidate(&mut self, session_id: SessionId) {
        self.verified.remove(&session_id);
    }

    pub(crate) fn cached(&self, session_id: SessionId) -> Option<VerifiedHead> {
        self.verified.get(&session_id).copied()
    }

    pub(crate) fn append(&mut self, request: &AppendRequest) -> Result<CommittedBatch, StoreError> {
        let session_id = request.session_id();
        let previous = self.cached(session_id);
        let limits = self.limits;
        let committed = self
            .with_immediate(|transaction| append_in_transaction(transaction, request, &limits))?;
        self.invalidate(session_id);
        let genesis = committed.first_sequence == 1;
        let prior_verified = previous.is_some_and(|head| {
            head.sequence.saturating_add(1) == committed.first_sequence
                && committed
                    .records
                    .first()
                    .is_some_and(|record| record.previous_checksum() == head.checksum)
        });
        if (genesis || prior_verified)
            && let Some(last) = committed.records.last()
        {
            self.remember(
                session_id,
                VerifiedHead {
                    sequence: last.sequence(),
                    checksum: Some(last.checksum()),
                },
            );
        }
        Ok(committed)
    }

    pub(crate) fn load(&mut self, request: LoadRequest) -> Result<LoadedSession, StoreError> {
        let loaded = load_session(
            &self.connection,
            request.session_id,
            self.limits.snapshot_bytes,
            self.cached(request.session_id),
        )?;
        self.remember(
            request.session_id,
            VerifiedHead {
                sequence: loaded.head_sequence,
                checksum: loaded.head_checksum,
            },
        );
        Ok(loaded)
    }

    pub(crate) fn load_from(
        &mut self,
        request: LoadFromRequest,
    ) -> Result<LoadedSession, StoreError> {
        let loaded = load_session_window(
            &self.connection,
            request.session_id,
            self.limits.snapshot_bytes,
            request.window,
            self.cached(request.session_id),
        )?;
        self.remember(
            request.session_id,
            VerifiedHead {
                sequence: loaded.head_sequence,
                checksum: loaded.head_checksum,
            },
        );
        Ok(loaded)
    }

    pub(crate) fn scan(&mut self, request: ScanRequest) -> Result<ScanPage, StoreError> {
        scan_session(&self.connection, request)
    }

    pub(crate) fn write_metadata(
        &mut self,
        request: WriteMetadataRequest,
    ) -> Result<MetadataReceipt, StoreError> {
        self.with_immediate(|transaction| {
            let session = load_session_row(transaction, request.session_id)?.ok_or(
                StoreError::InvalidRequest {
                    reason_code: "metadata_session_not_found",
                },
            )?;
            if session.head_checksum != request.expected_head_checksum {
                return Err(StoreError::InvalidRequest {
                    reason_code: "metadata_cas_mismatch",
                });
            }
            transaction
                .execute(
                    "UPDATE sessions SET metadata = ?1 WHERE session_id = ?2",
                    params![
                        request.metadata.as_bytes(),
                        request.session_id.as_bytes().as_slice()
                    ],
                )
                .map_err(map_sqlite_error)?;
            Ok(MetadataReceipt {
                session_id: request.session_id,
                head_checksum: session.head_checksum,
                metadata: request.metadata,
            })
        })
    }

    pub(crate) fn write_snapshot(
        &mut self,
        request: &SnapshotRequest,
    ) -> Result<SnapshotReceipt, StoreError> {
        check_snapshot_size(request.snapshot.bytes().len(), self.limits.snapshot_bytes)?;
        self.with_immediate(|transaction| {
            let session = load_session_row(transaction, request.session_id)?.ok_or(
                StoreError::InvalidRequest {
                    reason_code: "snapshot_session_not_found",
                },
            )?;
            admit_snapshot_sequence(
                request.snapshot.sequence(),
                session.current_sequence,
                session.snapshot_sequence,
            )?;
            let receipt = SnapshotReceipt {
                session_id: request.session_id,
                sequence: request.snapshot.sequence(),
                digest: request.snapshot.digest(),
                bytes: request.snapshot.bytes().len(),
            };
            transaction
                .execute(
                    "INSERT OR REPLACE INTO snapshots
                     (session_id, sequence, payload_cbor, digest, timestamp)
                     VALUES (?1, ?2, ?3, ?4, 0)",
                    params![
                        request.session_id.as_bytes().as_slice(),
                        i64_from_u64(request.snapshot.sequence(), "snapshot_sequence")?,
                        request.snapshot.bytes(),
                        request.snapshot.digest().as_bytes().as_slice(),
                    ],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "UPDATE sessions SET snapshot_sequence = ?1 WHERE session_id = ?2",
                    params![
                        i64_from_u64(request.snapshot.sequence(), "snapshot_sequence")?,
                        request.session_id.as_bytes().as_slice(),
                    ],
                )
                .map_err(map_sqlite_error)?;
            Ok(receipt)
        })
    }

    pub(crate) fn write_state_snapshot(
        &mut self,
        request: &StateSnapshotRequest,
    ) -> Result<SnapshotReceipt, StoreError> {
        let snapshot = encode_state_request(request, self.limits.snapshot_bytes)?;
        self.write_snapshot(&SnapshotRequest {
            session_id: request.session_id,
            snapshot,
        })
    }

    pub(crate) fn prune(&mut self, request: PruneRequest) -> Result<PruneReceipt, StoreError> {
        self.invalidate(request.session_id);
        let snapshot_bytes = self.limits.snapshot_bytes;
        self.with_immediate(|transaction| {
            let session = load_session_row(transaction, request.session_id)?.ok_or(
                StoreError::InvalidRequest {
                    reason_code: "prune_session_not_found",
                },
            )?;
            let snapshot = load_snapshot(transaction, request.session_id, snapshot_bytes)?.ok_or(
                StoreError::InvalidRequest {
                    reason_code: "prune_requires_snapshot",
                },
            )?;
            admit_prune_snapshot(snapshot.sequence(), session.current_sequence)?;
            // Alignment rule twin: the memory store enforces the same
            // "snapshot ends exactly at a batch's last_sequence" predicate over
            // its in-memory batch list (lib.rs prune_sync); change both together.
            let aligned: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM batches
                     WHERE session_id = ?1 AND last_sequence = ?2",
                    params![
                        request.session_id.as_bytes().as_slice(),
                        i64_from_u64(snapshot.sequence(), "snapshot_sequence")?,
                    ],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_error)?;
            if aligned == 0 {
                return Err(StoreError::InvalidRequest {
                    reason_code: "prune_not_batch_aligned",
                });
            }
            let accelerated = accelerated_from(&snapshot).ok_or(StoreError::Integrity {
                reason_code: "prune_snapshot_undecodable",
            })?;
            let retained_outstanding = outstanding_count(&accelerated);
            let retained_tombstones = tombstone_count(&accelerated);
            let pruned_through = i64_from_u64(snapshot.sequence(), "snapshot_sequence")?;
            transaction
                .execute(
                    "DELETE FROM records WHERE session_id = ?1 AND sequence < ?2",
                    params![request.session_id.as_bytes().as_slice(), pruned_through],
                )
                .map_err(map_sqlite_error)?;
            transaction
                .execute(
                    "DELETE FROM batches WHERE session_id = ?1 AND last_sequence < ?2",
                    params![request.session_id.as_bytes().as_slice(), pruned_through],
                )
                .map_err(map_sqlite_error)?;
            let _ = request.horizon;
            Ok(PruneReceipt {
                pruned_through_sequence: snapshot.sequence(),
                retained_outstanding,
                retained_tombstones,
            })
        })
    }
}
