use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use finstack_ai_kernel::{AppendRequest, CommittedBatch, SessionId};
use finstack_ai_runtime::{
    LoadRequest, LoadedSession, MetadataReceipt, PruneReceipt, PruneRequest, SCAN_PAGE_MAX_RECORDS,
    ScanPage, ScanRequest, SnapshotReceipt, SnapshotRequest, StateSnapshotRequest, StoreError,
    WriteMetadataRequest,
};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::config::{SqliteStoreConfig, SqliteStoreLimits, health_label, is_memory_path};
use crate::error::{i64_from_u64, map_sqlite_error};
use crate::load::{
    accelerated_from, encode_state_request, load_session, load_session_records, load_session_row,
    load_snapshot, outstanding_count, session_exists, tombstone_count, verify_stored_session,
};
use crate::schema::{apply_durability, apply_schema, quick_check};

/// File-backed or in-memory sqlite journal with one owned connection.
///
/// Records are append-only and authoritative. Optional
/// [`JournalStore::prune`](finstack_ai_runtime::JournalStore::prune) deletes
/// snapshot-covered prefix records while retaining the snapshot-boundary
/// record, outstanding tail, and settlement indexes.
pub struct SqliteJournalStore {
    path: PathBuf,
    pub(crate) limits: SqliteStoreLimits,
    pub(crate) durable: bool,
    pub(crate) detail: &'static str,
    inner: Mutex<Connection>,
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
    /// unsupported `user_version` and `PRAGMA quick_check` errors.
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
        let connection = if memory {
            Connection::open_in_memory().map_err(map_sqlite_error)?
        } else {
            Connection::open(&config.path).map_err(map_sqlite_error)?
        };
        connection
            .busy_timeout(config.busy_timeout)
            .map_err(map_sqlite_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(map_sqlite_error)?;
        apply_durability(&connection, config.durability, memory)?;
        apply_schema(&connection)?;
        quick_check(&connection)?;
        Ok(Self {
            path: config.path,
            limits,
            durable,
            detail,
            inner: Mutex::new(connection),
        })
    }

    /// Configured database path, including `:memory:`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.inner.lock().map_err(|_| StoreError::Unavailable {
            reason_code: "sqlite_lock_poisoned",
        })
    }

    fn with_immediate<T>(
        &self,
        body: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_error)?;
        let value = body(&transaction)?;
        transaction.commit().map_err(map_sqlite_error)?;
        Ok(value)
    }

    pub(crate) fn append_sync(
        &self,
        request: &AppendRequest,
    ) -> Result<CommittedBatch, StoreError> {
        self.with_immediate(|transaction| self.append_in_transaction(transaction, request))
    }

    pub(crate) fn load_sync(&self, request: LoadRequest) -> Result<LoadedSession, StoreError> {
        let connection = self.lock()?;
        load_session(&connection, request.session_id, self.limits.snapshot_bytes)
    }

    pub(crate) fn scan_sync(&self, request: ScanRequest) -> Result<ScanPage, StoreError> {
        if request.limit == 0 {
            return Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_zero",
            });
        }
        if request.limit > SCAN_PAGE_MAX_RECORDS {
            return Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_exceeded",
            });
        }
        let connection = self.lock()?;
        if !session_exists(&connection, request.session_id)? {
            return Ok(ScanPage {
                session_id: request.session_id,
                records: Arc::from([]),
                next_sequence: None,
            });
        }
        let stored = load_session_records(&connection, request.session_id)?;
        verify_stored_session(&connection, request.session_id, &stored)?;
        let start = if request.from_sequence == 0 {
            1
        } else {
            request.from_sequence
        };
        let matched = stored
            .iter()
            .map(|row| row.envelope.clone())
            .filter(|record| record.sequence() >= start)
            .collect::<Vec<_>>();
        let limit = usize::try_from(request.limit).expect("u32 fits usize");
        let records = matched.iter().take(limit).cloned().collect::<Vec<_>>();
        let next_sequence = if matched.len() > records.len() {
            records
                .last()
                .and_then(|record| record.sequence().checked_add(1))
        } else {
            None
        };
        Ok(ScanPage {
            session_id: request.session_id,
            records: records.into(),
            next_sequence,
        })
    }

    pub(crate) fn write_metadata_sync(
        &self,
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

    pub(crate) fn write_snapshot_sync(
        &self,
        request: &SnapshotRequest,
    ) -> Result<SnapshotReceipt, StoreError> {
        if request.snapshot.bytes().len() > self.limits.snapshot_bytes {
            return Err(StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: self.limits.snapshot_bytes,
            });
        }
        self.with_immediate(|transaction| {
            let session = load_session_row(transaction, request.session_id)?.ok_or(
                StoreError::InvalidRequest {
                    reason_code: "snapshot_session_not_found",
                },
            )?;
            if request.snapshot.sequence() > session.current_sequence {
                return Err(StoreError::InvalidRequest {
                    reason_code: "snapshot_ahead_of_journal",
                });
            }
            if session
                .snapshot_sequence
                .is_some_and(|current| current > request.snapshot.sequence())
            {
                return Err(StoreError::InvalidRequest {
                    reason_code: "snapshot_sequence_regression",
                });
            }
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

    /// Current database page count on the owned connection. Used by PR-040 disk-full tests.
    #[doc(hidden)]
    pub fn page_count(&self) -> Result<i64, StoreError> {
        let connection = self.lock()?;
        connection
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(map_sqlite_error)
    }

    /// Cap the database page count on the owned connection. Used by PR-040 disk-full tests.
    #[doc(hidden)]
    pub fn set_max_page_count(&self, pages: i64) -> Result<(), StoreError> {
        let connection = self.lock()?;
        connection
            .pragma_update(None, "max_page_count", pages)
            .map_err(map_sqlite_error)
    }

    /// Switch off WAL so `max_page_count` can raise `SQLITE_FULL` on the next write.
    #[doc(hidden)]
    pub fn use_rollback_journal_for_test(&self) -> Result<(), StoreError> {
        let connection = self.lock()?;
        connection
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(map_sqlite_error)?;
        let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        Ok(())
    }

    /// Insert a padding blob on the owned connection. Used to exhaust free pages.
    #[doc(hidden)]
    pub fn insert_padding_blob(&self, bytes: usize) -> Result<(), StoreError> {
        let connection = self.lock()?;
        connection
            .execute_batch("CREATE TABLE IF NOT EXISTS padding (blob BLOB)")
            .map_err(map_sqlite_error)?;
        connection
            .execute(
                "INSERT INTO padding (blob) VALUES (?1)",
                params![vec![0_u8; bytes]],
            )
            .map_err(map_sqlite_error)?;
        Ok(())
    }

    /// Persist a batch's rows, then `ROLLBACK`. Used by PR-040-A01.
    #[doc(hidden)]
    pub fn append_then_rollback(&self, request: &AppendRequest) -> Result<(), StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_error)?;
        self.append_in_transaction(&transaction, request)?;
        transaction.rollback().map_err(map_sqlite_error)
    }

    pub(crate) fn write_state_snapshot_sync(
        &self,
        request: &StateSnapshotRequest,
    ) -> Result<SnapshotReceipt, StoreError> {
        let snapshot = encode_state_request(request, self.limits.snapshot_bytes)?;
        self.write_snapshot_sync(&SnapshotRequest {
            session_id: request.session_id,
            snapshot,
        })
    }

    /// Drop the disposable snapshot cache for one session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidRequest`] when the session is missing.
    pub fn discard_snapshot(&self, session_id: SessionId) -> Result<(), StoreError> {
        self.with_immediate(|transaction| {
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
    }

    pub(crate) fn prune_sync(&self, request: PruneRequest) -> Result<PruneReceipt, StoreError> {
        self.with_immediate(|transaction| {
            let session = load_session_row(transaction, request.session_id)?.ok_or(
                StoreError::InvalidRequest {
                    reason_code: "prune_session_not_found",
                },
            )?;
            let snapshot =
                load_snapshot(transaction, request.session_id, self.limits.snapshot_bytes)?.ok_or(
                    StoreError::InvalidRequest {
                        reason_code: "prune_requires_snapshot",
                    },
                )?;
            if snapshot.sequence() == 0 || snapshot.sequence() > session.current_sequence {
                return Err(StoreError::InvalidRequest {
                    reason_code: "prune_snapshot_not_aligned",
                });
            }
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
