//! Native idle-session allocation and resident-memory profiles.
#![allow(unsafe_code)]
//!
//! NFR-PERF-005 is framework-owned heap (channels, task control blocks, and
//! other scoped allocations). Incremental `ps` RSS is a separate warning
//! metric and must not be treated as that budget. `size_of::<RunTaskOwner>()`
//! is the handle width only.

use std::alloc::{GlobalAlloc, Layout, System};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai::runtime::ports::model::{
    Model, ModelContextProfile, ModelName, ModelResponse, ModelStreamItem, TextDelta,
    TokenEstimatorRef, TokenEstimatorSource,
};
use finstack_ai::{Agent, AgentRunRequest, PrincipalRef, RunSecurityContext};
use finstack_ai_kernel::{AgentId, BundleId, ComponentId, ComponentRef, Version};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::events::EventHubConfig;
use finstack_ai_runtime::ports::model::ApprovalGrantMode;
use finstack_ai_runtime::run::{RunTaskConfig, RunTaskOwner, ShutdownOutcome};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

struct ScopedAlloc;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards to `System`. Accounting uses relaxed atomics
// and is never consulted for aliasing, deallocation, or other soundness.
unsafe impl GlobalAlloc for ScopedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is the caller-supplied allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATED.fetch_add(
                u64::try_from(layout.size()).expect("layout"),
                Ordering::Relaxed,
            );
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        DEALLOCATED.fetch_add(
            u64::try_from(layout.size()).expect("layout"),
            Ordering::Relaxed,
        );
        // SAFETY: `pointer` was allocated with `layout` by this allocator.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is the caller-supplied allocation layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            ALLOCATED.fetch_add(
                u64::try_from(layout.size()).expect("layout"),
                Ordering::Relaxed,
            );
        }
        pointer
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `pointer` was allocated with `layout` by this allocator.
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() {
            let old = u64::try_from(layout.size()).expect("layout");
            let new = u64::try_from(new_size).expect("layout");
            if new >= old {
                ALLOCATED.fetch_add(new.saturating_sub(old), Ordering::Relaxed);
            } else {
                DEALLOCATED.fetch_add(old.saturating_sub(new), Ordering::Relaxed);
            }
        }
        new_pointer
    }
}

#[global_allocator]
static GLOBAL: ScopedAlloc = ScopedAlloc;

fn net_heap_bytes() -> u64 {
    ALLOCATED
        .load(Ordering::Relaxed)
        .saturating_sub(DEALLOCATED.load(Ordering::Relaxed))
}

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

#[derive(Clone, Copy)]
enum Mode {
    Idle,
    Active,
}

fn parse_args() -> (Mode, usize) {
    let mut args = std::env::args()
        .skip(1)
        .filter(|argument| argument != "--bench");
    let mut mode = Mode::Idle;
    let mut sessions = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--mode" => {
                mode = match args.next().as_deref() {
                    Some("idle") => Mode::Idle,
                    Some("active") => Mode::Active,
                    other => panic!("unsupported mode {other:?}"),
                };
            }
            "--sessions" => {
                let count = args
                    .next()
                    .expect("session count")
                    .parse::<usize>()
                    .expect("session count is an integer");
                assert!(count > 0, "session count must be non-zero");
                sessions = Some(count);
            }
            other => panic!("unexpected argument {other}"),
        }
    }
    (mode, sessions.expect("--sessions is required"))
}

fn run_config() -> RunTaskConfig {
    RunTaskConfig {
        command_capacity: 1,
        event_hub: EventHubConfig {
            source_capacity: 1,
            max_subscribers: 1,
        },
        shutdown_deadline: Duration::from_secs(1),
        approval_grant: ApprovalGrantMode::PerCall,
    }
}

async fn measure_idle(sessions: usize) -> (u64, u64, u64, u64) {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions,
            batches_per_session: 1,
            records_per_session: 1,
            snapshot_bytes: 1,
        })
        .expect("memory store"),
    );
    let heap_before = net_heap_bytes();
    let baseline_kib = resident_kib();
    let mut owners = Vec::with_capacity(sessions);
    for _ in 0..sessions {
        owners.push(
            RunTaskOwner::spawn(CommitCoordinator::new(store.clone()), run_config())
                .expect("idle owner"),
        );
    }
    tokio::task::yield_now().await;
    let heap_after = net_heap_bytes();
    let resident_kib = resident_kib();
    let incremental_kib = resident_kib.saturating_sub(baseline_kib);
    let framework_owned_bytes = heap_after.saturating_sub(heap_before);
    for owner in &mut owners {
        let report = owner.shutdown().await;
        assert_eq!(report.outcome, ShutdownOutcome::Graceful);
        assert_eq!(report.aborted_tasks, 0);
    }
    (
        baseline_kib,
        resident_kib,
        incremental_kib,
        framework_owned_bytes,
    )
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("preview-1").expect("model"),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 8_192,
        max_output_tokens: 256,
        reserved_output_tokens: 128,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: Arc::from("scripted.utf8"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn completed(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([finstack_ai_kernel::ContentBlock::Text(
                    finstack_ai_kernel::TextBlock::try_new(text).expect("text"),
                )]),
                tool_calls: Arc::from([]),
                usage: finstack_ai_kernel::Usage::empty(),
                provider_ids: finstack_ai_kernel::ProviderIds::empty(),
                completion_id: Arc::from("active-session"),
                continuation_state: None,
            }))),
        ],
    }
}

fn request(input: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("preview-1").expect("model name"),
        input,
        RunSecurityContext::try_new(
            "active-profile",
            PrincipalRef::try_new("issuer", "subject", Some("active-profile")).expect("principal"),
            "local",
            "test",
            "active-policy",
            "active-decision",
            None,
        )
        .expect("security"),
    )
    .expect("request")
}

async fn measure_active(sessions: usize) -> (u64, u64, u64, u64) {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: sessions.saturating_mul(2).max(8),
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("memory store"),
    );
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("active"); sessions],
    ));
    let agent = Agent::builder(
        AgentId::parse("test.agent.active-profile").expect("agent"),
        BundleId::parse("test.bundle.active-profile").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.active-profile").expect("model"),
                Some(VERSION),
            ),
            Arc::clone(&model) as Arc<dyn Model>,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.active-profile").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .build()
    .await
    .expect("agent");
    let heap_before = net_heap_bytes();
    let baseline_kib = resident_kib();
    let mut joins = Vec::with_capacity(sessions);
    for index in 0..sessions {
        let agent = agent.clone();
        joins.push(tokio::spawn(async move {
            agent
                .run(request(&format!("active session {index}")))
                .await
                .expect("active run")
        }));
    }
    for join in joins {
        join.await.expect("join");
    }
    let heap_after = net_heap_bytes();
    let resident_kib = resident_kib();
    let incremental_kib = resident_kib.saturating_sub(baseline_kib);
    (
        baseline_kib,
        resident_kib,
        incremental_kib,
        heap_after.saturating_sub(heap_before),
    )
}

fn main() {
    let (mode, sessions) = parse_args();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime");
    let label = match mode {
        Mode::Idle => "IDLE_SESSION_MEMORY",
        Mode::Active => "ACTIVE_SESSION_MEMORY",
    };
    let (baseline_kib, resident_kib, incremental_kib, framework_owned_bytes) = match mode {
        Mode::Idle => runtime.block_on(measure_idle(sessions)),
        Mode::Active => runtime.block_on(measure_active(sessions)),
    };
    let session_count = u64::try_from(sessions).expect("session count fits u64");
    let bytes_per_session = incremental_kib
        .saturating_mul(1_024)
        .checked_div(session_count)
        .expect("non-zero sessions");
    let framework_owned_bytes_per_session = framework_owned_bytes
        .checked_div(session_count)
        .expect("non-zero sessions");
    println!(
        "{label} {{\"baseline_kib\":{baseline_kib},\"resident_kib\":{resident_kib},\"incremental_kib\":{incremental_kib},\"sessions\":{sessions},\"bytes_per_session\":{bytes_per_session},\"framework_owned_bytes\":{framework_owned_bytes},\"framework_owned_bytes_per_session\":{framework_owned_bytes_per_session},\"run_task_owner_handle_size_of\":{},\"incremental_rss_is_warning_metric\":true}}",
        std::mem::size_of::<RunTaskOwner>()
    );
}
