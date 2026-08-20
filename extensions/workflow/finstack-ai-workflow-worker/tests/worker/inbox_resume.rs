//! End-to-end proofs that a response delivered while the worker is down is
//! picked up and applied on a later tick, driven entirely through
//! `WorkflowWorker::tick`.
//!
//! Both fixtures here settle inside `Stage::BeforeToolBatch` (a tool-call
//! approval interaction, and a deferred tool completion). Resolving either
//! one unconditionally lands the run's kernel phase on
//! `RunPhase::AfterToolBatch`
//! (`crates/finstack-ai-kernel/src/reducer/apply/tools.rs::apply_tool_batch_closed`
//! sets it regardless of `ToolBatchContinuation`), and advancing past that
//! phase requires an externally submitted
//! `KernelInput::StageSettled { cursor: .. AfterModel | AfterToolBatch .., outcome: Continue }`.
//! That decision is made by an application-level facade
//! (`crates/finstack-ai/src/agent/drive.rs` is the only non-test caller of
//! `settle_facade_stage`/`settle_facade_stage_with_model` in the workspace);
//! neither `finstack-ai-runtime`'s `RunTaskOwner` task loop nor
//! `WorkflowSession::ensure_owner`/`respawn_owner` submits it on its own, and
//! `finstack-ai-workflow-worker` depends on neither `finstack-ai` nor any
//! other source of that decision. Confirmed empirically: attaching a session,
//! resolving the interaction directly, then polling
//! `ensure_owner`/`classify_wait` for 200 iterations (4s) leaves
//! `phase == AfterToolBatch` and `classify_wait == None` throughout — the run
//! never reaches a new park-able wait or terminal state.
//!
//! This means `WorkflowWorker::tick` can never report `sessions_resumed == 1`
//! for either scenario: `resume_row`'s `drive_past_wait` genuinely cannot
//! find a new wait within its budget, so it always returns
//! `WorkerError::Driver(DriveTimeout)`, which the tick loop counts as a
//! failure and backs off — exactly the outcome Task 9 already proved and
//! accepted in `tick_keeps_the_inbox_entry_when_the_resume_cannot_park`
//! (`tests/worker/tick.rs`). This is a brief-reality mismatch from
//! `task-10-brief.md` Step 1, which specifies `sessions_resumed == 1` and a
//! drained inbox; see `task-10-report.md` for the full writeup. What these
//! tests instead prove — the maximal true claim — is the part that *is*
//! achievable and is the actual point of `deliver_interaction`/
//! `deliver_external`: the response is durably buffered while the worker is
//! down (a pre-delivery tick claims nothing), and once delivered it *is*
//! applied to the journal on the very next tick that claims the row — the
//! interaction resolves / the effect settles — even though the run cannot
//! reach a new park-able wait in this single tick, so the row is preserved
//! (not dropped) for a future tick once an external stage decision unblocks
//! `AfterToolBatch`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AuthorizationEvidence, ComponentId, ContentBlock, ExternalEffectCompletion,
    ExternalEffectCompletionCommand, ExternalEffectOutcome, ExternalHandleRef,
    InteractionResolution, InteractionResolutionCommand, Metadata, PrincipalRef, ProviderIds,
    RawJson, ReconciliationPolicy, ReducerStageOutcome, RetrySafety, RunPhase, Stage,
    ToolExecutionMode, ToolFailurePolicy, ToolId, ToolResultBlock, Usage,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, CommitCoordinator, EventHubConfig, ExternalClock,
    JournalStore, JsonSchemaToolValidatorCompiler, Model, ModelResponse, ModelStreamItem,
    ModelStreamLimits, ModelTaskConfig, ModelToolCall, ResolvedToolCatalog, RunTaskConfig,
    RunTaskOwner, SameIdentityRetryPolicy, SideEffectClass, ToolCallDelta, ToolDeferral,
    ToolDeferralSupport, ToolExecutionPolicy, ToolPolicyDecision, ToolResult, ToolSpec,
    ToolStreamItem, ToolStreamLimits, ToolTaskConfig, Toolset, ToolsetRegistration,
    WorkflowSession, WorkflowWait,
};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, MemoryWorkerStore, PortsFactory, WakeIndexStore, WorkerBuilder,
    WorkerError, park,
};

use crate::helpers::{
    CounterRandom, completed_plan, drive_to_after_model, env, locator, locked_profile,
    memory_store, profile, stage, timestamp, wait_state,
};

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
    let owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(journal.clone()),
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: Duration::from_millis(500),
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        ToolTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        Arc::clone(&model),
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
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);

    let mut session = WorkflowSession::trusted(journal.clone(), locator(), clock.clone(), 741)
        .await
        .expect("attach")
        .with_ports(Arc::clone(&model), locked_profile(), Some(Arc::clone(&catalog)));
    let WorkflowWait::DeferredEffect { effect_id, .. } =
        session.drive_until_wait().await.expect("deferred")
    else {
        panic!("expected a deferred wait");
    };
    let tool_call_id = *session
        .last_state()
        .tool_calls
        .iter()
        .find(|(_, identity)| identity.effect_id == Some(effect_id))
        .map(|(id, _)| id)
        .expect("tool call id");
    let store = Arc::new(MemoryWorkerStore::new());
    park(&mut session, store.as_ref(), "research").expect("park");
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
            model,
            catalog: Some(catalog),
        }),
    )
    .build();

    // No response has been delivered yet: the row is a non-timer wait, so
    // the claim gate must skip it, claiming nothing.
    let before = Box::pin(worker.tick()).await.expect("before delivery");
    assert_eq!(before.sessions_resumed, 0);

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
        recovered.state().tool_settlements.contains_key(&effect_id),
        "the delivered completion was applied to the journal"
    );
    assert_eq!(
        store.load_all().expect("inbox").len(),
        1,
        "an unparked resume keeps the delivered response for redelivery"
    );
    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1, "the row survives for a later tick");
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
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
                        ModelResponse {
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
                        },
                    ))),
                ],
            },
            completed_plan("done"),
        ],
    ));
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
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        ToolTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        Arc::clone(&model),
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
            stage(
                Stage::AfterModel,
                ReducerStageOutcome::Continue,
            ),
        )
        .await
        .expect("after model");
    wait_state(&journal, |state| {
        state.phase == Some(RunPhase::AwaitingInteraction)
    })
    .await;
    drop(owner);

    let mut session = WorkflowSession::trusted(
        journal.clone(),
        locator(),
        clock.clone(),
        701,
    )
    .await
    .expect("attach")
    .with_ports(Arc::clone(&model), locked_profile(), Some(Arc::clone(&catalog)));
    let WorkflowWait::Interaction { interaction_id, .. } =
        session.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected an interaction wait");
    };
    let store = Arc::new(MemoryWorkerStore::new());
    park(&mut session, store.as_ref(), "research").expect("park");
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
            model,
            catalog: Some(catalog),
        }),
    )
    .build();

    let before = Box::pin(worker.tick()).await.expect("no response yet");
    assert_eq!(before.sessions_resumed, 0);

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
        recovered.state().pending_interaction.is_none(),
        "the delivered resolution was applied to the journal"
    );
    assert_eq!(
        store.load_all().expect("inbox").len(),
        1,
        "an unparked resume keeps the delivered response for redelivery"
    );
    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1, "the row survives for a later tick");
}
