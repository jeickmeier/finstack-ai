//! Native idle-session resident-memory baseline.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::{
    CommitCoordinator, EventHubConfig, RunTaskConfig, RunTaskOwner, ShutdownOutcome,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

fn resident_kib() -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps must be available on supported benchmark hosts");
    assert!(output.status.success(), "ps failed");
    String::from_utf8(output.stdout)
        .expect("ps output is utf-8")
        .trim()
        .parse()
        .expect("ps rss is an integer KiB value")
}

fn session_count() -> usize {
    let mut args = std::env::args()
        .skip(1)
        .filter(|argument| argument != "--bench");
    assert_eq!(args.next().as_deref(), Some("--sessions"));
    let count = args
        .next()
        .expect("session count")
        .parse::<usize>()
        .expect("session count is an integer");
    assert!(count > 0, "session count must be non-zero");
    assert!(args.next().is_none(), "unexpected idle-memory argument");
    count
}

fn run_config() -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: 1,
        event_hub: EventHubConfig {
            source_capacity: 1,
            max_subscribers: 1,
        },
        shutdown_deadline: Duration::from_secs(1),
    }
}

async fn measure(sessions: usize) -> (u64, u64, u64) {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions,
            batches_per_session: 1,
            records_per_session: 1,
            snapshot_bytes: 1,
        })
        .expect("memory store"),
    );
    let baseline_kib = resident_kib();
    let mut owners = Vec::with_capacity(sessions);
    for _ in 0..sessions {
        owners.push(
            RunTaskOwner::spawn(CommitCoordinator::new(store.clone()), run_config())
                .expect("idle owner"),
        );
    }
    tokio::task::yield_now().await;
    let resident_kib = resident_kib();
    let incremental_kib = resident_kib.saturating_sub(baseline_kib);
    for owner in &mut owners {
        let report = owner.shutdown().await;
        assert_eq!(report.outcome, ShutdownOutcome::Graceful);
        assert_eq!(report.aborted_tasks, 0);
    }
    (baseline_kib, resident_kib, incremental_kib)
}

fn main() {
    let sessions = session_count();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    let (baseline_kib, resident_kib, incremental_kib) = runtime.block_on(measure(sessions));
    let bytes_per_session = incremental_kib
        .saturating_mul(1_024)
        .checked_div(u64::try_from(sessions).expect("session count fits u64"))
        .expect("non-zero sessions");
    println!(
        "IDLE_SESSION_MEMORY {{\"baseline_kib\":{baseline_kib},\"resident_kib\":{resident_kib},\"incremental_kib\":{incremental_kib},\"sessions\":{sessions},\"bytes_per_session\":{bytes_per_session}}}"
    );
}
