//! Process-kill helper for `SQLite` process-kill fault case. Not a public product surface.

#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process;
use std::time::Duration;

use finstack_ai_kernel::{
    AuthorizationEvidence, Digest, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, Id, IdTag, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordTag, RunTag, SessionTag,
};
use finstack_ai_runtime::JournalStore;
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
};

type BoxError = Box<dyn Error + Send + Sync>;

fn main() -> Result<(), BoxError> {
    let mut args = env::args().skip(1);
    let mode = args.next().ok_or("missing helper mode")?;
    let path = PathBuf::from(args.next().ok_or("missing database path")?);
    match mode.as_str() {
        "commit-then-abort" => commit_then_abort(path),
        other => Err(format!("unknown helper mode: {other}").into()),
    }
}

fn commit_then_abort(path: PathBuf) -> Result<(), BoxError> {
    let store = SqliteJournalStore::try_open(SqliteStoreConfig {
        path,
        durability: SqliteDurability::Durable,
        limits: SqliteStoreLimits {
            sessions: 4,
            batches_per_session: 8,
            records_per_session: 16,
            snapshot_bytes: 1024,
        },
        busy_timeout: Duration::from_secs(1),
    })?;
    let principal = PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))?;
    let authorization = AuthorizationEvidence::try_new("policy-v1", "decision-v1")?;
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        "completion-1",
        ExternalCommandTarget::Effect(id(1_000)),
        principal,
        authorization,
        "conflicting_completion",
        Digest::raw_json(b"{}"),
        None,
    )?;
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(1),
        id::<SessionTag>(1),
        id::<LaneTag>(101),
        Some(id::<RunTag>(201)),
        finstack_ai_kernel::Timestamp::from_unix_ms(1).unwrap_or(finstack_ai_kernel::UNIX_EPOCH),
        Vec::new(),
        RecordBody::ExternalCommandRejected(rejection),
    )?;
    let request =
        finstack_ai_kernel::AppendRequest::try_new(id(1), id::<SessionTag>(1), 1, vec![draft])?;
    pollster_block(store.append(request))?;
    println!("acked");
    io::stdout().flush()?;
    process::abort();
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
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
