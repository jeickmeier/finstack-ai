//! `SQLite` storage for immutable host recovery payloads.

#[cfg(test)]
use std::sync::Arc;

use finstack_ai_kernel::{LaneId, OperationLocator, RunId, SessionId};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    MAX_RECOVERY_DESCRIPTOR_BYTES, MAX_RECOVERY_SCAN, RecoveryRegistration, RecoveryStore,
    SqliteWorkerStore, WorkerError,
};

const DDL: &str = "
CREATE TABLE IF NOT EXISTS finstack_workflow_recovery_schema (
 singleton INTEGER PRIMARY KEY CHECK (singleton = 1), version INTEGER NOT NULL
);
INSERT OR IGNORE INTO finstack_workflow_recovery_schema VALUES (1, 1);
CREATE TABLE IF NOT EXISTS finstack_workflow_recovery (
 tenant_scope TEXT NOT NULL, run_id TEXT NOT NULL,
 session_id TEXT NOT NULL, lane_id TEXT NOT NULL, descriptor BLOB NOT NULL,
 PRIMARY KEY (tenant_scope, run_id)
);
";

pub(super) fn initialize(conn: &Connection) -> Result<(), WorkerError> {
    conn.execute_batch(DDL).map_err(|_| unavailable())?;
    let version: i64 = conn
        .query_row(
            "SELECT version FROM finstack_workflow_recovery_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| integrity())?;
    if version != 1 {
        return Err(WorkerError::InvalidConfiguration {
            code: "sqlite_worker_recovery_schema_version",
        });
    }
    Ok(())
}

fn unavailable() -> WorkerError {
    WorkerError::StoreUnavailable {
        code: "sqlite_worker_recovery",
    }
}
fn integrity() -> WorkerError {
    WorkerError::StoreIntegrity {
        code: "sqlite_worker_recovery_integrity",
    }
}
fn conflict() -> WorkerError {
    WorkerError::Conflict {
        code: "recovery_descriptor_conflict",
    }
}

type RawRegistration = (String, String, String, Vec<u8>);

impl SqliteWorkerStore {
    /// Whether the exact admitted run already has a scheduling row.
    /// This scalar lookup bounds admission reconciliation independently of
    /// the number of other runs parked in the same tenant.
    ///
    /// # Errors
    /// Returns a storage error when the scheduling table is unavailable.
    pub fn recovery_has_wake(&self, locator: &OperationLocator) -> Result<bool, WorkerError> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM finstack_workflow_worker_wake WHERE tenant_scope = ?1 AND session_id = ?2 AND run_id = ?3)",
                params![locator.tenant_scope.as_ref(), locator.session_id.to_canonical_string(), locator.run_id.to_canonical_string()],
                |row| row.get(0),
            ).map_err(|_| unavailable())
        })
    }
}

fn decode(tenant: &str, raw: RawRegistration) -> Result<RecoveryRegistration, WorkerError> {
    let (run, session, lane, descriptor) = raw;
    let locator = OperationLocator::try_new(
        tenant,
        SessionId::parse(&session).map_err(|_| integrity())?,
        LaneId::parse(&lane).map_err(|_| integrity())?,
        RunId::parse(&run).map_err(|_| integrity())?,
    )
    .map_err(|_| integrity())?;
    let registration = RecoveryRegistration {
        locator,
        descriptor: descriptor.into(),
    };
    registration.validate()?;
    Ok(registration)
}

impl RecoveryStore for SqliteWorkerStore {
    fn insert_recovery(&self, registration: &RecoveryRegistration) -> Result<(), WorkerError> {
        registration.validate()?;
        self.with_conn(|conn| {
            conn.execute("INSERT OR IGNORE INTO finstack_workflow_recovery (tenant_scope, run_id, session_id, lane_id, descriptor) VALUES (?1, ?2, ?3, ?4, ?5)", params![
                registration.locator.tenant_scope.as_ref(), registration.locator.run_id.to_canonical_string(),
                registration.locator.session_id.to_canonical_string(), registration.locator.lane_id.to_canonical_string(), registration.descriptor.as_ref()
            ]).map_err(|_| unavailable())?;
            let raw: RawRegistration = conn.query_row("SELECT run_id, session_id, lane_id, descriptor FROM finstack_workflow_recovery WHERE tenant_scope = ?1 AND run_id = ?2 AND length(descriptor) <= ?3", params![registration.locator.tenant_scope.as_ref(), registration.locator.run_id.to_canonical_string(), MAX_RECOVERY_DESCRIPTOR_BYTES], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).map_err(|_| integrity())?;
            if decode(&registration.locator.tenant_scope, raw)? != *registration { return Err(conflict()); }
            Ok(())
        })
    }

    fn load_recovery(
        &self,
        locator: &OperationLocator,
    ) -> Result<Option<RecoveryRegistration>, WorkerError> {
        self.with_conn(|conn| {
            // Check size in SQL before materializing a corrupted large payload.
            let raw = conn.query_row("SELECT run_id, session_id, lane_id, CASE WHEN length(descriptor) <= ?3 THEN descriptor ELSE NULL END FROM finstack_workflow_recovery WHERE tenant_scope = ?1 AND run_id = ?2", params![locator.tenant_scope.as_ref(), locator.run_id.to_canonical_string(), MAX_RECOVERY_DESCRIPTOR_BYTES], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional().map_err(|_| integrity())?;
            let decoded = raw.map(|raw| decode(&locator.tenant_scope, raw)).transpose()?;
            if decoded.as_ref().is_some_and(|row| row.locator != *locator) { return Err(conflict()); }
            Ok(decoded)
        })
    }

    fn scan_recovery(
        &self,
        tenant_scope: &str,
        after: Option<RunId>,
        limit: usize,
    ) -> Result<Vec<RecoveryRegistration>, WorkerError> {
        if limit == 0 || limit > MAX_RECOVERY_SCAN {
            return Err(WorkerError::InvalidConfiguration {
                code: "recovery_scan_limit",
            });
        }
        self.with_conn(|conn| {
            let mut query = conn.prepare("SELECT run_id, session_id, lane_id, CASE WHEN length(descriptor) <= ?4 THEN descriptor ELSE NULL END FROM finstack_workflow_recovery WHERE tenant_scope = ?1 AND run_id > ?2 ORDER BY run_id LIMIT ?3").map_err(|_| unavailable())?;
            let rows = query.query_map(params![tenant_scope, after.map_or_else(String::new, |id| id.to_canonical_string()), limit, MAX_RECOVERY_DESCRIPTOR_BYTES], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).map_err(|_| unavailable())?;
            rows.map(|row| decode(tenant_scope, row.map_err(|_| integrity())?)).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::Id;

    fn registration() -> RecoveryRegistration {
        RecoveryRegistration {
            locator: OperationLocator::try_new(
                "tenant-a",
                Id::from_bytes([1; 16]),
                Id::from_bytes([2; 16]),
                Id::from_bytes([3; 16]),
            )
            .expect("locator"),
            descriptor: Arc::from(br#"{"version":1}"#.as_slice()),
        }
    }

    #[test]
    fn immutable_recovery_survives_reopen_and_is_scoped() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("host.sqlite");
        let row = registration();
        let store = SqliteWorkerStore::try_open(&path).expect("store");
        store.insert_recovery(&row).expect("insert");
        store.insert_recovery(&row).expect("repeat");
        let mut changed = row.clone();
        changed.descriptor = Arc::from(b"different".as_slice());
        assert_eq!(
            store
                .insert_recovery(&changed)
                .expect_err("immutable")
                .code(),
            "recovery_descriptor_conflict"
        );
        drop(store);
        let store = SqliteWorkerStore::try_open(&path).expect("reopen");
        assert_eq!(
            store.load_recovery(&row.locator).expect("load"),
            Some(row.clone())
        );
        assert!(
            store
                .scan_recovery("tenant-b", None, 1)
                .expect("scoped")
                .is_empty()
        );
        assert_eq!(
            store.scan_recovery("tenant-a", None, 1).expect("scan"),
            vec![row.clone()]
        );
        assert!(
            store
                .scan_recovery("tenant-a", Some(row.locator.run_id), 1)
                .expect("next")
                .is_empty()
        );
        let mut wrong = row.locator;
        wrong.lane_id = Id::from_bytes([4; 16]);
        assert!(store.load_recovery(&wrong).is_err());
    }

    #[test]
    fn unknown_recovery_schema_fails_closed() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("host.sqlite");
        drop(SqliteWorkerStore::try_open(&path).expect("store"));
        Connection::open(&path)
            .expect("db")
            .execute(
                "UPDATE finstack_workflow_recovery_schema SET version = 2",
                [],
            )
            .expect("future");
        assert!(matches!(
            SqliteWorkerStore::try_open(&path),
            Err(WorkerError::InvalidConfiguration {
                code: "sqlite_worker_recovery_schema_version"
            })
        ));
    }

    #[test]
    fn recovery_descriptor_and_scan_bounds_fail_before_materialization() {
        let store = SqliteWorkerStore::try_open(":memory:").expect("store");
        let mut row = registration();
        assert!(!store.recovery_has_wake(&row.locator).expect("no wake"));
        row.descriptor = vec![0; MAX_RECOVERY_DESCRIPTOR_BYTES + 1].into();
        assert_eq!(
            store.insert_recovery(&row).expect_err("bound").code(),
            "recovery_descriptor_size"
        );
        assert!(
            store
                .load_recovery(&row.locator)
                .expect("not written")
                .is_none()
        );
        for limit in [0, MAX_RECOVERY_SCAN + 1] {
            assert_eq!(
                store
                    .scan_recovery("tenant-a", None, limit)
                    .expect_err("scan bound")
                    .code(),
                "recovery_scan_limit"
            );
        }
        store
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO finstack_workflow_recovery VALUES (?1, ?2, ?3, ?4, zeroblob(?5))",
                    params![
                        row.locator.tenant_scope.as_ref(),
                        row.locator.run_id.to_canonical_string(),
                        row.locator.session_id.to_canonical_string(),
                        row.locator.lane_id.to_canonical_string(),
                        MAX_RECOVERY_DESCRIPTOR_BYTES + 1,
                    ],
                )
                .map_err(|_| unavailable())?;
                Ok(())
            })
            .expect("corrupt external file");
        assert!(store.load_recovery(&row.locator).is_err());
        assert!(store.scan_recovery("tenant-a", None, 1).is_err());
    }
}
