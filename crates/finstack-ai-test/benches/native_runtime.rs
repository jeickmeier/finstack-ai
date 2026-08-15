//! Native idle-session and active-session resident-memory profiles.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai::runtime::{
    AgentId, BundleId, ComponentId, ComponentRef, JournalStore, Model, ModelContextProfile,
    ModelName, ModelResponse, ModelStreamItem, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    Version,
};
use finstack_ai::{Agent, AgentRunRequest, PrincipalRef, RunSecurityContext};
use finstack_ai_runtime::{
    CommitCoordinator, EventHubConfig, RunTaskConfig, RunTaskOwner, ShutdownOutcome,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

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
    }
}

async fn measure_idle(sessions: usize) -> (u64, u64, u64) {
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
                assistant_content: Arc::from([finstack_ai::runtime::ContentBlock::Text(
                    finstack_ai::runtime::TextBlock::try_new(text).expect("text"),
                )]),
                tool_calls: Arc::from([]),
                usage: finstack_ai::runtime::Usage::empty(),
                provider_ids: finstack_ai::runtime::ProviderIds::empty(),
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

async fn measure_active(sessions: usize) -> (u64, u64, u64) {
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
    let resident_kib = resident_kib();
    let incremental_kib = resident_kib.saturating_sub(baseline_kib);
    (baseline_kib, resident_kib, incremental_kib)
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
    let (baseline_kib, resident_kib, incremental_kib) = match mode {
        Mode::Idle => runtime.block_on(measure_idle(sessions)),
        Mode::Active => runtime.block_on(measure_active(sessions)),
    };
    let bytes_per_session = incremental_kib
        .saturating_mul(1_024)
        .checked_div(u64::try_from(sessions).expect("session count fits u64"))
        .expect("non-zero sessions");
    println!(
        "{label} {{\"baseline_kib\":{baseline_kib},\"resident_kib\":{resident_kib},\"incremental_kib\":{incremental_kib},\"sessions\":{sessions},\"bytes_per_session\":{bytes_per_session},\"run_task_owner_size_of\":{}}}",
        std::mem::size_of::<RunTaskOwner>()
    );
}
