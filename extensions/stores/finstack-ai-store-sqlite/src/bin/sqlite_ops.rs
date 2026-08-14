//! Leaf backup, restore, export, import, diagnose, and migrate commands.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{AppendRequest, Id, RecordDraft, RecordEnvelope, SessionId};
use finstack_ai_protocol::{from_diagnostic_json, to_diagnostic_jsonl};
use finstack_ai_runtime::{
    CommitCoordinator, JournalStore, LoadRequest, SCAN_PAGE_MAX_RECORDS, ScanRequest,
};
use finstack_ai_store_sqlite::{
    SCHEMA_USER_VERSION, SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
};
use rusqlite::Connection;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        eprintln!("usage: sqlite_ops <backup|restore|export|import|diagnose|migrate> [args...]");
        return ExitCode::from(2);
    };
    let result = match command.as_str() {
        "backup" => backup(args.next(), args.next()),
        "restore" => restore(args.next(), args.next()),
        "export" => export(args.next(), args.next()),
        "import" => import(args.next(), args.next()),
        "diagnose" => diagnose(args.next(), args.next()),
        "migrate" => migrate(args.next()),
        other => {
            eprintln!("unknown command: {other}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn backup(src: Option<String>, dest: Option<String>) -> Result<(), String> {
    let src = PathBuf::from(src.ok_or("backup <src-db> <dest-db>")?);
    let dest = PathBuf::from(dest.ok_or("backup <src-db> <dest-db>")?);
    copy_trio(&src, &dest, false)
}

fn restore(src: Option<String>, dest: Option<String>) -> Result<(), String> {
    let src = PathBuf::from(src.ok_or("restore <src-db> <dest-db>")?);
    let dest = PathBuf::from(dest.ok_or("restore <src-db> <dest-db>")?);
    if dest.exists() {
        return Err("restore_dest_exists".into());
    }
    copy_trio(&src, &dest, true)
}

fn export(db: Option<String>, session: Option<String>) -> Result<(), String> {
    let store = open_store(PathBuf::from(db.ok_or("export <db> <session-hex>")?))?;
    let session_id = parse_session(&session.ok_or("export <db> <session-hex>")?)?;
    let mut records = Vec::new();
    let mut from_sequence = 0;
    loop {
        let page = pollster_block(store.scan(ScanRequest {
            session_id,
            from_sequence,
            limit: SCAN_PAGE_MAX_RECORDS,
        }))
        .map_err(|error| format!("{error}"))?;
        records.extend(page.records.iter().cloned());
        match page.next_sequence {
            Some(next) => from_sequence = next,
            None => break,
        }
    }
    let jsonl = to_diagnostic_jsonl(&records).map_err(|error| error.to_string())?;
    io::stdout()
        .write_all(jsonl.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn import(db: Option<String>, session: Option<String>) -> Result<(), String> {
    let store = open_store(PathBuf::from(db.ok_or("import <db> <session-hex>")?))?;
    let session_id = parse_session(&session.ok_or("import <db> <session-hex>")?)?;
    let loaded = pollster_block(store.load(LoadRequest { session_id }))
        .map_err(|error| format!("{error}"))?;
    if loaded.head_sequence != 0 {
        return Err("import_session_not_empty".into());
    }
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    let mut expected = 1_u64;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let envelope: RecordEnvelope =
            from_diagnostic_json(line).map_err(|error| format!("import_line_{index}: {error}"))?;
        if envelope.session_id() != session_id {
            return Err("import_session_mismatch".into());
        }
        let draft = RecordDraft::try_new(
            envelope.format_version(),
            envelope.kind_version(),
            envelope.record_id(),
            envelope.session_id(),
            envelope.lane_id(),
            envelope.run_id(),
            envelope.timestamp(),
            envelope.derived_event_ids().to_vec(),
            envelope.body().clone(),
        )
        .map_err(|error| format!("import_draft_{index}: {error}"))?;
        pollster_block(
            store.append(
                AppendRequest::try_new(
                    Id::from_bytes(envelope.record_id().to_bytes()),
                    session_id,
                    expected,
                    vec![draft],
                )
                .map_err(|error| format!("import_append_{index}: {error}"))?,
            ),
        )
        .map_err(|error| format!("import_append_{index}: {error}"))?;
        expected += 1;
    }
    Ok(())
}

fn diagnose(db: Option<String>, session: Option<String>) -> Result<(), String> {
    let path = PathBuf::from(db.ok_or("diagnose <db> <session-hex>")?);
    let session_id = parse_session(&session.ok_or("diagnose <db> <session-hex>")?)?;
    let version = user_version(&path)?;
    let store = open_store(path)?;
    let loaded = pollster_block(store.load(LoadRequest { session_id }))
        .map_err(|error| format!("{error}"))?;
    let recovered = pollster_block(CommitCoordinator::recover(
        Arc::new(store) as Arc<dyn JournalStore>,
        session_id,
    ));
    println!("user_version={version}");
    println!("head_sequence={}", loaded.head_sequence);
    println!(
        "head_checksum={}",
        loaded
            .head_checksum
            .map_or_else(|| "none".into(), |digest| digest.to_string())
    );
    println!(
        "snapshot={}",
        if loaded.accelerated.is_some() {
            "valid"
        } else if loaded.snapshot.is_some() {
            "ignored"
        } else {
            "absent"
        }
    );
    match recovered {
        Ok(coordinator) => {
            let state = coordinator.state();
            println!(
                "phase={}",
                state.phase.map_or_else(
                    || "none".into(),
                    |phase| format!("{phase:?}").to_ascii_lowercase(),
                )
            );
            println!(
                "outstanding={}",
                u64::from(state.pending_model_effect.is_some())
                    + u64::from(state.pending_interaction.is_some())
                    + u64::try_from(
                        state
                            .active_tool_batch
                            .as_ref()
                            .map_or(0, |batch| batch.calls.len())
                    )
                    .unwrap_or(u64::MAX)
            );
            println!(
                "child_mappings={}",
                coordinator.session().child_mappings().len()
            );
            println!("corruption=none");
        }
        Err(error) => {
            println!("phase=uncertain");
            println!("outstanding=0");
            println!("child_mappings=0");
            println!("corruption={error}");
        }
    }
    Ok(())
}

fn migrate(db: Option<String>) -> Result<(), String> {
    let path = PathBuf::from(db.ok_or("migrate <db>")?);
    let before = user_version(&path)?;
    if before != 0 && before != SCHEMA_USER_VERSION {
        return Err(format!("sqlite_schema_unsupported:{before}"));
    }
    let _store = open_store(path.clone())?;
    let after = user_version(&path)?;
    if after != SCHEMA_USER_VERSION {
        return Err(format!("migrate_failed:{after}"));
    }
    println!("user_version={after}");
    Ok(())
}

fn copy_trio(src: &Path, dest: &Path, refuse_partial: bool) -> Result<(), String> {
    if !src.exists() {
        return Err("source_db_missing".into());
    }
    let src_wal = sidecar(src, "-wal");
    let src_shm = sidecar(src, "-shm");
    let dest_wal = sidecar(dest, "-wal");
    let dest_shm = sidecar(dest, "-shm");
    if refuse_partial && (dest_wal.exists() || dest_shm.exists()) {
        return Err("restore_partial_set".into());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(src, dest).map_err(|error| error.to_string())?;
    if src_wal.exists() {
        fs::copy(&src_wal, &dest_wal).map_err(|error| error.to_string())?;
    }
    if src_shm.exists() {
        fs::copy(&src_shm, &dest_shm).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().expect("file name").to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn open_store(path: PathBuf) -> Result<SqliteJournalStore, String> {
    SqliteJournalStore::try_open(SqliteStoreConfig {
        path,
        durability: SqliteDurability::Durable,
        limits: SqliteStoreLimits {
            sessions: 64,
            batches_per_session: 4_096,
            records_per_session: 16_384,
            snapshot_bytes: 1024 * 1024,
        },
        busy_timeout: Duration::from_secs(2),
    })
    .map_err(|error| format!("{error}"))
}

fn user_version(path: &Path) -> Result<i32, String> {
    if !path.exists() {
        return Ok(0);
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())
}

fn parse_session(hex: &str) -> Result<SessionId, String> {
    if hex.len() == 32 {
        let mut bytes = [0_u8; 16];
        for (index, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let text = std::str::from_utf8(chunk).map_err(|error| error.to_string())?;
            bytes[index] = u8::from_str_radix(text, 16).map_err(|error| error.to_string())?;
        }
        return Ok(SessionId::from_bytes(bytes));
    }
    let ordinal: u64 = hex.parse().map_err(|_| "session_id_invalid".to_string())?;
    Ok(id(ordinal))
}

fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
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
