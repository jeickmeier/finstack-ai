//! Shared owned-`SQLite` worker shell for derived search indexes.
//!
//! One bounded worker thread owns each index connection. SQL never runs on an
//! async executor; queued requests whose caller has gone away are discarded
//! before execution. A [`DatabaseSpec`] declares the index's meta table, kind
//! stem, current schema version, fresh-install DDL and in-place migrations, so
//! every index crate shares one open/migrate/call implementation while keeping
//! its own stable error-code prefix and identity digest label.
//!
//! Available only with the `sqlite` feature; the default build of this crate
//! stays free of `rusqlite` and Tokio.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::Digest;
use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use crate::{SearchError, configuration_digest};

/// One in-place schema step from `from_version` to `from_version + 1`.
///
/// The SQL runs inside the migration transaction after every earlier step. The
/// shell updates the meta-table `kind` and `PRAGMA user_version` itself, so the
/// statement only has to transform tables and indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migration {
    /// Schema version this step upgrades from.
    pub from_version: u32,
    /// Statements applied to reach `from_version + 1`.
    pub sql: &'static str,
}

/// Static description of one owned index database.
///
/// The meta table must be a singleton `(singleton INTEGER PRIMARY KEY
/// CHECK(singleton=1), kind TEXT NOT NULL)` row whose `kind` is
/// `"{kind}.v{version}"`; `ddl` must create it at the current `version`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseSpec {
    /// Domain label for the canonical-path identity digest.
    pub digest_label: &'static str,
    /// Name of the worker thread owning the connection.
    pub thread_name: &'static str,
    /// Singleton meta table carrying the schema `kind`.
    pub meta_table: &'static str,
    /// Kind stem; the stored value is `"{kind}.v{version}"`.
    pub kind: &'static str,
    /// Current schema version written by `ddl`.
    pub version: u32,
    /// Complete fresh-install schema at `version`.
    pub ddl: &'static str,
    /// Contiguous in-place upgrade steps ending at `version`.
    pub migrations: &'static [Migration],
    /// Stable prefix for the `_schema`, `_schema_version` and
    /// `_foreign_database` validation reasons.
    pub error_prefix: &'static str,
}

impl DatabaseSpec {
    fn invalid(&self, suffix: &str) -> SearchError {
        SearchError::SearchInvalid {
            reason: Arc::from(format!("{}_{suffix}", self.error_prefix)),
        }
    }

    fn kind_at(&self, version: u32) -> String {
        format!("{}.v{version}", self.kind)
    }
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

struct Worker {
    sender: Option<mpsc::Sender<Job>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Handle to an owned, schema-versioned index database.
///
/// Cloning shares the worker; the connection closes when the last clone drops.
/// `:memory:` paths skip WAL and canonicalization so tests stay hermetic.
#[derive(Clone)]
pub struct Database {
    worker: Arc<Worker>,
    identity: Digest,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl Database {
    /// Open or create the database at `path`, verifying ownership and applying
    /// any pending migrations before the worker starts.
    ///
    /// # Errors
    /// Returns [`SearchError::SearchUnavailable`] for connection or I/O failure
    /// and `{error_prefix}_foreign_database`, `{error_prefix}_schema` or
    /// `{error_prefix}_schema_version` validation errors when the file is not an
    /// index this spec owns at a reachable version.
    ///
    /// # Examples
    /// ```
    /// use finstack_ai_search_core::sqlite::{Database, DatabaseSpec};
    ///
    /// static SPEC: DatabaseSpec = DatabaseSpec {
    ///     digest_label: "example-index-path",
    ///     thread_name: "finstack-example-index",
    ///     meta_table: "example_meta",
    ///     kind: "finstack.example-index",
    ///     version: 1,
    ///     ddl: "CREATE TABLE example_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),kind TEXT NOT NULL);\
    ///           INSERT INTO example_meta VALUES(1,'finstack.example-index.v1');",
    ///     migrations: &[],
    ///     error_prefix: "example_index",
    /// };
    ///
    /// let database = Database::open(std::path::Path::new(":memory:"), &SPEC)?;
    /// assert_eq!(database.identity(), Database::open(std::path::Path::new(":memory:"), &SPEC)?.identity());
    /// # Ok::<(), finstack_ai_search_core::SearchError>(())
    /// ```
    pub fn open(path: &Path, spec: &'static DatabaseSpec) -> Result<Self, SearchError> {
        let in_memory = path == Path::new(":memory:");
        let mut connection = Connection::open(path).map_err(|_| SearchError::SearchUnavailable)?;
        connection
            .busy_timeout(Duration::from_secs(1))
            .map_err(|_| SearchError::SearchUnavailable)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|_| SearchError::SearchUnavailable)?;
        if !in_memory {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| SearchError::SearchUnavailable)?;
        }
        schema(&mut connection, spec)?;
        let location = if in_memory {
            path.to_path_buf()
        } else {
            std::fs::canonicalize(path).map_err(|_| SearchError::SearchUnavailable)?
        };
        let identity = configuration_digest(spec.digest_label, &location.to_string_lossy())?;
        let (sender, mut receiver) = mpsc::channel::<Job>(32);
        let join = std::thread::Builder::new()
            .name(spec.thread_name.into())
            .spawn(move || {
                while let Some(job) = receiver.blocking_recv() {
                    job(&mut connection);
                }
            })
            .map_err(|_| SearchError::SearchUnavailable)?;
        Ok(Self {
            worker: Arc::new(Worker {
                sender: Some(sender),
                join: Some(join),
            }),
            identity,
        })
    }

    /// Digest of the spec's label and the canonical database location, for
    /// inclusion in source configuration digests.
    #[must_use]
    pub const fn identity(&self) -> Digest {
        self.identity
    }

    /// Run `operation` on the worker thread and await its result.
    ///
    /// # Errors
    /// Returns [`SearchError::SearchUnavailable`] when the bounded queue is full
    /// or the worker has stopped; otherwise the operation's own result.
    pub async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, SearchError> + Send + 'static,
    ) -> Result<T, SearchError> {
        let (reply, result) = oneshot::channel();
        let job: Job = Box::new(move |connection| {
            if !reply.is_closed() {
                let _ = reply.send(operation(connection));
            }
        });
        self.worker
            .sender
            .as_ref()
            .ok_or(SearchError::SearchUnavailable)?
            .try_send(job)
            .map_err(|_| SearchError::SearchUnavailable)?;
        result.await.map_err(|_| SearchError::SearchUnavailable)?
    }
}

fn schema(connection: &mut Connection, spec: &DatabaseSpec) -> Result<(), SearchError> {
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| SearchError::SearchUnavailable)?;
    if version == 0 {
        let tables: u32 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .map_err(|_| SearchError::SearchUnavailable)?;
        if tables != 0 {
            return Err(spec.invalid("foreign_database"));
        }
        let tx = connection
            .transaction()
            .map_err(|_| SearchError::SearchUnavailable)?;
        tx.execute_batch(spec.ddl)
            .map_err(|_| SearchError::SearchUnavailable)?;
        tx.pragma_update(None, "user_version", spec.version)
            .map_err(|_| SearchError::SearchUnavailable)?;
        return tx.commit().map_err(|_| SearchError::SearchUnavailable);
    }
    let known = version == spec.version
        || spec
            .migrations
            .iter()
            .any(|migration| migration.from_version == version);
    if !known {
        return Err(spec.invalid("schema_version"));
    }
    let kind: String = connection
        .query_row(
            &format!("SELECT kind FROM {} WHERE singleton=1", spec.meta_table),
            [],
            |r| r.get(0),
        )
        .map_err(|_| spec.invalid("schema"))?;
    if kind != spec.kind_at(version) {
        return Err(spec.invalid("schema_version"));
    }
    if version == spec.version {
        return Ok(());
    }
    let mut steps = Vec::new();
    let mut current = version;
    while current < spec.version {
        let step = spec
            .migrations
            .iter()
            .find(|migration| migration.from_version == current)
            .ok_or_else(|| spec.invalid("schema_version"))?;
        steps.push(step);
        current += 1;
    }
    let tx = connection
        .transaction()
        .map_err(|_| SearchError::SearchUnavailable)?;
    for step in steps {
        tx.execute_batch(step.sql)
            .map_err(|_| SearchError::SearchUnavailable)?;
    }
    tx.execute(
        &format!("UPDATE {} SET kind=?1 WHERE singleton=1", spec.meta_table),
        [spec.kind_at(spec.version)],
    )
    .map_err(|_| SearchError::SearchUnavailable)?;
    tx.pragma_update(None, "user_version", spec.version)
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.commit().map_err(|_| SearchError::SearchUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = "CREATE TABLE shell_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),kind TEXT NOT NULL);";

    const V1: DatabaseSpec = DatabaseSpec {
        digest_label: "shell-test-path",
        thread_name: "finstack-shell-test",
        meta_table: "shell_meta",
        kind: "finstack.shell-test",
        version: 1,
        ddl: "CREATE TABLE shell_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),kind TEXT NOT NULL);INSERT INTO shell_meta VALUES(1,'finstack.shell-test.v1');CREATE TABLE items(id INTEGER PRIMARY KEY);",
        migrations: &[],
        error_prefix: "shell_test",
    };

    const V3: DatabaseSpec = DatabaseSpec {
        digest_label: "shell-test-path",
        thread_name: "finstack-shell-test",
        meta_table: "shell_meta",
        kind: "finstack.shell-test",
        version: 3,
        ddl: "CREATE TABLE shell_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),kind TEXT NOT NULL);INSERT INTO shell_meta VALUES(1,'finstack.shell-test.v3');CREATE TABLE items(id INTEGER PRIMARY KEY, step INTEGER NOT NULL DEFAULT 0);CREATE INDEX items_step ON items(step);",
        migrations: &[
            Migration {
                from_version: 2,
                sql: "CREATE INDEX items_step ON items(step);",
            },
            Migration {
                from_version: 1,
                sql: "ALTER TABLE items ADD COLUMN step INTEGER NOT NULL DEFAULT 0;",
            },
        ],
        error_prefix: "shell_test",
    };

    const GAPPED: DatabaseSpec = DatabaseSpec {
        migrations: &[Migration {
            from_version: 2,
            sql: "",
        }],
        ..V3
    };

    fn reason(error: &SearchError) -> &str {
        match error {
            SearchError::SearchInvalid { reason } => reason,
            other => panic!("expected validation failure, got {other:?}"),
        }
    }

    async fn journal_mode(database: &Database) -> String {
        database
            .call(|db| {
                db.pragma_query_value(None, "journal_mode", |row| row.get(0))
                    .map_err(|_| SearchError::SearchUnavailable)
            })
            .await
            .unwrap()
    }

    #[test]
    fn foreign_database_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foreign.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE external(value TEXT)")
            .unwrap();
        let error = Database::open(&path, &V1).unwrap_err();
        assert_eq!(reason(&error), "shell_test_foreign_database");
        let unowned = dir.path().join("unowned.sqlite");
        Connection::open(&unowned)
            .unwrap()
            .execute_batch(&format!(
                "{META}INSERT INTO shell_meta VALUES(1,'finstack.other-index.v1');PRAGMA user_version=1;"
            ))
            .unwrap();
        let error = Database::open(&unowned, &V1).unwrap_err();
        assert_eq!(reason(&error), "shell_test_schema_version");
        let missing_meta = dir.path().join("missing.sqlite");
        Connection::open(&missing_meta)
            .unwrap()
            .execute_batch("CREATE TABLE items(id INTEGER PRIMARY KEY);PRAGMA user_version=1;")
            .unwrap();
        let error = Database::open(&missing_meta, &V1).unwrap_err();
        assert_eq!(reason(&error), "shell_test_schema");
    }

    #[test]
    fn newer_version_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.sqlite");
        Connection::open(&path)
            .unwrap()
            .execute_batch("PRAGMA user_version=99")
            .unwrap();
        let error = Database::open(&path, &V1).unwrap_err();
        assert_eq!(reason(&error), "shell_test_schema_version");
        // A v3 database opened by the v1 spec is also unreachable, not foreign.
        let newer = dir.path().join("newer.sqlite");
        drop(Database::open(&newer, &V3).unwrap());
        let error = Database::open(&newer, &V1).unwrap_err();
        assert_eq!(reason(&error), "shell_test_schema_version");
    }

    #[tokio::test]
    async fn migration_chain_applies_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chain.sqlite");
        {
            let old = Database::open(&path, &V1).unwrap();
            old.call(|db| {
                db.execute("INSERT INTO items(id) VALUES(7)", [])
                    .map_err(|_| SearchError::SearchUnavailable)
            })
            .await
            .unwrap();
        }
        let migrated = Database::open(&path, &V3).unwrap();
        let (version, kind, indexes, rows): (u32, String, u32, u32) = migrated
            .call(|db| {
                let version = db
                    .pragma_query_value(None, "user_version", |row| row.get(0))
                    .map_err(|_| SearchError::SearchUnavailable)?;
                let kind = db
                    .query_row("SELECT kind FROM shell_meta WHERE singleton=1", [], |r| {
                        r.get(0)
                    })
                    .map_err(|_| SearchError::SearchUnavailable)?;
                let indexes = db
                    .query_row(
                        "SELECT count(*) FROM sqlite_master WHERE name='items_step'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|_| SearchError::SearchUnavailable)?;
                let rows = db
                    .query_row("SELECT count(*) FROM items WHERE step=0", [], |r| r.get(0))
                    .map_err(|_| SearchError::SearchUnavailable)?;
                Ok((version, kind, indexes, rows))
            })
            .await
            .unwrap();
        assert_eq!(version, 3);
        assert_eq!(kind, "finstack.shell-test.v3");
        assert_eq!(indexes, 1);
        assert_eq!(rows, 1);
        assert_eq!(
            migrated.identity(),
            Database::open(&path, &V3).unwrap().identity()
        );
        // A gap in the chain is a version the spec cannot reach.
        let gapped = dir.path().join("gapped.sqlite");
        drop(Database::open(&gapped, &V1).unwrap());
        let error = Database::open(&gapped, &GAPPED).unwrap_err();
        assert_eq!(reason(&error), "shell_test_schema_version");
    }

    #[tokio::test]
    async fn in_memory_path_skips_wal() {
        let memory = Database::open(Path::new(":memory:"), &V1).unwrap();
        assert_eq!(journal_mode(&memory).await, "memory");
        let dir = tempfile::tempdir().unwrap();
        let file = Database::open(&dir.path().join("wal.sqlite"), &V1).unwrap();
        assert_eq!(journal_mode(&file).await, "wal");
        assert_ne!(memory.identity(), file.identity());
    }
}
