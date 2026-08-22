//! Stock daemon binary for [`finstack_ai_workflow_worker`].
//!
//! Opens the three sqlite adapter tables (worker, cron, journal) on one
//! file, builds a [`WorkerBuilder`] with no `PortsFactory` or `RunStarter`
//! registered, and spawns the tick loop until `ctrl-c`.
//!
//! With nothing registered the only phase that does useful work is cron:
//! due schedules are claimed and recorded as fires, which then wait for a
//! host with a matching `RunStarter`. This daemon touches no parked session
//! at all — resuming one, including reaping a wake row whose run already
//! reached a terminal state, needs host-owned ports (a model, tools,
//! middleware) that only an embedding host can supply, so every due wake row
//! is counted as a failure and backed off until such a host runs.
//! Usage: `finstack_workflow_worker <sqlite-path>`.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_store_sqlite::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteJournalStore, SqliteStoreConfig,
    SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_workflow_local::SqliteCronStore;
use finstack_ai_workflow_worker::{SqliteWorkerStore, WorkerBuilder};

/// Journal resource ceilings used by the stock daemon.
const JOURNAL_LIMITS: SqliteStoreLimits = SqliteStoreLimits {
    sessions: 10_000,
    batches_per_session: 512,
    records_per_session: 4_096,
    snapshot_bytes: 256 * 1024,
};

/// Poll interval between ticks.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: finstack_workflow_worker <sqlite-path>");
        std::process::exit(2);
    });
    let store = Arc::new(SqliteWorkerStore::try_open(&path).expect("worker store"));
    let cron = Arc::new(SqliteCronStore::try_open(&path).expect("cron store"));
    let durability = if path == ":memory:" {
        SqliteDurability::Relaxed {
            synchronous: SqliteSynchronous::Normal,
        }
    } else {
        SqliteDurability::Durable
    };
    let journal = Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: path.clone().into(),
            durability,
            limits: JOURNAL_LIMITS,
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("journal"),
    );
    let worker = Arc::new(
        WorkerBuilder::new(
            journal,
            cron,
            Arc::clone(&store) as _,
            Arc::clone(&store) as _,
            Arc::clone(&store) as _,
        )
        .build()
        .expect("worker"),
    );
    let handle = worker.spawn(POLL_INTERVAL);
    tokio::signal::ctrl_c().await.expect("signal");
    handle.shutdown().await;
}
