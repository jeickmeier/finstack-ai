//! Executable spec for the tick driving the kernel's credential-free
//! interaction expiry.
//!
//! `finstack_ai_runtime::ingress::InteractionResumeAction::ExpireIfDue`
//! (`crates/finstack-ai-runtime/src/services/interaction.rs`) is applied by
//! `apply_interaction_resume`
//! (`crates/finstack-ai-runtime/src/exec/settlement/interaction.rs`), which
//! every `RunTaskOwner` constructor runs while it attaches
//! (`crates/finstack-ai-runtime/src/exec/task/owner.rs`). The worker therefore
//! submits no input of its own: it only has to *attach* a past-deadline
//! parked session with a clock that is past the committed deadline, and the
//! kernel expires the interaction on its own — no principal, no evidence.
//!
//! What the tick needs, then, is the deadline itself, so it can decide to
//! attach without an inbox entry. `park` records it on the wake row's
//! `expires_at`, and the wake phase treats a past-deadline interaction row as
//! claimable exactly like a due timer row.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AuthorizationEvidence, InteractionKind, InteractionResolution, InteractionResolutionCommand,
    InteractionTerminalOutcome, Metadata, PrincipalRef, ProviderIds, RawJson, ReducerStageOutcome,
    RetrySafety, RunPhase, Stage, Timestamp, ToolExecutionMode, ToolFailurePolicy, ToolId, Usage,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::events::EventHubConfig;
use finstack_ai_runtime::ids::ExternalClock;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::{
    ApprovalGrantMode, ApprovalMetadata, ApprovalRequirement, Model, ModelResponse,
    ModelStreamItem, ModelStreamLimits, ModelToolCall, SideEffectClass, ToolCallDelta,
    ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    JsonSchemaToolValidatorCompiler, ResolvedToolCatalog, ToolExecutionPolicy, ToolPolicyDecision,
    ToolResult, ToolStreamItem, ToolStreamLimits, Toolset, ToolsetRegistration,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy, ToolTaskConfig,
};
use finstack_ai_runtime::workflow::{WorkflowSession, WorkflowWait};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, MemoryWorkerStore, PortsFactory, WakeIndexStore, WakeReason, WakeRow,
    WorkerBuilder, WorkerError, WorkflowWorker, park,
};

use crate::helpers::{
    CounterRandom, completed_plan, drive_to_after_model_with_deadline, env, id, locator,
    locked_profile, memory_store, profile, stage, timestamp, wait_state,
};

/// The committed run deadline, and therefore the approval's `expires_at`.
const DEADLINE_MS: i64 = 4_000;

struct BindPorts {
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
}

impl PortsFactory for BindPorts {
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError> {
        Ok(session.with_ports(
            Arc::clone(&self.model),
            locked_profile(),
            Some(Arc::clone(&self.catalog)),
        ))
    }
}

/// One approval-gated `echo` tool.
fn approval_tools() -> Arc<[ToolSpec]> {
    Arc::from([ToolSpec {
        id: ToolId::parse("finstack.tools.echo").expect("tool id"),
        model_name: Arc::from("echo"),
        title: Arc::from("echo"),
        description: Arc::from("echo"),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"value":{"type":"integer"}},"required":["value"],"type":"object"}"#,
        )
        .expect("input"),
        output_schema: None,
        execution: ToolExecutionMode::Parallel,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    }])
}

/// Catalog whose only tool requires approval, wired to a scripted toolset
/// that must never be called on the expiry path.
fn approval_catalog(tools: &Arc<[ToolSpec]>) -> (Arc<ScriptedToolset>, Arc<ResolvedToolCatalog>) {
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(tools),
        vec![ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(
                ToolResult {
                    output: RawJson::parse(r#"{"ok":true,"value":1}"#).expect("out"),
                    is_error: false,
                },
            )))],
        }],
    ));
    let toolset_port: Arc<dyn Toolset> = Arc::clone(&toolset) as Arc<dyn Toolset>;
    let policies = tools
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                ToolExecutionPolicy {
                    failure_policy: ToolFailurePolicy::ReturnToModel,
                    approval: ToolPolicyDecision::RequireApproval,
                    max_concurrency: 1,
                },
            )
        })
        .collect();
    let catalog = Arc::new(
        ResolvedToolCatalog::try_new(
            [ToolsetRegistration {
                toolset: toolset_port,
                policies,
                components: BTreeMap::new(),
            }],
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .expect("catalog"),
    );
    (toolset, catalog)
}

/// Model that asks for `echo` once, then reports done.
fn approval_model() -> Arc<dyn Model> {
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![
            ScriptedModelPlan {
                actions: vec![
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                        index: 0,
                        name: Some(Arc::from("echo")),
                        arguments_delta: Arc::from(arguments.as_str()),
                        provider_call_id: None,
                    }))),
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                        assistant_content: Arc::from([]),
                        tool_calls: Arc::from([ModelToolCall {
                            name: Arc::from("echo"),
                            arguments,
                            provider_call_id: None,
                        }]),
                        usage: Usage::empty(),
                        provider_ids: ProviderIds::empty(),
                        completion_id: Arc::from("completion-tools"),
                        continuation_state: None,
                    }))),
                ],
            },
            completed_plan("done"),
        ],
    ))
}

/// A worker over `store` with no journal-backed run behind it: every claim it
/// makes lands on an unregistered workflow kind, so a claim is observable as
/// `failures == 1` and a skip as an all-zero report.
fn bare_worker(store: &Arc<MemoryWorkerStore>, clock: &ExternalClock) -> WorkflowWorker {
    WorkerBuilder::new(
        memory_store() as Arc<dyn JournalStore>,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(store) as Arc<dyn WakeIndexStore>,
        Arc::clone(store) as Arc<dyn FireStore>,
        Arc::clone(store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_millis(200))
    .build()
    .expect("worker")
}

/// Synthetic interaction row: no inbox entry, no registered ports factory.
fn interaction_row(session: u64, expires_at: Option<Timestamp>) -> WakeRow {
    WakeRow {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(session),
        lane_id: id(2),
        run_id: id(3),
        workflow_kind: Arc::from("unregistered-kind"),
        reason: WakeReason::Interaction,
        wake_at: None,
        expires_at,
        pending_id: Arc::from("interaction-1"),
        leased_by: None,
        lease_expires_at: None,
        attempts: 0,
    }
}

#[tokio::test]
async fn an_interaction_before_its_deadline_is_never_claimed() {
    let store = Arc::new(MemoryWorkerStore::new());
    store
        .upsert(&interaction_row(11, Some(timestamp(DEADLINE_MS))))
        .expect("row");
    let clock = ExternalClock::new(timestamp(DEADLINE_MS - 1));
    let worker = bare_worker(&store, &clock);

    let report = Box::pin(worker.tick()).await.expect("tick");
    assert_eq!(report.sessions_expired, 0);
    assert_eq!(report.sessions_resumed, 0);
    assert_eq!(
        report.failures, 0,
        "a claim would have failed on the unregistered kind; nothing was claimed"
    );
}

#[tokio::test]
async fn a_deadline_less_interaction_is_never_claimed() {
    let store = Arc::new(MemoryWorkerStore::new());
    store.upsert(&interaction_row(12, None)).expect("row");
    let clock = ExternalClock::new(timestamp(1_000_000));
    let worker = bare_worker(&store, &clock);

    let report = Box::pin(worker.tick()).await.expect("tick");
    assert_eq!(report.sessions_expired, 0);
    assert_eq!(report.sessions_resumed, 0);
    assert_eq!(
        report.failures, 0,
        "no deadline means no clock can ever make the row due"
    );
}

/// Everything one parked, approval-gated run needs: the journal it lives in,
/// the worker over its adapter tables, and the identities the assertions use.
struct Parked {
    journal: Arc<finstack_ai_store_memory::MemoryJournalStore>,
    store: Arc<MemoryWorkerStore>,
    toolset: Arc<ScriptedToolset>,
    clock: ExternalClock,
    worker: WorkflowWorker,
    interaction_id: finstack_ai_kernel::InteractionId,
}

/// Drive a fresh run to an approval interaction carrying the run's effective
/// deadline, park it, and build the worker that will resume it. `seed` keeps
/// two fixtures in one test binary from colliding on generated ids.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors inbox_resume's fixture assembly end to end"
)]
async fn park_on_approval(seed: u64) -> Parked {
    let tools = approval_tools();
    let (toolset, catalog) = approval_catalog(&tools);
    let journal = memory_store();
    let model = approval_model();
    let model_port: Arc<dyn Model> = model.clone();
    let ready_model = Arc::new(
        finstack_ai_runtime::ports::model::ReadyModel::prepare(model_port)
            .await
            .expect("model readiness"),
    );
    let clock = ExternalClock::new(timestamp(2_500));
    let owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(journal.clone()),
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: Duration::from_millis(500),
            approval_grant: ApprovalGrantMode::PerCall,
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        ToolTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        ready_model,
        locked_profile(),
        Arc::clone(&catalog),
        clock.clone(),
        CounterRandom(std::sync::atomic::AtomicU64::new(seed)),
    )
    .await
    .expect("owner");
    drive_to_after_model_with_deadline(
        &owner.handle(),
        &journal,
        Arc::clone(&tools),
        Some(timestamp(DEADLINE_MS)),
    )
    .await;
    owner
        .handle()
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    wait_state(&journal, |state| {
        state.phase == Some(RunPhase::AwaitingInteraction)
    })
    .await;
    drop(owner);

    // ACT ONE: park the approval. The committed request carries the run's
    // effective deadline, and `park` copies it onto the wake row.
    let mut session =
        WorkflowSession::trusted_seeded(journal.clone(), locator(), clock.clone(), seed + 1)
            .await
            .expect("attach")
            .with_ports(
                Arc::clone(&model),
                locked_profile(),
                Some(Arc::clone(&catalog)),
            );
    let WorkflowWait::Interaction {
        interaction_id,
        request,
    } = session.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected an interaction wait");
    };
    assert_eq!(*request.kind(), InteractionKind::Approval);
    assert_eq!(
        request.expires_at(),
        Some(timestamp(DEADLINE_MS)),
        "the run's effective deadline is the approval's deadline"
    );
    let store = Arc::new(MemoryWorkerStore::new());
    park(&mut session, store.as_ref(), "research").expect("park");
    drop(session);
    let parked = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].reason, WakeReason::Interaction);
    assert_eq!(
        parked[0].wake_at, None,
        "an interaction is not clock-scheduled; `wake_at` stays the retry gate"
    );
    assert_eq!(
        parked[0].expires_at,
        Some(timestamp(DEADLINE_MS)),
        "the committed deadline is indexed for the tick"
    );

    let worker = WorkerBuilder::new(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_millis(300))
    .register_ports(
        "research",
        Arc::new(BindPorts {
            model: Arc::clone(&model),
            catalog: Arc::clone(&catalog),
        }),
    )
    .build()
    .expect("worker");

    Parked {
        journal,
        store,
        toolset,
        clock,
        worker,
        interaction_id,
    }
}

#[tokio::test]
async fn a_past_deadline_interaction_is_expired_by_the_tick() {
    let Parked {
        journal,
        store,
        toolset,
        clock,
        worker,
        interaction_id,
    } = Box::pin(park_on_approval(900)).await;

    // ACT TWO: before the deadline, nobody has answered and nothing is due.
    let before = Box::pin(worker.tick()).await.expect("before the deadline");
    assert_eq!(before.sessions_expired, 0);
    assert_eq!(before.sessions_resumed, 0);
    assert_eq!(before.failures, 0);
    let still_pending = CommitCoordinator::recover(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator().session_id,
    )
    .await
    .expect("recover");
    assert!(
        still_pending.state().pending_interaction.is_some(),
        "an unanswered approval before its deadline stays parked"
    );

    // ACT THREE: past the deadline, with still no inbox entry, the tick
    // attaches and the kernel expires the interaction on its own.
    clock.set(timestamp(DEADLINE_MS + 100));
    let expired = Box::pin(worker.tick()).await.expect("past the deadline");
    assert_eq!(
        expired.sessions_expired, 1,
        "the tick drove the kernel's credential-free expiry"
    );
    assert_eq!(expired.failures, 0);
    assert_eq!(
        expired.sessions_resumed, 1,
        "the expired run is driven past the interaction it was parked on"
    );
    let settled = CommitCoordinator::recover(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator().session_id,
    )
    .await
    .expect("recover");
    assert!(
        settled.state().pending_interaction.is_none(),
        "the pending approval is settled"
    );
    assert_eq!(
        settled
            .state()
            .last_interaction_terminal
            .as_ref()
            .map(|terminal| terminal.outcome),
        Some(InteractionTerminalOutcome::Expired),
        "the kernel classified the settlement as an expiry, not a decision"
    );
    assert!(
        settled.state().resolution_identities.is_empty(),
        "expiry is a deadline event: no principal resolved anything"
    );
    assert_eq!(
        toolset.call_count(),
        0,
        "an expired approval never executes the tool"
    );
    assert_eq!(
        store.load_batch(10).expect("inbox").len(),
        0,
        "the expiry path consumes no inbox entry"
    );
    assert_eq!(
        expired.sessions_reparked, 0,
        "this run has nothing left to wait for once its approval expires"
    );
    assert!(
        matches!(settled.state().phase, Some(RunPhase::Failed)),
        "the run whose deadline expired its approval is terminal"
    );
    assert!(
        store.load_tenant("tenant-a").expect("rows").is_empty(),
        "a terminal state deletes the wake row"
    );
    let _ = interaction_id;

    // Re-ticking is harmless: the row the expiry classified is gone, and a
    // replayed one could not classify as that interaction wait again.
    clock.jump(120_000).expect("later tick");
    let again = Box::pin(worker.tick()).await.expect("re-tick");
    assert_eq!(
        again.sessions_expired, 0,
        "an already expired interaction cannot double-fire"
    );
    assert_eq!(again.failures, 0);
}

/// A resolution buffered before the deadline but ticked after it does not
/// beat the deadline: the interaction ingress is fail-closed on a late answer.
/// `interaction_settled_input`
/// (`crates/finstack-ai-runtime/src/driver/ingress/shared.rs`) rewrites a
/// resolution whose `submitted_at` is at or after `expires_at` into
/// `InteractionSettled::Expired` before the reducer sees it, so the row is
/// settled `Expired` and `sessions_expired` counts it — the expiry is real.
#[tokio::test]
async fn a_resolution_that_races_the_deadline_still_expires_and_is_counted() {
    let Parked {
        journal,
        store,
        toolset,
        clock,
        worker,
        interaction_id,
    } = Box::pin(park_on_approval(920)).await;

    // Delivered while the worker was down, before the deadline. The inbox
    // buffers it; nothing is submitted to the journal yet.
    let command = InteractionResolutionCommand::try_new(
        locator(),
        InteractionResolution::try_new(
            interaction_id,
            "resolution-1",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
            RawJson::parse(r#"{"approved":true}"#).expect("response"),
            None::<&str>,
        )
        .expect("resolution"),
    )
    .expect("command");
    worker
        .deliver_interaction(&command, timestamp(DEADLINE_MS - 500))
        .expect("deliver interaction");

    // The tick only runs once the deadline has also passed, so the row is due
    // on both counts at once and `submit_response` submits the buffered
    // resolution with a past-deadline `submitted_at`.
    clock.set(timestamp(DEADLINE_MS + 100));
    let report = Box::pin(worker.tick()).await.expect("tick");
    assert_eq!(
        report.sessions_resumed, 1,
        "the row is claimed and driven exactly as an ordinary inbox delivery"
    );
    assert_eq!(
        report.sessions_expired, 1,
        "an expiry really was committed for this row on this tick"
    );

    let settled = CommitCoordinator::recover(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator().session_id,
    )
    .await
    .expect("recover");
    assert!(
        settled.state().pending_interaction.is_none(),
        "the approval is settled"
    );
    assert_eq!(
        settled
            .state()
            .last_interaction_terminal
            .as_ref()
            .map(|terminal| terminal.outcome),
        Some(InteractionTerminalOutcome::Expired),
        "a late answer is refused: the deadline settles it, not the principal"
    );
    assert!(
        settled.state().resolution_identities.is_empty(),
        "no resolution identity is recorded for a refused late answer"
    );
    assert_eq!(
        toolset.call_count(),
        0,
        "the tool the approval gated never runs"
    );
    assert!(
        store.load_batch(10).expect("inbox").is_empty(),
        "the consumed response is drained from the inbox"
    );
}
