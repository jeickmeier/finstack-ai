//! End-to-end proofs that a response delivered while the worker is down is
//! resumed on a later tick, driven entirely through `WorkflowWorker::tick`.
//!
//! Both fixtures here settle inside `Stage::BeforeToolBatch` (a tool-call
//! approval interaction, and a deferred tool completion). Resolving either
//! one unconditionally lands the run's kernel phase on
//! `RunPhase::AfterToolBatch`
//! (`crates/finstack-ai-kernel/src/reducer/apply/tools.rs::apply_tool_batch_closed`
//! sets it regardless of `ToolBatchContinuation`), and advancing past that
//! phase requires an externally submitted
//! `KernelInput::StageSettled { cursor: .. AfterModel | AfterToolBatch .., outcome: Continue }`.
//! That decision is normally made by an application-level facade
//! (`crates/finstack-ai/src/agent/drive.rs` is the only non-test caller of
//! `settle_facade_stage`/`settle_facade_stage_with_model` in the workspace);
//! neither `finstack-ai-runtime`'s `RunTaskOwner` task loop nor
//! `WorkflowSession::ensure_owner`/`respawn_owner` submits it on its own, and
//! `finstack-ai-workflow-worker` depends on neither `finstack-ai` nor any
//! other source of that decision. This is *not* a bug in `WorkflowWorker`:
//! it is a genuinely separate responsibility, so each test's middle act
//! proves the worker's own contract on its own terms — a claimed row whose
//! run cannot reach a new wait within budget is a counted failure with its
//! response preserved for redelivery, exactly like Task 9's
//! `tick_keeps_the_inbox_entry_when_the_resume_cannot_park`
//! (`tests/worker/tick.rs`) — before a final act
//! (`drive_past_missing_facade_decisions`) stands in for that missing
//! facade, so the very same worker can be shown driving the very same run
//! the rest of the way to `sessions_resumed == 1` with a drained inbox and a
//! deleted wake row, matching the brief's original assertions in full.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AuthorizationEvidence, ComponentId, ContentBlock, Digest, EffectOutputContract,
    EffectOutputKind, ExternalEffectCompletion, ExternalEffectCompletionCommand,
    ExternalEffectOutcome, ExternalHandleRef, InteractionResolution, InteractionResolutionCommand,
    Message, Metadata, PrincipalRef, ProviderIds, RawJson, ReconciliationPolicy,
    ReducerStageOutcome, RetrySafety, RunPhase, Stage, ToolExecutionMode, ToolFailurePolicy,
    ToolId, ToolResultBlock, Usage,
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
    JsonSchemaToolValidatorCompiler, ResolvedToolCatalog, ToolDeferral, ToolExecutionPolicy,
    ToolPolicyDecision, ToolResult, ToolStreamItem, ToolStreamLimits, Toolset, ToolsetRegistration,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy, ToolTaskConfig,
};
use finstack_ai_runtime::workflow::{WorkflowSession, WorkflowWait};
use finstack_ai_store_memory::MemoryJournalStore;
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, MemoryWorkerStore, PortsFactory, WakeIndexStore, WorkerBuilder,
    WorkerError, park_for_wake,
};

use crate::helpers::{
    CounterRandom, completed_plan, draft, drive_to_after_model, env, locator, locked_profile,
    memory_store, profile, stage, stage_at, timestamp, wait_state,
};

/// Stands in for the missing application-level facade (see the module doc):
/// spawns a temporary raw owner on `journal` and submits the exact sequence
/// of `StageSettled` decisions (`AfterToolBatch` -> `PrepareContext` ->
/// `BeforeModel` -> `AfterModel` -> `BeforeFinalize`) that carries a run from
/// `RunPhase::AfterToolBatch` — where the worker's own failed tick left it —
/// through the second model cycle and on to `RunPhase::Completed`. `tools`
/// must be the same catalog-bound spec list the fixture's run was built
/// with; `model`'s plan queue must have exactly one unconsumed plan left
/// (the follow-up "done" completion).
#[expect(
    clippy::too_many_lines,
    reason = "replicates the full BeforeToolBatch -> ... -> BeforeFinalize facade sequence"
)]
async fn drive_past_missing_facade_decisions(
    journal: &Arc<MemoryJournalStore>,
    model: &Arc<dyn Model>,
    tools: &Arc<[ToolSpec]>,
    catalog: &Arc<ResolvedToolCatalog>,
    clock: &ExternalClock,
    seed: u64,
) {
    let dyn_journal = Arc::clone(journal) as Arc<dyn JournalStore>;
    let recovered_coordinator =
        CommitCoordinator::recover(Arc::clone(&dyn_journal), locator().session_id)
            .await
            .expect("recover for facade");
    let ready_model = Arc::new(
        finstack_ai_runtime::ports::model::ReadyModel::prepare(Arc::clone(model))
            .await
            .expect("model readiness"),
    );
    let facade = RunTaskOwner::spawn_with_model_and_tools(
        recovered_coordinator,
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
        Arc::clone(catalog),
        clock.clone(),
        CounterRandom(std::sync::atomic::AtomicU64::new(seed)),
    )
    .await
    .expect("facade owner");

    let before_facade = CommitCoordinator::recover(Arc::clone(&dyn_journal), locator().session_id)
        .await
        .expect("recover");
    assert_eq!(
        before_facade.state().phase(),
        Some(RunPhase::AfterToolBatch),
        "the facade only needs to unblock AfterToolBatch"
    );
    let cycle = before_facade.state().cycle();

    facade
        .handle()
        .submit(
            env(3_100, &[300], &[], &[], &[], &[], &[], 301),
            stage_at(cycle, Stage::AfterToolBatch, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after tool batch");
    wait_state(journal, |state| {
        state.phase() == Some(RunPhase::PreparingContext)
    })
    .await;

    let next_cycle = cycle + 1;
    let messages: Arc<[Message]> = Arc::from(
        CommitCoordinator::recover(Arc::clone(&dyn_journal), locator().session_id)
            .await
            .expect("recover")
            .state()
            .messages()
            .as_slice(),
    );
    facade
        .handle()
        .submit(
            env(3_200, &[302, 303], &[], &[], &[304], &[], &[], 305),
            stage_at(
                next_cycle,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::clone(&messages),
                },
            ),
        )
        .await
        .expect("context prepared");

    let raw = RawJson::parse(
        draft(messages, Arc::clone(tools))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    facade
        .handle()
        .submit(
            env(3_300, &[306, 307], &[308], &[309], &[], &[310], &[], 311),
            stage_at(
                next_cycle,
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: raw,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(5_000)),
                },
            ),
        )
        .await
        .expect("model request");
    wait_state(journal, |state| state.phase() == Some(RunPhase::AfterModel)).await;

    facade
        .handle()
        .submit(
            env(3_400, &[312], &[], &[], &[], &[], &[], 313),
            stage_at(next_cycle, Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model, cycle 2");
    wait_state(journal, |state| {
        state.phase() == Some(RunPhase::BeforeFinalize)
    })
    .await;

    facade
        .handle()
        .submit(
            env(3_500, &[314, 315], &[316], &[], &[], &[], &[], 317),
            stage_at(
                next_cycle,
                Stage::BeforeFinalize,
                ReducerStageOutcome::FinalizeAccepted,
            ),
        )
        .await
        .expect("finalize");
    wait_state(journal, |state| state.terminal().is_some()).await;
    drop(facade);
}

/// Completion command settling a deferred *tool* effect. Unlike the generic
/// `completion_command` fixture in `helpers/mod.rs` (built for a deferred
/// *model* request, whose output is free-form), a tool completion's output
/// must decode as a [`ToolResultBlock`] matching the original
/// `tool_call_id` — `crates/finstack-ai-kernel/src/reducer/tool/records.rs::decode_tool_result`
/// enforces this, rejecting anything else as `tool_result_mismatch`.
fn tool_completion_command(
    effect_id: finstack_ai_kernel::EffectId,
    tool_call_id: finstack_ai_kernel::ToolCallId,
) -> ExternalEffectCompletionCommand {
    let result = ToolResultBlock::try_new(
        tool_call_id,
        vec![ContentBlock::Json(finstack_ai_kernel::JsonBlock::new(
            RawJson::parse(r#"{"ok":true,"value":1}"#).expect("output"),
        ))],
        false,
    )
    .expect("tool result");
    let output = RawJson::parse(serde_json::to_string(&result).expect("encode")).expect("output");
    ExternalEffectCompletionCommand::try_new(
        locator(),
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            "ext-1",
            ExternalEffectOutcome::Completed {
                output,
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("completion"),
    )
    .expect("command")
}

/// Binds the fixture model (and, for the interaction test, tool catalog)
/// onto an attached session, exactly like the `BindPorts` used in Task 9.
struct BindPorts {
    model: Arc<dyn Model>,
    catalog: Option<Arc<ResolvedToolCatalog>>,
}

impl PortsFactory for BindPorts {
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError> {
        Ok(session.with_ports(
            Arc::clone(&self.model),
            locked_profile(),
            self.catalog.clone(),
        ))
    }
}

/// Deferred-effect completions can only settle a *known tool effect*: the
/// ingress router (`crates/finstack-ai-runtime/src/driver/ingress/completion.rs`)
/// rejects a `Completed` outcome outright unless `effect_id` is registered in
/// `tool_calls`, so a model-request deferral (as in
/// `deferred_survives_worker_restart`) can never be settled this way — the
/// generic completion path is for deferred *tools*, not deferred model
/// requests. This fixture therefore defers a tool call instead: the model
/// requests `echo`, the toolset defers it, and the second plan is the
/// follow-up request the run makes once the tool result comes back.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "mirrors deferred_survives_worker_restart's fixture assembly"
)]
async fn deferred_completion_delivered_while_down_resumes_on_tick() {
    let tools: Arc<[ToolSpec]> = Arc::from([ToolSpec {
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
        deferral: ToolDeferralSupport::Supported,
    }]);
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(&tools),
        vec![ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Deferred(
                ToolDeferral {
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.tool.scripted").expect("component"),
                        "job-1",
                        RawJson::parse(b"{}").expect("metadata"),
                    )
                    .expect("handle"),
                    reconciliation: ReconciliationPolicy::CallbackOnly,
                    next_poll_at: None,
                    expires_at: None,
                },
            )))],
        }],
    ));
    let toolset_port: Arc<dyn Toolset> = toolset;
    let policies = tools
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                ToolExecutionPolicy {
                    failure_policy: ToolFailurePolicy::ReturnToModel,
                    approval: ToolPolicyDecision::Allow,
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
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    let journal = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
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
    ));
    let clock = ExternalClock::new(timestamp(2_500));
    let ready_model = Arc::new(
        finstack_ai_runtime::ports::model::ReadyModel::prepare(Arc::clone(&model))
            .await
            .expect("model readiness"),
    );
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
        CounterRandom(std::sync::atomic::AtomicU64::new(740)),
    )
    .await
    .expect("owner");
    drive_to_after_model(&owner.handle(), &journal, Arc::clone(&tools)).await;
    owner
        .handle()
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    wait_state(&journal, |state| {
        state.phase() == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);

    let mut session =
        WorkflowSession::trusted_seeded(journal.clone(), locator(), clock.clone(), 741)
            .await
            .expect("attach")
            .with_ports(
                Arc::clone(&model),
                locked_profile(),
                Some(Arc::clone(&catalog)),
            );
    let WorkflowWait::DeferredEffect { effect_id, .. } =
        session.drive_until_wait().await.expect("deferred")
    else {
        panic!("expected a deferred wait");
    };
    let tool_call_id = *session
        .last_state()
        .tool_calls()
        .iter()
        .find(|(_, identity)| identity.effect_id == Some(effect_id))
        .map(|(id, _)| id)
        .expect("tool call id");
    let store = Arc::new(MemoryWorkerStore::new());
    park_for_wake(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    let worker = WorkerBuilder::new(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_millis(500))
    .register_ports(
        "research",
        Arc::new(BindPorts {
            model: Arc::clone(&model),
            catalog: Some(Arc::clone(&catalog)),
        }),
    )
    .build()
    .expect("worker");

    // No response has been delivered yet: the row is a non-timer wait, so
    // the claim gate must skip it, claiming nothing (src/worker.rs's
    // `tick_wake` gate on a missing inbox entry).
    let before = Box::pin(worker.tick()).await.expect("before delivery");
    assert_eq!(before.sessions_resumed, 0);
    assert_eq!(before.failures, 0);
    assert!(
        store.load_batch(10).expect("inbox").is_empty(),
        "no response has been delivered yet"
    );

    worker
        .deliver_external(
            &tool_completion_command(effect_id, tool_call_id),
            timestamp(3_000),
        )
        .expect("deliver external");

    // The claimed row is driven far enough to apply the completion (the tool
    // effect settles), but — see the module doc — the run cannot reach a new
    // park-able wait within this tick, so it is counted as a failure and
    // preserved for a later tick rather than being dropped.
    let after = Box::pin(worker.tick()).await.expect("after delivery");
    assert_eq!(after.sessions_resumed, 0);
    assert_eq!(after.failures, 1);
    let recovered = CommitCoordinator::recover(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator().session_id,
    )
    .await
    .expect("recover");
    assert!(
        recovered
            .state()
            .tool_settlements()
            .contains_key(&effect_id),
        "the delivered completion was applied to the journal"
    );
    assert_eq!(
        store.load_batch(10).expect("inbox").len(),
        1,
        "an unparked resume keeps the delivered response for redelivery"
    );
    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1, "the row survives for a later tick");

    // THIRD ACT: prove the worker's own claim is genuine, not merely a
    // permanent-retry design. Once *something else* supplies the missing
    // stage decisions the worker cannot make on its own, the very same
    // worker resumes the run to completion, drains the inbox, and clears
    // the wake row — exactly the brief's original assertions.
    drive_past_missing_facade_decisions(&journal, &model, &tools, &catalog, &clock, 745).await;

    clock.jump(120_000).expect("past backoff");
    let completed = Box::pin(worker.tick()).await.expect("resume to completion");
    assert_eq!(
        completed.sessions_resumed, 1,
        "the resume is claimed and driven to completion"
    );
    assert!(
        store.load_batch(10).expect("inbox").is_empty(),
        "the consumed response is drained from the inbox"
    );
    assert!(
        store.load_tenant("tenant-a").expect("rows").is_empty(),
        "the wake row is deleted once the run reaches a terminal state"
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "mirrors interaction_survives_worker_restart's fixture assembly"
)]
async fn interaction_resolution_delivered_while_down_resumes_on_tick() {
    let tools: Arc<[ToolSpec]> = Arc::from([ToolSpec {
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
    }]);
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(&tools),
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
    let toolset_port: Arc<dyn Toolset> = toolset;
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
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    let journal = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
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
    ));
    let clock = ExternalClock::new(timestamp(2_500));
    let ready_model = Arc::new(
        finstack_ai_runtime::ports::model::ReadyModel::prepare(Arc::clone(&model))
            .await
            .expect("model readiness"),
    );
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
        CounterRandom(std::sync::atomic::AtomicU64::new(700)),
    )
    .await
    .expect("owner");
    drive_to_after_model(&owner.handle(), &journal, Arc::clone(&tools)).await;
    owner
        .handle()
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    wait_state(&journal, |state| {
        state.phase() == Some(RunPhase::AwaitingInteraction)
    })
    .await;
    drop(owner);

    let mut session =
        WorkflowSession::trusted_seeded(journal.clone(), locator(), clock.clone(), 701)
            .await
            .expect("attach")
            .with_ports(
                Arc::clone(&model),
                locked_profile(),
                Some(Arc::clone(&catalog)),
            );
    let WorkflowWait::Interaction { interaction_id, .. } =
        session.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected an interaction wait");
    };
    let store = Arc::new(MemoryWorkerStore::new());
    park_for_wake(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    let worker = WorkerBuilder::new(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_millis(500))
    .register_ports(
        "research",
        Arc::new(BindPorts {
            model: Arc::clone(&model),
            catalog: Some(Arc::clone(&catalog)),
        }),
    )
    .build()
    .expect("worker");

    // No response has been delivered yet: the row is a non-timer wait, so
    // the claim gate must skip it, claiming nothing (src/worker.rs's
    // `tick_wake` gate on a missing inbox entry).
    let before = Box::pin(worker.tick()).await.expect("no response yet");
    assert_eq!(before.sessions_resumed, 0);
    assert_eq!(before.failures, 0);
    assert!(
        store.load_batch(10).expect("inbox").is_empty(),
        "no response has been delivered yet"
    );

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
        .deliver_interaction(&command, timestamp(3_000))
        .expect("deliver interaction");

    // As above: the claimed row is driven far enough to apply the
    // resolution, but cannot reach a new park-able wait within this tick, so
    // it is a failure and the row/response are preserved for a later tick.
    let after = Box::pin(worker.tick()).await.expect("after delivery");
    assert_eq!(after.sessions_resumed, 0);
    assert_eq!(after.failures, 1);
    let recovered = CommitCoordinator::recover(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator().session_id,
    )
    .await
    .expect("recover");
    assert!(
        recovered.state().pending_interaction().is_none(),
        "the delivered resolution was applied to the journal"
    );
    assert_eq!(
        store.load_batch(10).expect("inbox").len(),
        1,
        "an unparked resume keeps the delivered response for redelivery"
    );
    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1, "the row survives for a later tick");

    // THIRD ACT: prove the worker's own claim is genuine, not merely a
    // permanent-retry design. Once *something else* supplies the missing
    // stage decisions the worker cannot make on its own, the very same
    // worker resumes the run to completion, drains the inbox, and clears
    // the wake row — exactly the brief's original assertions.
    drive_past_missing_facade_decisions(&journal, &model, &tools, &catalog, &clock, 705).await;

    clock.jump(120_000).expect("past backoff");
    let completed = Box::pin(worker.tick()).await.expect("resume to completion");
    assert_eq!(
        completed.sessions_resumed, 1,
        "the resume is claimed and driven to completion"
    );
    assert!(
        store.load_batch(10).expect("inbox").is_empty(),
        "the consumed response is drained from the inbox"
    );
    assert!(
        store.load_tenant("tenant-a").expect("rows").is_empty(),
        "the wake row is deleted once the run reaches a terminal state"
    );
}
