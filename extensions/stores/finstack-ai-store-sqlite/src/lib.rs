//! Durable local sqlite [`JournalStore`] implementation (TDD §18.2 / PR-040).
//!
//! WAL plus `synchronous=FULL` is the only acknowledged durable mode.
//! Records stay append-only; snapshots are a disposable cache. Applications
//! inject this crate explicitly. It is not the Agent, Python, or WASM default.

#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use finstack_ai_kernel::{
    AppendBatchId, AppendRequest, CommittedBatch, Digest, EventId, Id, IdTag, Metadata, RecordBody,
    RecordEnvelope, RecordId, SessionId, Timestamp,
};
use finstack_ai_protocol::{
    ProtocolError, commit_records, decode, decode_opaque_snapshot, encode, encode_snapshot,
    verify_chain, verify_chain_from,
};
use finstack_ai_runtime::{
    AcceleratedRestore, JournalStore, LoadRequest, LoadedSession, MetadataReceipt, OpaqueSnapshot,
    PortFuture, PruneReceipt, PruneRequest, SCAN_PAGE_MAX_RECORDS, ScanPage, ScanRequest,
    SnapshotReceipt, SnapshotRequest, StateSnapshotRequest, StoreError, StoreHealth, StoreLimits,
    WriteMetadataRequest,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

/// Applied `PRAGMA user_version` for the v1 schema. Other versions fail closed.
pub const SCHEMA_USER_VERSION: i32 = 1;

/// Default lock-wait used when a second connection contends for the writer.
pub const DEFAULT_BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// Required resource ceilings for [`SqliteJournalStore`].
pub type SqliteStoreLimits = StoreLimits;

/// Sqlite `synchronous` setting for explicitly labeled relaxed mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteSynchronous {
    /// `PRAGMA synchronous=NORMAL`.
    Normal,
    /// `PRAGMA synchronous=OFF`.
    Off,
}

/// Durability policy applied at open and advertised by [`JournalStore::health`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteDurability {
    /// WAL plus `synchronous=FULL` (and Darwin full-fsync controls).
    ///
    /// This is the only mode that may set `health().durable = true`. It still
    /// depends on the OS and filesystem honoring those flush settings.
    Durable,
    /// Named non-durable mode. Must never advertise NFR-REL-001.
    Relaxed {
        /// Relaxed `synchronous` pragma.
        synchronous: SqliteSynchronous,
    },
}

/// Open configuration for [`SqliteJournalStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteStoreConfig {
    /// Database path. `:memory:` is allowed only with [`SqliteDurability::Relaxed`].
    pub path: PathBuf,
    /// Durability / health labeling.
    pub durability: SqliteDurability,
    /// Resource ceilings.
    pub limits: SqliteStoreLimits,
    /// `SQLITE_BUSY` wait before `sqlite_busy`.
    pub busy_timeout: Duration,
}

impl SqliteStoreConfig {
    /// Construct a config using [`DEFAULT_BUSY_TIMEOUT`].
    #[must_use]
    pub fn new(
        path: impl Into<PathBuf>,
        durability: SqliteDurability,
        limits: SqliteStoreLimits,
    ) -> Self {
        Self {
            path: path.into(),
            durability,
            limits,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        }
    }
}

/// File-backed or in-memory sqlite journal with one owned connection.
///
/// Records are append-only and authoritative. Optional
/// [`JournalStore::prune`](finstack_ai_runtime::JournalStore::prune) deletes
/// snapshot-covered prefix records while retaining the snapshot-boundary
/// record, outstanding tail, and settlement indexes.
pub struct SqliteJournalStore {
    path: PathBuf,
    limits: SqliteStoreLimits,
    durable: bool,
    detail: &'static str,
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

    fn append_in_transaction(
        &self,
        transaction: &Transaction<'_>,
        request: &AppendRequest,
    ) -> Result<CommittedBatch, StoreError> {
        let request_cbor = request_cbor(request)?;
        if let Some(existing) = load_batch(transaction, request.batch_id())? {
            return if existing.request_cbor == request_cbor {
                Ok(existing.committed)
            } else {
                Err(StoreError::Corruption {
                    reason_code: "append_batch_id_reuse",
                })
            };
        }

        if request.records().is_empty() {
            return Err(StoreError::InvalidRequest {
                reason_code: "empty_append_batch",
            });
        }

        let mut reused = Vec::new();
        for record in request.records() {
            if let Some(batch_id) = lookup_record_batch(transaction, record.record_id())? {
                reused.push(batch_id);
            }
        }
        if !reused.is_empty() {
            if reused.len() != request.records().len() {
                return Err(StoreError::Corruption {
                    reason_code: "mixed_record_id_reuse",
                });
            }
            let original_batch_id = reused[0];
            if reused.iter().any(|batch_id| *batch_id != original_batch_id) {
                return Err(StoreError::Corruption {
                    reason_code: "mixed_record_batch_reuse",
                });
            }
            let existing =
                load_batch(transaction, original_batch_id)?.ok_or(StoreError::Integrity {
                    reason_code: "missing_record_batch_index",
                })?;
            let incoming = request_identity(request)?;
            if existing.identity.session_id == incoming.session_id
                && existing.identity.expected_sequence == incoming.expected_sequence
                && existing.identity.draft_cbor == incoming.draft_cbor
            {
                return Ok(existing.committed);
            }
            return Err(StoreError::Corruption {
                reason_code: "record_id_reuse",
            });
        }

        let session = load_session_row(transaction, request.session_id())?;
        let current_head = session.as_ref().map_or(0, |row| row.current_sequence);
        let actual_next_sequence = current_head.checked_add(1).ok_or(StoreError::Integrity {
            reason_code: "sequence_exhausted",
        })?;
        if request.expected_sequence() != actual_next_sequence {
            return Err(StoreError::Conflict {
                expected_sequence: request.expected_sequence(),
                actual_next_sequence,
            });
        }

        let session_is_new = session.is_none();
        if session_is_new && count_sessions(transaction)? >= self.limits.sessions {
            return Err(StoreError::LimitExceeded {
                resource: "sessions",
                limit: self.limits.sessions,
            });
        }
        if let Some(row) = &session {
            if row.batches >= self.limits.batches_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "batches_per_session",
                    limit: self.limits.batches_per_session,
                });
            }
            if row.records + request.records().len() > self.limits.records_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "records_per_session",
                    limit: self.limits.records_per_session,
                });
            }
        } else if request.records().len() > self.limits.records_per_session {
            return Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: self.limits.records_per_session,
            });
        }

        let previous_checksum = session.as_ref().and_then(|row| row.head_checksum);
        let committed = build_committed_batch(request, previous_checksum)?;
        persist_committed_batch(
            transaction,
            request,
            &request_cbor,
            &committed,
            session_is_new,
        )?;
        Ok(committed)
    }

    fn append_sync(&self, request: &AppendRequest) -> Result<CommittedBatch, StoreError> {
        self.with_immediate(|transaction| self.append_in_transaction(transaction, request))
    }

    fn load_sync(&self, request: LoadRequest) -> Result<LoadedSession, StoreError> {
        let connection = self.lock()?;
        load_session(&connection, request.session_id, self.limits.snapshot_bytes)
    }

    fn scan_sync(&self, request: ScanRequest) -> Result<ScanPage, StoreError> {
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

    fn write_metadata_sync(
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

    fn write_snapshot_sync(
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

    fn write_state_snapshot_sync(
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

    fn prune_sync(&self, request: PruneRequest) -> Result<PruneReceipt, StoreError> {
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

impl JournalStore for SqliteJournalStore {
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let result = self.append_sync(&request);
        Box::pin(async move { result })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let result = self.load_sync(request);
        Box::pin(async move { result })
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let result = self.write_snapshot_sync(&request);
        Box::pin(async move { result })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        let ready = true;
        let durable = self.durable;
        let detail = Arc::from(self.detail);
        Box::pin(async move {
            Ok(StoreHealth {
                ready,
                durable,
                detail,
            })
        })
    }

    fn scan(&self, request: ScanRequest) -> PortFuture<Result<ScanPage, StoreError>> {
        let result = self.scan_sync(request);
        Box::pin(async move { result })
    }

    fn write_metadata(
        &self,
        request: WriteMetadataRequest,
    ) -> PortFuture<Result<MetadataReceipt, StoreError>> {
        let result = self.write_metadata_sync(request);
        Box::pin(async move { result })
    }

    fn write_state_snapshot(
        &self,
        request: StateSnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        let result = self.write_state_snapshot_sync(&request);
        Box::pin(async move { result })
    }

    fn prune(&self, request: PruneRequest) -> PortFuture<Result<PruneReceipt, StoreError>> {
        let result = self.prune_sync(request);
        Box::pin(async move { result })
    }
}

const V1_DDL: &str = "
CREATE TABLE sessions (
  session_id BLOB PRIMARY KEY,
  current_sequence INTEGER NOT NULL,
  head_checksum BLOB,
  snapshot_sequence INTEGER,
  metadata BLOB NOT NULL
);
CREATE TABLE batches (
  batch_id BLOB PRIMARY KEY,
  session_id BLOB NOT NULL REFERENCES sessions(session_id),
  first_sequence INTEGER NOT NULL,
  last_sequence INTEGER NOT NULL,
  expected_sequence INTEGER NOT NULL,
  request_cbor BLOB NOT NULL,
  UNIQUE (session_id, first_sequence)
);
CREATE TABLE records (
  session_id BLOB NOT NULL,
  sequence INTEGER NOT NULL,
  record_id BLOB NOT NULL UNIQUE,
  batch_id BLOB NOT NULL REFERENCES batches(batch_id),
  lane_id BLOB NOT NULL,
  run_id BLOB,
  kind TEXT NOT NULL,
  format_version INTEGER NOT NULL,
  kind_version INTEGER NOT NULL,
  payload_cbor BLOB NOT NULL,
  timestamp INTEGER NOT NULL,
  committed_at INTEGER,
  payload_digest BLOB NOT NULL,
  previous_checksum BLOB,
  envelope_checksum BLOB NOT NULL,
  derived_event_ids BLOB NOT NULL,
  PRIMARY KEY (session_id, sequence)
);
CREATE TABLE snapshots (
  session_id BLOB PRIMARY KEY,
  sequence INTEGER NOT NULL,
  payload_cbor BLOB NOT NULL,
  digest BLOB NOT NULL,
  timestamp INTEGER NOT NULL
);
CREATE INDEX records_batch_id ON records(batch_id);
CREATE INDEX batches_session_first ON batches(session_id, first_sequence);
";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AppendIdentity {
    batch_id: [u8; 16],
    session_id: [u8; 16],
    expected_sequence: u64,
    draft_cbor: Vec<Vec<u8>>,
}

struct LoadedBatch {
    request_cbor: Vec<u8>,
    identity: AppendIdentity,
    committed: CommittedBatch,
}

struct SessionRow {
    current_sequence: u64,
    head_checksum: Option<Digest>,
    snapshot_sequence: Option<u64>,
    batches: usize,
    records: usize,
}

struct StoredRecord {
    batch_id: AppendBatchId,
    envelope: RecordEnvelope,
}

fn is_memory_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text == ":memory:" || text.contains("mode=memory")
}

fn health_label(
    durability: SqliteDurability,
    memory: bool,
) -> Result<(bool, &'static str), StoreError> {
    match durability {
        SqliteDurability::Durable => {
            if memory {
                return Err(StoreError::InvalidRequest {
                    reason_code: "sqlite_durable_requires_file",
                });
            }
            Ok((true, "sqlite_durable_wal_full"))
        }
        SqliteDurability::Relaxed { synchronous } => {
            let detail = if memory {
                "sqlite_relaxed_in_memory"
            } else {
                match synchronous {
                    SqliteSynchronous::Normal => "sqlite_relaxed_synchronous_normal",
                    SqliteSynchronous::Off => "sqlite_relaxed_synchronous_off",
                }
            };
            Ok((false, detail))
        }
    }
}

fn apply_durability(
    connection: &Connection,
    durability: SqliteDurability,
    memory: bool,
) -> Result<(), StoreError> {
    match durability {
        SqliteDurability::Durable => {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(map_sqlite_error)?;
            connection
                .pragma_update(None, "synchronous", "FULL")
                .map_err(map_sqlite_error)?;
            #[cfg(target_os = "macos")]
            {
                connection
                    .pragma_update(None, "fullfsync", "ON")
                    .map_err(map_sqlite_error)?;
                connection
                    .pragma_update(None, "checkpoint_fullfsync", "ON")
                    .map_err(map_sqlite_error)?;
            }
        }
        SqliteDurability::Relaxed { synchronous } => {
            if !memory {
                connection
                    .pragma_update(None, "journal_mode", "WAL")
                    .map_err(map_sqlite_error)?;
            }
            let value = match synchronous {
                SqliteSynchronous::Normal => "NORMAL",
                SqliteSynchronous::Off => "OFF",
            };
            connection
                .pragma_update(None, "synchronous", value)
                .map_err(map_sqlite_error)?;
        }
    }
    Ok(())
}

fn apply_schema(connection: &Connection) -> Result<(), StoreError> {
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(map_sqlite_error)?;
    match version {
        0 => {
            connection.execute_batch(V1_DDL).map_err(map_sqlite_error)?;
            connection
                .pragma_update(None, "user_version", SCHEMA_USER_VERSION)
                .map_err(map_sqlite_error)?;
            Ok(())
        }
        SCHEMA_USER_VERSION => Ok(()),
        _ => Err(StoreError::Integrity {
            reason_code: "sqlite_schema_unsupported",
        }),
    }
}

fn quick_check(connection: &Connection) -> Result<(), StoreError> {
    let status: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(map_sqlite_error)?;
    if status == "ok" {
        Ok(())
    } else {
        Err(StoreError::Integrity {
            reason_code: "sqlite_quick_check",
        })
    }
}

fn request_identity(request: &AppendRequest) -> Result<AppendIdentity, StoreError> {
    let draft_cbor = request
        .records()
        .iter()
        .map(|draft| encode(draft).map_err(protocol_error))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AppendIdentity {
        batch_id: request.batch_id().to_bytes(),
        session_id: request.session_id().to_bytes(),
        expected_sequence: request.expected_sequence(),
        draft_cbor,
    })
}

fn request_cbor(request: &AppendRequest) -> Result<Vec<u8>, StoreError> {
    encode(&request_identity(request)?).map_err(protocol_error)
}

fn build_committed_batch(
    request: &AppendRequest,
    previous_checksum: Option<Digest>,
) -> Result<CommittedBatch, StoreError> {
    let records = commit_records(
        request.records(),
        request.expected_sequence(),
        previous_checksum,
        None,
    )
    .map_err(protocol_error)?;
    let last_sequence = request
        .expected_sequence()
        .checked_add(
            u64::try_from(records.len() - 1).map_err(|_| StoreError::Integrity {
                reason_code: "record_count_overflow",
            })?,
        )
        .ok_or(StoreError::Integrity {
            reason_code: "sequence_exhausted",
        })?;
    CommittedBatch::try_new(
        request.batch_id(),
        request.expected_sequence(),
        last_sequence,
        records,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "committed_batch_invalid",
    })
}

fn persist_committed_batch(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    request_cbor: &[u8],
    committed: &CommittedBatch,
    session_is_new: bool,
) -> Result<(), StoreError> {
    if session_is_new {
        transaction
            .execute(
                "INSERT INTO sessions
                 (session_id, current_sequence, head_checksum, snapshot_sequence, metadata)
                 VALUES (?1, 0, NULL, NULL, ?2)",
                params![
                    request.session_id().as_bytes().as_slice(),
                    Metadata::empty().as_bytes(),
                ],
            )
            .map_err(map_sqlite_error)?;
    }
    transaction
        .execute(
            "INSERT INTO batches
             (batch_id, session_id, first_sequence, last_sequence, expected_sequence, request_cbor)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                request.batch_id().as_bytes().as_slice(),
                request.session_id().as_bytes().as_slice(),
                i64_from_u64(committed.first_sequence, "first_sequence")?,
                i64_from_u64(committed.last_sequence, "last_sequence")?,
                i64_from_u64(request.expected_sequence(), "expected_sequence")?,
                request_cbor,
            ],
        )
        .map_err(map_sqlite_error)?;
    for envelope in committed.records.iter() {
        insert_record(transaction, request.batch_id(), envelope)?;
    }
    let head_checksum = committed
        .records
        .last()
        .map(RecordEnvelope::checksum)
        .ok_or(StoreError::Integrity {
            reason_code: "empty_committed_batch",
        })?;
    let previous_sequence =
        request
            .expected_sequence()
            .checked_sub(1)
            .ok_or(StoreError::Integrity {
                reason_code: "sequence_exhausted",
            })?;
    let updated = transaction
        .execute(
            "UPDATE sessions
             SET current_sequence = ?1, head_checksum = ?2
             WHERE session_id = ?3 AND current_sequence = ?4",
            params![
                i64_from_u64(committed.last_sequence, "last_sequence")?,
                head_checksum.as_bytes().as_slice(),
                request.session_id().as_bytes().as_slice(),
                i64_from_u64(previous_sequence, "previous_sequence")?,
            ],
        )
        .map_err(map_sqlite_error)?;
    if updated != 1 {
        return Err(StoreError::Integrity {
            reason_code: "sequence_cas_failed",
        });
    }
    Ok(())
}

fn insert_record(
    transaction: &Transaction<'_>,
    batch_id: AppendBatchId,
    envelope: &RecordEnvelope,
) -> Result<(), StoreError> {
    let payload_cbor = encode(envelope.body()).map_err(protocol_error)?;
    let derived_event_ids = encode(&envelope.derived_event_ids()).map_err(protocol_error)?;
    let run_id = envelope.run_id().map(|id| id.to_bytes());
    let previous = envelope
        .previous_checksum()
        .map(|digest| *digest.as_bytes());
    transaction
        .execute(
            "INSERT INTO records (
                session_id, sequence, record_id, batch_id, lane_id, run_id, kind,
                format_version, kind_version, payload_cbor, timestamp, committed_at,
                payload_digest, previous_checksum, envelope_checksum, derived_event_ids
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?13, ?14, ?15)",
            params![
                envelope.session_id().as_bytes().as_slice(),
                i64_from_u64(envelope.sequence(), "sequence")?,
                envelope.record_id().as_bytes().as_slice(),
                batch_id.as_bytes().as_slice(),
                envelope.lane_id().as_bytes().as_slice(),
                run_id.as_ref().map(<[u8; 16]>::as_slice),
                envelope.body().kind_name(),
                i64::from(envelope.format_version()),
                i64::from(envelope.kind_version()),
                payload_cbor,
                envelope.timestamp().as_unix_ms(),
                envelope.payload_digest().as_bytes().as_slice(),
                previous.as_ref().map(<[u8; 32]>::as_slice),
                envelope.checksum().as_bytes().as_slice(),
                derived_event_ids,
            ],
        )
        .map_err(map_sqlite_error)?;
    Ok(())
}

fn load_batch(
    connection: &Connection,
    batch_id: AppendBatchId,
) -> Result<Option<LoadedBatch>, StoreError> {
    let Some((request_cbor, first_sequence, last_sequence)) = connection
        .query_row(
            "SELECT request_cbor, first_sequence, last_sequence FROM batches WHERE batch_id = ?1",
            params![batch_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
    else {
        return Ok(None);
    };
    let identity = decode::<AppendIdentity>(&request_cbor).map_err(protocol_error)?;
    let records = load_batch_records(connection, batch_id)?;
    let committed = CommittedBatch::try_new(
        batch_id,
        u64_from_i64(first_sequence, "first_sequence")?,
        u64_from_i64(last_sequence, "last_sequence")?,
        records,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "committed_batch_invalid",
    })?;
    Ok(Some(LoadedBatch {
        request_cbor,
        identity,
        committed,
    }))
}

fn load_batch_records(
    connection: &Connection,
    batch_id: AppendBatchId,
) -> Result<Vec<RecordEnvelope>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT session_id, sequence, record_id, lane_id, run_id, kind,
                    format_version, kind_version, payload_cbor, timestamp,
                    payload_digest, previous_checksum, envelope_checksum, derived_event_ids
             FROM records WHERE batch_id = ?1 ORDER BY sequence",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![batch_id.as_bytes().as_slice()], row_to_envelope)
        .map_err(map_sqlite_error)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite_error)?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
}

fn load_session_records(
    connection: &Connection,
    session_id: SessionId,
) -> Result<Vec<StoredRecord>, StoreError> {
    let mut statement = connection
        .prepare(
            "SELECT session_id, sequence, record_id, lane_id, run_id, kind,
                    format_version, kind_version, payload_cbor, timestamp,
                    payload_digest, previous_checksum, envelope_checksum, derived_event_ids,
                    batch_id
             FROM records WHERE session_id = ?1 ORDER BY sequence",
        )
        .map_err(map_sqlite_error)?;
    let rows = statement
        .query_map(params![session_id.as_bytes().as_slice()], |row| {
            let batch_id = row.get::<_, Vec<u8>>(14)?;
            let envelope = row_to_envelope(row)?;
            Ok((batch_id, envelope))
        })
        .map_err(map_sqlite_error)?;
    let mut stored = Vec::new();
    for row in rows {
        let (batch_bytes, envelope) = row.map_err(map_sqlite_error)?;
        stored.push(StoredRecord {
            batch_id: id_from_blob(&batch_bytes)?,
            envelope: envelope?,
        });
    }
    Ok(stored)
}

fn row_to_envelope(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<RecordEnvelope, StoreError>> {
    let session_id = row.get::<_, Vec<u8>>(0)?;
    let sequence = row.get::<_, i64>(1)?;
    let record_id = row.get::<_, Vec<u8>>(2)?;
    let lane_id = row.get::<_, Vec<u8>>(3)?;
    let run_id = row.get::<_, Option<Vec<u8>>>(4)?;
    let kind = row.get::<_, String>(5)?;
    let format_version = row.get::<_, i64>(6)?;
    let kind_version = row.get::<_, i64>(7)?;
    let payload_cbor = row.get::<_, Vec<u8>>(8)?;
    let timestamp = row.get::<_, i64>(9)?;
    let payload_digest = row.get::<_, Vec<u8>>(10)?;
    let previous_checksum = row.get::<_, Option<Vec<u8>>>(11)?;
    let envelope_checksum = row.get::<_, Vec<u8>>(12)?;
    let derived_event_ids = row.get::<_, Vec<u8>>(13)?;
    Ok(reconstruct_envelope(
        &session_id,
        sequence,
        &record_id,
        &lane_id,
        run_id.as_deref(),
        &kind,
        format_version,
        kind_version,
        &payload_cbor,
        timestamp,
        &payload_digest,
        previous_checksum.as_deref(),
        &envelope_checksum,
        &derived_event_ids,
    ))
}

#[allow(clippy::too_many_arguments)]
fn reconstruct_envelope(
    session_id: &[u8],
    sequence: i64,
    record_id: &[u8],
    lane_id: &[u8],
    run_id: Option<&[u8]>,
    kind: &str,
    format_version: i64,
    kind_version: i64,
    payload_cbor: &[u8],
    timestamp: i64,
    payload_digest: &[u8],
    previous_checksum: Option<&[u8]>,
    envelope_checksum: &[u8],
    derived_event_ids: &[u8],
) -> Result<RecordEnvelope, StoreError> {
    let body = decode::<RecordBody>(payload_cbor).map_err(protocol_error)?;
    if body.kind_name() != kind {
        return Err(StoreError::Integrity {
            reason_code: "sqlite_kind_mismatch",
        });
    }
    let events = decode::<Vec<EventId>>(derived_event_ids).map_err(protocol_error)?;
    let envelope = RecordEnvelope::try_new(
        u16_from_i64(format_version, "format_version")?,
        u16_from_i64(kind_version, "kind_version")?,
        id_from_blob(record_id)?,
        id_from_blob(session_id)?,
        id_from_blob(lane_id)?,
        run_id.map(id_from_blob).transpose()?,
        u64_from_i64(sequence, "sequence")?,
        Timestamp::from_unix_ms(timestamp).map_err(|_| StoreError::Integrity {
            reason_code: "sqlite_timestamp",
        })?,
        None,
        digest_from_blob(payload_digest)?,
        previous_checksum.map(digest_from_blob).transpose()?,
        digest_from_blob(envelope_checksum)?,
        events,
        body,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "sqlite_envelope_invalid",
    })?;
    Ok(envelope)
}

fn load_session(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
) -> Result<LoadedSession, StoreError> {
    if !session_exists(connection, session_id)? {
        return Ok(LoadedSession::empty(session_id));
    }
    let stored = load_session_records(connection, session_id)?;
    let head_checksum = verify_stored_session(connection, session_id, &stored)?;
    let committed_batches = group_batches(&stored)?;
    let (metadata, head_sequence, snapshot) =
        load_session_extras(connection, session_id, snapshot_bytes)?;
    Ok(LoadedSession {
        session_id,
        head_sequence,
        head_checksum,
        metadata,
        committed_batches: committed_batches.into(),
        snapshot: snapshot.clone(),
        accelerated: snapshot.as_ref().and_then(accelerated_from),
    })
}

fn load_session_extras(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
) -> Result<(Metadata, u64, Option<OpaqueSnapshot>), StoreError> {
    let (metadata, head_sequence, snapshot_sequence) = connection
        .query_row(
            "SELECT metadata, current_sequence, snapshot_sequence FROM sessions WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .map_err(map_sqlite_error)?;
    let metadata = Metadata::parse(metadata).map_err(|_| StoreError::Integrity {
        reason_code: "sqlite_metadata",
    })?;
    let snapshot = if snapshot_sequence.is_some() {
        load_snapshot(connection, session_id, snapshot_bytes)?
    } else {
        None
    };
    Ok((
        metadata,
        u64_from_i64(head_sequence, "current_sequence")?,
        snapshot,
    ))
}

fn load_snapshot(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
) -> Result<Option<OpaqueSnapshot>, StoreError> {
    let Some((sequence, payload, digest)) = connection
        .query_row(
            "SELECT sequence, payload_cbor, digest FROM snapshots WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
    else {
        return Ok(None);
    };
    Ok(Some(OpaqueSnapshot::try_new(
        u64_from_i64(sequence, "snapshot_sequence")?,
        digest_from_blob(&digest)?,
        payload,
        snapshot_bytes,
    )?))
}

fn encode_state_request(
    request: &StateSnapshotRequest,
    max_bytes: usize,
) -> Result<OpaqueSnapshot, StoreError> {
    let (bytes, digest) = encode_snapshot(
        &request.state,
        request.state.last_applied_sequence,
        request.head_checksum,
        request.pending_timer_scheduled_at,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "snapshot_encode_failed",
    })?;
    OpaqueSnapshot::try_new(
        request.state.last_applied_sequence,
        digest,
        bytes,
        max_bytes,
    )
}

fn accelerated_from(snapshot: &OpaqueSnapshot) -> Option<AcceleratedRestore> {
    let decoded =
        decode_opaque_snapshot(snapshot.sequence(), snapshot.digest(), snapshot.bytes()).ok()?;
    Some(AcceleratedRestore {
        sequence: decoded.sequence,
        head_checksum: decoded.head_checksum,
        pending_timer_scheduled_at: decoded.pending_timer_scheduled_at,
        state: decoded.state,
    })
}

fn outstanding_count(restored: &AcceleratedRestore) -> u64 {
    u64::from(restored.state.pending_model_effect.is_some())
        .saturating_add(u64::from(restored.state.pending_interaction.is_some()))
}

fn tombstone_count(restored: &AcceleratedRestore) -> u64 {
    u64::try_from(
        restored
            .state
            .completion_identities
            .len()
            .saturating_add(restored.state.resolution_identities.len())
            .saturating_add(restored.state.model_settlements.len())
            .saturating_add(restored.state.tool_settlements.len()),
    )
    .unwrap_or(u64::MAX)
}

fn verify_stored_session(
    connection: &Connection,
    session_id: SessionId,
    stored: &[StoredRecord],
) -> Result<Option<Digest>, StoreError> {
    let records = stored
        .iter()
        .map(|row| row.envelope.clone())
        .collect::<Vec<_>>();
    let head = match records.first() {
        Some(first) if first.sequence() > 1 => {
            verify_chain_from(&records, first.previous_checksum(), Some(first.sequence()))
                .map_err(protocol_error)?
        }
        _ => verify_chain(&records).map_err(protocol_error)?,
    };
    let stored_head = connection
        .query_row(
            "SELECT head_checksum FROM sessions WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .map_err(map_sqlite_error)?;
    let stored_head = stored_head.as_deref().map(digest_from_blob).transpose()?;
    if head != stored_head {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    Ok(head)
}

fn group_batches(stored: &[StoredRecord]) -> Result<Vec<CommittedBatch>, StoreError> {
    let mut batches = Vec::new();
    let mut index = 0;
    while index < stored.len() {
        let batch_id = stored[index].batch_id;
        let first_sequence = stored[index].envelope.sequence();
        let mut records = Vec::new();
        while index < stored.len() && stored[index].batch_id == batch_id {
            records.push(stored[index].envelope.clone());
            index += 1;
        }
        let last_sequence = records
            .last()
            .map_or(first_sequence, RecordEnvelope::sequence);
        batches.push(
            CommittedBatch::try_new(batch_id, first_sequence, last_sequence, records).map_err(
                |_| StoreError::Integrity {
                    reason_code: "committed_batch_invalid",
                },
            )?,
        );
    }
    Ok(batches)
}

fn session_exists(connection: &Connection, session_id: SessionId) -> Result<bool, StoreError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .map_err(map_sqlite_error)?;
    Ok(count > 0)
}

fn load_session_row(
    connection: &Connection,
    session_id: SessionId,
) -> Result<Option<SessionRow>, StoreError> {
    let Some((current_sequence, head_checksum, snapshot_sequence)) = connection
        .query_row(
            "SELECT current_sequence, head_checksum, snapshot_sequence
             FROM sessions WHERE session_id = ?1",
            params![session_id.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_error)?
    else {
        return Ok(None);
    };
    let batches = count_where(
        connection,
        "SELECT COUNT(*) FROM batches WHERE session_id = ?1",
        session_id,
    )?;
    let records = count_where(
        connection,
        "SELECT COUNT(*) FROM records WHERE session_id = ?1",
        session_id,
    )?;
    Ok(Some(SessionRow {
        current_sequence: u64_from_i64(current_sequence, "current_sequence")?,
        head_checksum: head_checksum.as_deref().map(digest_from_blob).transpose()?,
        snapshot_sequence: snapshot_sequence
            .map(|value| u64_from_i64(value, "snapshot_sequence"))
            .transpose()?,
        batches,
        records,
    }))
}

fn count_sessions(connection: &Connection) -> Result<usize, StoreError> {
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .map_err(map_sqlite_error)?;
    usize_from_i64(count, "sessions")
}

fn count_where(
    connection: &Connection,
    sql: &str,
    session_id: SessionId,
) -> Result<usize, StoreError> {
    let count: i64 = connection
        .query_row(sql, params![session_id.as_bytes().as_slice()], |row| {
            row.get(0)
        })
        .map_err(map_sqlite_error)?;
    usize_from_i64(count, "count")
}

fn lookup_record_batch(
    connection: &Connection,
    record_id: RecordId,
) -> Result<Option<AppendBatchId>, StoreError> {
    let bytes = connection
        .query_row(
            "SELECT batch_id FROM records WHERE record_id = ?1",
            params![record_id.as_bytes().as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    bytes.as_deref().map(id_from_blob).transpose()
}

fn id_from_blob<T: IdTag>(bytes: &[u8]) -> Result<Id<T>, StoreError> {
    let value: [u8; 16] = bytes.try_into().map_err(|_| StoreError::Integrity {
        reason_code: "sqlite_id_width",
    })?;
    Ok(Id::from_bytes(value))
}

const DIGEST_HEX: &[u8; 16] = b"0123456789abcdef";

fn digest_from_blob(bytes: &[u8]) -> Result<Digest, StoreError> {
    if bytes.len() != 32 {
        return Err(StoreError::Integrity {
            reason_code: "sqlite_digest_width",
        });
    }
    let mut hex = [0_u8; 64];
    for (index, byte) in bytes.iter().enumerate() {
        hex[index * 2] = DIGEST_HEX[usize::from(byte >> 4)];
        hex[index * 2 + 1] = DIGEST_HEX[usize::from(byte & 0x0f)];
    }
    let hex = core::str::from_utf8(&hex).expect("hex alphabet");
    Digest::from_hex(hex).map_err(|_| StoreError::Integrity {
        reason_code: "sqlite_digest_hex",
    })
}

fn i64_from_u64(value: u64, reason_code: &'static str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

fn u64_from_i64(value: i64, reason_code: &'static str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

fn u16_from_i64(value: i64, reason_code: &'static str) -> Result<u16, StoreError> {
    u16::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

fn usize_from_i64(value: i64, reason_code: &'static str) -> Result<usize, StoreError> {
    usize::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rusqlite map_err adapter takes the owned error"
)]
fn map_sqlite_error(error: rusqlite::Error) -> StoreError {
    if let Some(code) = error.sqlite_error_code() {
        return match code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                StoreError::Unavailable {
                    reason_code: "sqlite_busy",
                }
            }
            rusqlite::ErrorCode::DiskFull => StoreError::Unavailable {
                reason_code: "sqlite_disk_full",
            },
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                StoreError::Integrity {
                    reason_code: "sqlite_corrupt",
                }
            }
            _ => StoreError::Unavailable {
                reason_code: "sqlite_error",
            },
        };
    }
    StoreError::Unavailable {
        reason_code: "sqlite_error",
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "protocol map_err adapter takes the owned error"
)]
fn protocol_error(error: ProtocolError) -> StoreError {
    match error {
        ProtocolError::LimitExceeded { resource, limit } => {
            StoreError::LimitExceeded { resource, limit }
        }
        ProtocolError::Integrity { reason_code } | ProtocolError::InvalidCbor { reason_code } => {
            StoreError::Integrity { reason_code }
        }
        ProtocolError::Codec { .. } => StoreError::Integrity {
            reason_code: "canonical_codec",
        },
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::{Arc, Barrier};
    use std::task::{Context, Poll, Waker};
    use std::thread;

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
        DeadlinePropagation, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
        KernelInput, LaneCreated, LaneTag, PrincipalPropagation, PrincipalRef,
        RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordDraft, RecordTag, RunAccepted, RunLimits,
        RunPropagationPolicy, RunRelation, RunSecurityContext, RunTag, SessionCreated, SessionTag,
        Timestamp, TransitionEnv,
    };
    use finstack_ai_protocol::{envelope_checksum, payload_digest, verify_envelope};
    use finstack_ai_runtime::CommitCoordinator;
    use finstack_ai_test::{JournalStoreConformanceCase, check_journal_store_conformance};
    use tempfile::TempDir;

    use super::*;

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => thread::yield_now(),
            }
        }
    }

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn limits() -> SqliteStoreLimits {
        SqliteStoreLimits {
            sessions: 4,
            batches_per_session: 8,
            records_per_session: 16,
            snapshot_bytes: 1024,
        }
    }

    fn snapshot_capable_store() -> Arc<SqliteJournalStore> {
        Arc::new(
            SqliteJournalStore::try_open(SqliteStoreConfig {
                path: PathBuf::from(":memory:"),
                durability: SqliteDurability::Relaxed {
                    synchronous: SqliteSynchronous::Normal,
                },
                limits: SqliteStoreLimits {
                    sessions: 4,
                    batches_per_session: 8,
                    records_per_session: 16,
                    snapshot_bytes: 256 * 1024,
                },
                busy_timeout: DEFAULT_BUSY_TIMEOUT,
            })
            .expect("store"),
        )
    }

    fn accept_root_run(store: &Arc<SqliteJournalStore>) {
        let mut coordinator = CommitCoordinator::new(Arc::clone(store) as Arc<dyn JournalStore>);
        let run_id = id::<RunTag>(3);
        block_on(
            coordinator.submit(
                TransitionEnv {
                    now: Timestamp::from_unix_ms(1_000).expect("ts"),
                    ids: AllocatedIds::try_new(
                        vec![id(1)],
                        vec![id(1)],
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        vec![id(101)],
                        Vec::new(),
                    )
                    .expect("ids"),
                },
                KernelInput::AcceptRun(AcceptRun {
                    session_id: id::<SessionTag>(1),
                    lane_id: id::<LaneTag>(2),
                    accepted: RunAccepted::try_new(
                        run_id,
                        RunRelation::root(run_id).expect("relation"),
                        RunSecurityContext::try_new(
                            "tenant-a",
                            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                                .expect("principal"),
                            "oidc",
                            "high",
                            "policy-v1",
                            "decision-v1",
                            None,
                        )
                        .expect("security"),
                        None,
                        RunLimits::empty(),
                        RunPropagationPolicy {
                            cancellation: CancellationPropagation::Cascade,
                            deadline: DeadlinePropagation::MinimumOfParentAndChild,
                            budget: BudgetPropagation::SharedScope,
                            principal: PrincipalPropagation::Inherit,
                        },
                        Digest::raw_json(br#"{"agent":"fixture"}"#),
                        None,
                    )
                    .expect("accepted"),
                }),
            ),
        )
        .expect("accept");
    }

    fn memory_store() -> SqliteJournalStore {
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: PathBuf::from(":memory:"),
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: limits(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("memory store")
    }

    fn file_store(dir: &TempDir, durability: SqliteDurability) -> SqliteJournalStore {
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: dir.path().join("journal.sqlite"),
            durability,
            limits: limits(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("file store")
    }

    fn draft(record_ordinal: u64, session_ordinal: u64) -> RecordDraft {
        let principal =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
        let authorization =
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
        let rejection = ExternalCommandRejected::try_new(
            ExternalCommandKind::EffectCompletion,
            format!("completion-{record_ordinal}"),
            ExternalCommandTarget::Effect(id(record_ordinal + 1000)),
            principal,
            authorization,
            "conflicting_completion",
            Digest::raw_json(b"{}"),
            None,
        )
        .expect("rejection");
        RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(record_ordinal),
            id::<SessionTag>(session_ordinal),
            id::<LaneTag>(session_ordinal + 100),
            Some(id::<RunTag>(session_ordinal + 200)),
            Timestamp::from_unix_ms(i64::try_from(record_ordinal).expect("timestamp"))
                .expect("timestamp"),
            Vec::new(),
            RecordBody::ExternalCommandRejected(rejection),
        )
        .expect("draft")
    }

    fn request(
        batch_ordinal: u64,
        session_ordinal: u64,
        expected_sequence: u64,
        drafts: Vec<RecordDraft>,
    ) -> AppendRequest {
        AppendRequest::try_new(
            id(batch_ordinal),
            id::<SessionTag>(session_ordinal),
            expected_sequence,
            drafts,
        )
        .expect("append request")
    }

    #[test]
    fn durable_mode_is_rejected_for_memory_and_labeled_on_files() {
        assert!(matches!(
            SqliteJournalStore::try_open(SqliteStoreConfig {
                path: PathBuf::from(":memory:"),
                durability: SqliteDurability::Durable,
                limits: limits(),
                busy_timeout: DEFAULT_BUSY_TIMEOUT,
            }),
            Err(StoreError::InvalidRequest {
                reason_code: "sqlite_durable_requires_file"
            })
        ));
        let dir = TempDir::new().expect("tempdir");
        let store = file_store(&dir, SqliteDurability::Durable);
        let health = block_on(store.health()).expect("health");
        assert!(health.ready);
        assert!(health.durable);
        assert_eq!(health.detail.as_ref(), "sqlite_durable_wal_full");
    }

    #[test]
    fn relaxed_modes_never_advertise_nfr_rel_001() {
        let memory = memory_store();
        let health = block_on(memory.health()).expect("health");
        assert!(health.ready);
        assert!(!health.durable);
        assert_eq!(health.detail.as_ref(), "sqlite_relaxed_in_memory");

        let dir = TempDir::new().expect("tempdir");
        let store = file_store(
            &dir,
            SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Off,
            },
        );
        let health = block_on(store.health()).expect("health");
        assert!(!health.durable);
        assert_eq!(health.detail.as_ref(), "sqlite_relaxed_synchronous_off");
    }

    #[test]
    fn unknown_user_version_fails_closed() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("journal.sqlite");
        drop(file_store(&dir, SqliteDurability::Durable));
        let connection = Connection::open(&path).expect("reopen");
        connection
            .pragma_update(None, "user_version", 99)
            .expect("bump");
        drop(connection);
        assert!(matches!(
            SqliteJournalStore::try_open(SqliteStoreConfig {
                path,
                durability: SqliteDurability::Durable,
                limits: limits(),
                busy_timeout: DEFAULT_BUSY_TIMEOUT,
            }),
            Err(StoreError::Integrity {
                reason_code: "sqlite_schema_unsupported"
            })
        ));
    }

    #[test]
    fn append_load_and_health_preserve_boundaries() {
        let store = memory_store();
        let first = block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
        let second = block_on(store.append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)])))
            .expect("append");
        assert_eq!((first.first_sequence, first.last_sequence), (1, 1));
        assert_eq!((second.first_sequence, second.last_sequence), (2, 3));
        verify_envelope(&first.records[0]).expect("first envelope");
        verify_envelope(&second.records[0]).expect("second envelope");
        assert_eq!(
            second.records[0].previous_checksum(),
            Some(first.records[0].checksum())
        );
        assert_eq!(
            first.records[0].payload_digest(),
            payload_digest(first.records[0].body()).expect("payload")
        );
        assert_eq!(
            first.records[0].checksum(),
            envelope_checksum(&first.records[0]).expect("checksum")
        );

        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 3);
        assert_eq!(loaded.head_checksum, Some(second.records[1].checksum()));
        assert_eq!(loaded.metadata, Metadata::empty());
        assert_eq!(loaded.committed_batches.as_ref(), &[first, second]);
        assert!(loaded.snapshot.is_none());
    }

    #[test]
    fn scan_and_metadata_cas_are_session_local() {
        let store = memory_store();
        block_on(store.append(request(1, 1, 1, vec![draft(1, 1)]))).expect("append");
        let second = block_on(store.append(request(2, 1, 2, vec![draft(2, 1), draft(3, 1)])))
            .expect("append");
        let page = block_on(store.scan(ScanRequest {
            session_id: id::<SessionTag>(1),
            from_sequence: 0,
            limit: 2,
        }))
        .expect("scan");
        assert_eq!(page.records.len(), 2);
        assert_eq!(page.next_sequence, Some(3));
        assert!(matches!(
            block_on(store.scan(ScanRequest {
                session_id: id::<SessionTag>(1),
                from_sequence: 1,
                limit: 0,
            })),
            Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_zero"
            })
        ));

        let metadata = Metadata::parse(br#"{"label":"demo"}"#).expect("metadata");
        assert!(matches!(
            block_on(store.write_metadata(WriteMetadataRequest {
                session_id: id::<SessionTag>(1),
                expected_head_checksum: None,
                metadata: metadata.clone(),
            })),
            Err(StoreError::InvalidRequest {
                reason_code: "metadata_cas_mismatch"
            })
        ));
        let receipt = block_on(store.write_metadata(WriteMetadataRequest {
            session_id: id::<SessionTag>(1),
            expected_head_checksum: Some(second.records[1].checksum()),
            metadata: metadata.clone(),
        }))
        .expect("cas");
        assert_eq!(receipt.metadata, metadata);
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.metadata, metadata);
    }

    #[test]
    fn structural_session_and_lane_records_commit_without_run_id() {
        let store = memory_store();
        let session = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(90),
            id::<SessionTag>(9),
            id::<LaneTag>(91),
            None,
            Timestamp::from_unix_ms(1).expect("ts"),
            Vec::new(),
            RecordBody::SessionCreated(SessionCreated::new(Metadata::empty())),
        )
        .expect("session");
        let lane = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(91),
            id::<SessionTag>(9),
            id::<LaneTag>(91),
            None,
            Timestamp::from_unix_ms(1).expect("ts"),
            Vec::new(),
            RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
        )
        .expect("lane");
        let committed =
            block_on(store.append(request(90, 9, 1, vec![session, lane]))).expect("append");
        assert_eq!(committed.records.len(), 2);
        verify_envelope(&committed.records[0]).expect("session envelope");
        verify_envelope(&committed.records[1]).expect("lane envelope");
    }

    #[test]
    fn batch_and_record_idempotency_precede_sequence_checks() {
        let store = memory_store();
        let frozen = request(10, 1, 1, vec![draft(10, 1)]);
        let original = block_on(store.append(frozen.clone())).expect("append");
        assert_eq!(
            block_on(store.append(frozen.clone())).expect("same batch"),
            original
        );

        let same_records_new_batch = AppendRequest::try_new(
            id(11),
            frozen.session_id(),
            frozen.expected_sequence(),
            frozen.records().to_vec(),
        )
        .expect("request");
        assert_eq!(
            block_on(store.append(same_records_new_batch)).expect("record idempotency"),
            original
        );

        let unequal_batch = request(10, 1, 2, vec![draft(11, 1)]);
        assert!(matches!(
            block_on(store.append(unequal_batch)),
            Err(StoreError::Corruption {
                reason_code: "append_batch_id_reuse"
            })
        ));
        let mixed = request(12, 1, 2, vec![draft(10, 1), draft(12, 1)]);
        assert!(matches!(
            block_on(store.append(mixed)),
            Err(StoreError::Corruption {
                reason_code: "mixed_record_id_reuse"
            })
        ));
    }

    #[test]
    fn conflicts_are_atomic_and_concurrent_writers_have_one_winner() {
        let store = Arc::new(memory_store());
        let barrier = Arc::new(Barrier::new(3));
        let mut joins = Vec::new();
        for ordinal in [20_u64, 21] {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            joins.push(thread::spawn(move || {
                barrier.wait();
                block_on(store.append(request(ordinal, 1, 1, vec![draft(ordinal, 1)])))
            }));
        }
        barrier.wait();
        let results = joins
            .into_iter()
            .map(|join| join.join().expect("writer"))
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(StoreError::Conflict { .. })))
                .count(),
            1
        );

        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 1);
        assert_eq!(loaded.committed_batches.len(), 1);
    }

    #[test]
    fn configured_limits_and_snapshot_cache_are_enforced() {
        assert!(
            SqliteJournalStore::try_open(SqliteStoreConfig {
                path: PathBuf::from(":memory:"),
                durability: SqliteDurability::Relaxed {
                    synchronous: SqliteSynchronous::Normal,
                },
                limits: SqliteStoreLimits {
                    sessions: 0,
                    ..limits()
                },
                busy_timeout: DEFAULT_BUSY_TIMEOUT,
            })
            .is_err()
        );
        let store = SqliteJournalStore::try_open(SqliteStoreConfig {
            path: PathBuf::from(":memory:"),
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 1,
                batches_per_session: 1,
                records_per_session: 1,
                snapshot_bytes: 3,
            },
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("store");
        block_on(store.append(request(30, 1, 1, vec![draft(30, 1)]))).expect("append");
        assert!(matches!(
            block_on(store.append(request(31, 1, 2, vec![draft(31, 1)]))),
            Err(StoreError::LimitExceeded {
                resource: "batches_per_session",
                limit: 1
            })
        ));
        assert!(matches!(
            block_on(store.append(request(32, 2, 1, vec![draft(32, 2)]))),
            Err(StoreError::LimitExceeded {
                resource: "sessions",
                limit: 1
            })
        ));

        let oversized =
            OpaqueSnapshot::try_new(1, Digest::raw_json(b"four"), b"four".as_slice(), 8)
                .expect("caller ceiling");
        assert!(matches!(
            block_on(store.write_snapshot(SnapshotRequest {
                session_id: id::<SessionTag>(1),
                snapshot: oversized,
            })),
            Err(StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: 3
            })
        ));

        let snapshot = OpaqueSnapshot::try_new(1, Digest::raw_json(b"one"), b"one".as_slice(), 3)
            .expect("snapshot");
        let receipt = block_on(store.write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: snapshot.clone(),
        }))
        .expect("snapshot write");
        assert_eq!(receipt.bytes, 3);
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.snapshot, Some(snapshot));
    }

    #[test]
    fn injected_rollback_leaves_no_partial_batch() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("journal.sqlite");
        let store = file_store(&dir, SqliteDurability::Durable);
        store
            .append_then_rollback(&request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]))
            .expect("rollback");
        drop(store);
        let reopened = SqliteJournalStore::try_open(SqliteStoreConfig {
            path,
            durability: SqliteDurability::Durable,
            limits: limits(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("reopen");
        let loaded = block_on(reopened.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        assert_eq!(loaded.head_sequence, 0);
        assert!(loaded.committed_batches.is_empty());
    }

    #[test]
    fn conformance_and_ambiguous_ack_use_the_sqlite_store() {
        let store = memory_store();
        block_on(check_journal_store_conformance(
            &store,
            JournalStoreConformanceCase {
                request: request(1, 1, 1, vec![draft(1, 1)]),
                expected_first_sequence: 1,
                expected_last_sequence: 1,
            },
        ))
        .expect("conformance");
    }

    #[test]
    fn discarding_sqlite_snapshots_still_recovers_from_the_journal() {
        let store = snapshot_capable_store();
        accept_root_run(&store);
        let recovered = block_on(CommitCoordinator::recover(
            Arc::clone(&store) as Arc<dyn JournalStore>,
            id::<SessionTag>(1),
        ))
        .expect("recover");
        let expected = recovered.state().state_hash().expect("hash");
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("load");
        block_on(store.write_state_snapshot(StateSnapshotRequest {
            session_id: id::<SessionTag>(1),
            state: recovered.state().clone(),
            head_checksum: loaded.head_checksum.expect("head"),
            pending_timer_scheduled_at: None,
        }))
        .expect("snapshot");
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("accelerated");
        assert!(loaded.accelerated.is_some());
        store
            .discard_snapshot(id::<SessionTag>(1))
            .expect("discard");
        let loaded = block_on(store.load(LoadRequest {
            session_id: id::<SessionTag>(1),
        }))
        .expect("after discard");
        assert!(loaded.snapshot.is_none());
        assert!(loaded.accelerated.is_none());
        let rebuilt =
            block_on(CommitCoordinator::recover(store, id::<SessionTag>(1))).expect("rebuild");
        assert_eq!(rebuilt.state().state_hash().expect("hash"), expected);
    }

    #[test]
    fn v1_schema_applies_from_user_version_zero() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("fresh.sqlite");
        let connection = Connection::open(&path).expect("create");
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, 0);
        drop(connection);
        let store = SqliteJournalStore::try_open(SqliteStoreConfig {
            path: path.clone(),
            durability: SqliteDurability::Durable,
            limits: limits(),
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("apply v1");
        drop(store);
        let connection = Connection::open(&path).expect("reopen");
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, SCHEMA_USER_VERSION);
    }
}
