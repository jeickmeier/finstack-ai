//! crash-recovery contract `sqlite_ops` leaf-command proofs.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use finstack_ai_kernel::{
    BudgetPropagation, CancellationPropagation, DeadlinePropagation, Digest, Id, IdTag, LaneTag,
    PrincipalPropagation, PrincipalRef, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
    RecordDraft, RecordTag, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
    RunSecurityContext, RunTag, SessionTag, Timestamp,
};
use finstack_ai_runtime::JournalStore;
use finstack_ai_store_sqlite::{
    SCHEMA_USER_VERSION, SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
};

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn unique_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pr048-ops-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

fn open(path: PathBuf) -> SqliteJournalStore {
    SqliteJournalStore::try_open(SqliteStoreConfig {
        path,
        durability: SqliteDurability::Durable,
        limits: SqliteStoreLimits {
            sessions: 4,
            batches_per_session: 16,
            records_per_session: 32,
            snapshot_bytes: 4096,
        },
        busy_timeout: Duration::from_secs(1),
    })
    .expect("open")
}

fn pollster_block<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

fn seed(path: &Path) {
    let store = open(path.to_path_buf());
    let run_id = id::<RunTag>(201);
    let accepted = RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
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
    .expect("accepted");
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(1),
        id::<SessionTag>(1),
        id::<LaneTag>(101),
        Some(run_id),
        Timestamp::from_unix_ms(1).expect("ts"),
        vec![id(301)],
        RecordBody::RunAccepted(accepted),
    )
    .expect("draft");
    pollster_block(
        store.append(
            finstack_ai_kernel::AppendRequest::try_new(id(1), id::<SessionTag>(1), 1, vec![draft])
                .expect("request"),
        ),
    )
    .expect("append");
}

fn ops() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sqlite_ops"))
}

#[test]
fn backup_restore_diagnose_export_import_and_migrate() {
    let dir = unique_dir();
    let src = dir.join("journal.sqlite");
    seed(&src);
    let backup = dir.join("backup.sqlite");
    assert!(
        ops()
            .args([
                "backup",
                src.to_str().expect("src"),
                backup.to_str().expect("dst")
            ])
            .status()
            .expect("backup")
            .success()
    );
    let restored = dir.join("restored.sqlite");
    assert!(
        ops()
            .args([
                "restore",
                backup.to_str().expect("src"),
                restored.to_str().expect("dst"),
            ])
            .status()
            .expect("restore")
            .success()
    );
    let diagnose = ops()
        .args(["diagnose", restored.to_str().expect("db"), "1"])
        .output()
        .expect("diagnose");
    assert!(diagnose.status.success());
    let text = String::from_utf8_lossy(&diagnose.stdout);
    assert!(
        text.contains(&format!("user_version={SCHEMA_USER_VERSION}")),
        "{text}"
    );
    assert!(text.contains("head_sequence=1"), "{text}");
    assert!(text.contains("corruption=none"), "{text}");

    let export = ops()
        .args(["export", restored.to_str().expect("db"), "1"])
        .output()
        .expect("export");
    assert!(export.status.success());
    assert!(!export.stdout.is_empty());

    let imported = dir.join("imported.sqlite");
    let mut import = ops()
        .args(["import", imported.to_str().expect("db"), "1"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("import");
    {
        use std::io::Write;
        import
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(&export.stdout)
            .expect("write");
    }
    assert!(import.wait().expect("wait").success());

    let migrate = ops()
        .args(["migrate", imported.to_str().expect("db")])
        .output()
        .expect("migrate");
    assert!(migrate.status.success());
    assert!(
        String::from_utf8_lossy(&migrate.stdout)
            .contains(&format!("user_version={SCHEMA_USER_VERSION}"))
    );

    let missing = dir.join("missing.sqlite");
    let restore_partial = ops()
        .args([
            "restore",
            missing.to_str().expect("src"),
            dir.join("nope.sqlite").to_str().expect("dst"),
        ])
        .output()
        .expect("partial");
    assert!(!restore_partial.status.success());

    let unknown = dir.join("unknown.sqlite");
    {
        let connection = rusqlite::Connection::open(&unknown).expect("unknown db");
        connection
            .pragma_update(None, "user_version", 99)
            .expect("stamp");
    }
    let migrate_unknown = ops()
        .args(["migrate", unknown.to_str().expect("db")])
        .output()
        .expect("unknown migrate");
    assert!(!migrate_unknown.status.success());
    assert!(String::from_utf8_lossy(&migrate_unknown.stderr).contains("sqlite_schema_unsupported"));
}
