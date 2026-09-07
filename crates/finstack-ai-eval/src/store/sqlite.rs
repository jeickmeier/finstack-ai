//! Dedicated `SQLite` evaluation database: schema version 1, WAL, FULL durability.
use super::state::{MAX_EVENT_BYTES, MAX_LOG_BYTES, State};
use super::{EvalStore, Mutation, RunnerLease, StoreSnapshot, mutation_methods};
use crate::error::{invalid, unavailable};
use crate::{EVAL_RUNNER_BUSY, EVAL_STORE_VERSION, EvalError};
use rusqlite::{Connection, TransactionBehavior, params};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Inner {
    connection: Connection,
    state: State,
    sequence: u64,
    bytes: u64,
}

/// Disk-backed append-only experiment. Use a separate file from the journal.
/// An OS file lock grants one active runner across independent processes.
pub struct SqliteEvalStore {
    inner: Mutex<Inner>,
    lease_path: PathBuf,
}
impl SqliteEvalStore {
    /// Open or create a dedicated evaluation database.
    /// # Errors
    /// Rejects unrelated/unversioned databases, unknown versions, corrupted logs,
    /// and unavailable paths. Acknowledged appends use `synchronous=FULL`.
    pub fn try_open(path: impl AsRef<Path>) -> Result<Self, EvalError> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() || path == Path::new(":memory:") {
            return Err(invalid());
        }
        let mut connection = Connection::open(path).map_err(|_| unavailable())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| unavailable())?;
        let version: u32 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| unavailable())?;
        if version > 1 {
            return Err(EvalError::new(
                EVAL_STORE_VERSION,
                "unsupported evaluation database version",
            ));
        }
        if version == 0 {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| unavailable())?;
            let tables: u64 = tx.query_row("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'", [], |row| row.get(0)).map_err(|_| unavailable())?;
            if tables != 0 {
                return Err(EvalError::new(
                    EVAL_STORE_VERSION,
                    "evaluation database must own its file",
                ));
            }
            tx.execute_batch("CREATE TABLE eval_events (sequence INTEGER PRIMARY KEY, payload BLOB NOT NULL); PRAGMA user_version = 1;").map_err(|_| unavailable())?;
            tx.commit().map_err(|_| unavailable())?;
        }
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|_| unavailable())?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|_| unavailable())?;
        let mut inner = Inner {
            connection,
            state: State::default(),
            sequence: 0,
            bytes: 0,
        };
        refresh(&mut inner)?;
        let canonical = path.canonicalize().map_err(|_| unavailable())?;
        let mut lease_path = canonical.into_os_string();
        lease_path.push(".runner-lock");
        Ok(Self {
            inner: Mutex::new(inner),
            lease_path: lease_path.into(),
        })
    }

    fn append(&self, mutation: Mutation) -> Result<(), EvalError> {
        let mut inner = self.inner.lock().map_err(|_| unavailable())?;
        // BEGIN IMMEDIATE prevents a second writer changing the checked prefix.
        inner
            .connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| unavailable())?;
        let result = append_transaction(&mut inner, mutation);
        if result.is_err() {
            let _ = inner.connection.execute_batch("ROLLBACK");
        }
        result
    }
}

fn refresh(inner: &mut Inner) -> Result<(), EvalError> {
    let mut statement = inner.connection.prepare("SELECT sequence, length(payload), payload FROM eval_events WHERE sequence > ? ORDER BY sequence").map_err(|_| unavailable())?;
    let mut rows = statement
        .query(params![inner.sequence])
        .map_err(|_| unavailable())?;
    while let Some(row) = rows.next().map_err(|_| unavailable())? {
        let sequence: u64 = row.get(0).map_err(|_| unavailable())?;
        let length: u64 = row.get(1).map_err(|_| unavailable())?;
        if sequence != inner.sequence + 1
            || length > MAX_EVENT_BYTES as u64
            || inner
                .bytes
                .checked_add(length)
                .is_none_or(|bytes| bytes > MAX_LOG_BYTES)
        {
            return Err(invalid());
        }
        let bytes: Vec<u8> = row.get(2).map_err(|_| unavailable())?;
        let mutation = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        inner.state.apply(mutation)?;
        inner.sequence = sequence;
        inner.bytes += length;
    }
    Ok(())
}

fn append_transaction(inner: &mut Inner, mutation: Mutation) -> Result<(), EvalError> {
    refresh(inner)?;
    if !inner.state.check(&mutation)? {
        inner
            .connection
            .execute_batch("COMMIT")
            .map_err(|_| unavailable())?;
        return Ok(());
    }
    let bytes = serde_json::to_vec(&mutation).map_err(|_| invalid())?;
    let total = inner
        .bytes
        .checked_add(bytes.len() as u64)
        .ok_or_else(invalid)?;
    if bytes.len() > MAX_EVENT_BYTES || total > MAX_LOG_BYTES {
        return Err(invalid());
    }
    let sequence = inner.sequence + 1;
    inner
        .connection
        .execute(
            "INSERT INTO eval_events(sequence, payload) VALUES (?, ?)",
            params![sequence, bytes],
        )
        .map_err(|_| unavailable())?;
    inner
        .connection
        .execute_batch("COMMIT")
        .map_err(|_| unavailable())?;
    inner.state.apply(mutation)?;
    inner.sequence = sequence;
    inner.bytes = total;
    Ok(())
}

struct FileLease {
    _file: File,
}
impl RunnerLease for FileLease {}
impl EvalStore for SqliteEvalStore {
    fn acquire_runner(&self) -> Result<Box<dyn RunnerLease>, EvalError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lease_path)
            .map_err(|_| unavailable())?;
        file.try_lock()
            .map_err(|_| EvalError::new(EVAL_RUNNER_BUSY, "experiment runner already active"))?;
        Ok(Box::new(FileLease { _file: file }))
    }
    mutation_methods!();
    fn snapshot(&self) -> Result<StoreSnapshot, EvalError> {
        let mut inner = self.inner.lock().map_err(|_| unavailable())?;
        refresh(&mut inner)?;
        Ok(inner.state.snapshot.clone())
    }
}
