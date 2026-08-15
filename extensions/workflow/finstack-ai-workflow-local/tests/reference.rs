//! PR-059 reference-driver proofs (A01, A02, A04, TM-19).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
    ComponentId, ContentBlock, DeadlinePropagation, Digest, Duration as KernelDuration, EffectId,
    EffectOutputContract, EffectOutputKind, ErrorCategory, ExternalEffectCompletion,
    ExternalEffectCompletionCommand, ExternalEffectOutcome, ExternalHandleRef, Id, IdTag,
    InteractionResolution, InteractionResolutionCommand, KernelInput, KernelState, Message,
    MessageRole, Metadata, OperationLocator, OutputSpec, PrincipalPropagation, PrincipalRef,
    ProviderIds, RawJson, ReconciliationPolicy, RecordBody, ReducerStageOutcome,
    RetryClassification, RetryDirective, RetrySafety, RunAccepted, RunLimits, RunPhase,
    RunPropagationPolicy, RunRelation, RunSecurityContext, Stage, StageCursor, TextBlock,
    Timestamp, TransitionEnv, Usage,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, Clock, CommitCoordinator, EventHubConfig, ExternalClock,
    IdGenerationError, JournalStore, JsonSchemaToolValidatorCompiler, LoadRequest,
    LockedModelContextProfile, ManualDriveAction, Model, ModelContextProfile, ModelDeferral,
    ModelError, ModelName, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamItem, ModelStreamLimits, ModelTaskConfig, ModelToolCall, RandomSource,
    ResolvedToolCatalog, RunHandle, RunTaskConfig, RunTaskOwner, SideEffectClass, TextDelta,
    TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, ToolExecutionPolicy, ToolFailurePolicy,
    ToolPolicyDecision, ToolResult, ToolSpec, ToolStreamItem, ToolStreamLimits, ToolTaskConfig,
    Toolset, ToolsetRegistration, WorkflowSession, WorkflowWait, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_local::LocalWorkflowDriver;

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("scripted-1").expect("model"),
        hard_input_bytes: 2_000_000,
        context_window_tokens: 3_000_000,
        max_output_tokens: 1_000,
        reserved_output_tokens: 1_000,
        provider_overhead_tokens: 0,
        estimator: TokenEstimatorRef {
            id: Arc::from("scripted-bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn locked_profile() -> LockedModelContextProfile {
    resolve_model_context_profile(profile(), None, None, false).expect("locked profile")
}

fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 128,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    )
}

struct CounterRandom(AtomicU64);

impl RandomSource for CounterRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        for chunk in buf.chunks_mut(8) {
            let value = self.0.fetch_add(1, Ordering::AcqRel).to_be_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
        Ok(())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors TransitionEnv allocation used by model_port proofs"
)]
fn env(
    now: i64,
    records: &[u64],
    events: &[u64],
    effects: &[u64],
    turns: &[u64],
    model_requests: &[u64],
    messages: &[u64],
    append_batch: u64,
) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            records.iter().copied().map(id).collect(),
            events.iter().copied().map(id).collect(),
            effects.iter().copied().map(id).collect(),
            Vec::new(),
            messages.iter().copied().map(id).collect(),
            turns.iter().copied().map(id).collect(),
            model_requests.iter().copied().map(id).collect(),
            Vec::new(),
            Vec::new(),
            vec![id(append_batch)],
            Vec::new(),
        )
        .expect("ids"),
    }
}

fn accepted() -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
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
        Digest::raw_json(b"agent"),
        None,
    )
    .expect("accepted")
}

fn user_message() -> Message {
    Message::try_new(
        id(44),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

fn draft(messages: Arc<[Message]>, tools: Arc<[ToolSpec]>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: profile().model,
        messages,
        tools,
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 2_000_000,
            max_input_tokens: 2_000_000,
            max_output_tokens: 1_000,
        },
    }
}

fn completed(text: &str) -> ModelResponse {
    ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new(text).expect("text"))]),
        tool_calls: Arc::from([]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::try_new(None::<&str>, Some("response-1"), None::<&str>)
            .expect("provider ids"),
        completion_id: Arc::from("completion-1"),
        continuation_state: None,
    }
}

fn retryable_failure() -> ModelError {
    ModelError::try_new(
        "temporary_model_failure",
        ErrorCategory::Model,
        true,
        "temporary model failure",
        Metadata::empty(),
    )
    .expect("retryable error")
}

fn completed_plan(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(text)))),
        ],
    }
}

fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
}

async fn spawn_model_owner(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    clock: impl Clock + Send + Sync + 'static,
    random: u64,
) -> RunTaskOwner {
    RunTaskOwner::spawn_with_model(
        coordinator,
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
        },
        model,
        locked_profile(),
        clock,
        CounterRandom(AtomicU64::new(random)),
    )
    .await
    .expect("owner")
}

async fn drive_to_active_model_request(handle: &RunHandle) {
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id(1),
                lane_id: id(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]), Arc::from([]))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
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
}

async fn drive_to_after_model(
    handle: &RunHandle,
    store: &Arc<MemoryJournalStore>,
    tools: Arc<[ToolSpec]>,
) {
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id(1),
                lane_id: id(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]), tools)
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
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
    wait_state(store, |state| state.phase == Some(RunPhase::AfterModel)).await;
}

async fn wait_state(
    store: &Arc<MemoryJournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let recovered = CommitCoordinator::recover(store.clone(), id(1))
                .await
                .expect("recover");
            if predicate(recovered.state()) {
                return recovered;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("state wait")
}

async fn journal_trace(store: &Arc<MemoryJournalStore>) -> Vec<(String, Option<EffectId>)> {
    let loaded = store
        .load(LoadRequest { session_id: id(1) })
        .await
        .expect("load");
    let mut rows = Vec::new();
    for batch in loaded.committed_batches.iter() {
        for envelope in batch.records.iter() {
            rows.push(match envelope.body() {
                RecordBody::EffectRequested(requested) => {
                    ("effect_requested".into(), Some(requested.effect_id()))
                }
                RecordBody::EffectCompleted(completed) => {
                    ("effect_completed".into(), Some(completed.effect_id()))
                }
                RecordBody::EffectDeferred(deferred) => {
                    ("effect_deferred".into(), Some(deferred.effect_id))
                }
                RecordBody::RetryScheduled(scheduled) => {
                    ("retry_scheduled".into(), Some(scheduled.timer_effect_id))
                }
                RecordBody::InteractionRequested(request) => {
                    ("interaction_requested".into(), Some(request.effect_id()))
                }
                RecordBody::RunAccepted(_) => ("run_accepted".into(), None),
                RecordBody::StageOutcomeRecorded(_) => ("stage_outcome".into(), None),
                RecordBody::ContextPrepared(_) => ("context_prepared".into(), None),
                other => (
                    format!("{other:?}")
                        .split('(')
                        .next()
                        .unwrap_or("other")
                        .into(),
                    None,
                ),
            });
        }
    }
    rows
}

async fn attach_driver(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    clock: ExternalClock,
    seed: u64,
) -> LocalWorkflowDriver {
    WorkflowSession::trusted(store, locator(), clock, seed)
        .await
        .expect("attach")
        .with_ports(model, locked_profile(), None)
}

#[tokio::test]
async fn adapter_matches_direct_owner_journal() {
    async fn run(through_adapter: bool) -> Vec<(String, Option<EffectId>)> {
        let store = memory_store();
        let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![completed_plan("hello")],
        ));
        let clock = ExternalClock::new(timestamp(2_000));
        let mut coordinator = CommitCoordinator::new(store.clone());
        let mut controller = coordinator.enable_manual_drive(1).expect("manual drive");
        let owner = spawn_model_owner(coordinator, Arc::clone(&model), clock.clone(), 500).await;
        let handle = owner.handle();
        let drive = tokio::spawn(async move { drive_to_active_model_request(&handle).await });
        let permit = tokio::time::timeout(StdDuration::from_secs(2), controller.next_effect())
            .await
            .expect("pause bound")
            .expect("paused execute");
        assert_eq!(permit.effect().action, ManualDriveAction::Execute);
        wait_state(&store, |state| state.pending_model_effect.is_some()).await;
        if through_adapter {
            drop(permit);
            drive.abort();
            let _ = drive.await;
            drop(owner);
            let mut driver = attach_driver(store.clone(), model, clock, 500).await;
            driver.ensure_owner().await.expect("spawn");
            wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
        } else {
            permit.continue_dispatch();
            drive.await.expect("drive");
            wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
            drop(owner);
        }
        journal_trace(&store).await
    }

    let direct = Box::pin(run(false)).await;
    let adapted = Box::pin(run(true)).await;
    let kinds = |rows: &[(String, Option<EffectId>)]| {
        rows.iter()
            .map(|(kind, effect)| (kind.clone(), *effect))
            .collect::<Vec<_>>()
    };
    assert_eq!(kinds(&direct), kinds(&adapted), "A01 journal parity");
    assert!(
        direct.iter().any(|(kind, _)| kind == "effect_requested"),
        "model effect present"
    );
}

#[tokio::test]
async fn conflicting_checkpoint_sequence_is_ignored() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        501,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
    drop(owner);
    let driver = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 501).await;
    let mut hint = driver.persist_handoff().expect("handoff");
    let journal_seq = hint.last_applied_seq;
    hint.last_applied_seq = journal_seq.saturating_add(99);
    assert_eq!(
        finstack_ai_runtime::resolve_checkpoint_sequence(journal_seq, Some(hint.last_applied_seq)),
        journal_seq
    );
    let _ = (store, clock, model);
}

#[tokio::test]
async fn timer_survives_worker_restart() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        800,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::BeforeFinalize)
    })
    .await;
    owner
        .handle()
        .submit(
            env(2_300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                ReducerStageOutcome::Retry(
                    RetryDirective::try_new(
                        RetryClassification::Model,
                        KernelDuration::from_millis(10),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    wait_state(&store, |state| state.phase == Some(RunPhase::Sleeping)).await;
    drop(owner);

    let mut driver = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 800).await;
    let WorkflowWait::Timer { effect_id, due_at } = driver.drive_until_wait().await.expect("timer")
    else {
        panic!("expected timer wait");
    };
    let first = (effect_id, due_at);
    driver.abort_owner();
    clock.jump(20).expect("jump");
    let mut resumed = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 801).await;
    let again = resumed.drive_until_wait().await.expect("resume");
    match again {
        WorkflowWait::Timer { effect_id, due_at } => {
            assert_eq!((effect_id, due_at), first, "same timer after restart");
        }
        WorkflowWait::Terminal { .. } => {}
        other => panic!("unexpected wait {other:?}"),
    }
    let second = attach_driver(store, model, clock, 802).await;
    let _ = second;
}

#[tokio::test]
async fn deferred_survives_worker_restart() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
                ModelDeferral {
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.model.scripted").expect("component"),
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
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        740,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);

    let mut driver = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 740).await;
    let WorkflowWait::DeferredEffect { effect_id, .. } =
        driver.drive_until_wait().await.expect("deferred")
    else {
        panic!("expected deferred");
    };
    driver.abort_owner();
    let mut resumed = attach_driver(store.clone(), Arc::clone(&model), clock.clone(), 741).await;
    let WorkflowWait::DeferredEffect {
        effect_id: again, ..
    } = resumed.drive_until_wait().await.expect("resume deferred")
    else {
        panic!("same deferred after restart");
    };
    assert_eq!(again, effect_id);
    let command = ExternalEffectCompletionCommand::try_new(
        locator(),
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            "ext-1",
            ExternalEffectOutcome::Completed {
                output: RawJson::parse(r#"{"ok":true}"#).expect("output"),
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("completion"),
    )
    .expect("command");
    resumed
        .complete_external(command, timestamp(3_000))
        .await
        .expect("complete");
    let _ = resumed.drive_until_wait().await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "A04 keeps catalog, park, abort, resume, and resolve contiguous"
)]
async fn interaction_survives_worker_restart() {
    let tools: Arc<[ToolSpec]> = Arc::from([ToolSpec {
        id: finstack_ai_kernel::ToolId::parse("finstack.tools.echo").expect("tool id"),
        model_name: Arc::from("echo"),
        title: Arc::from("echo"),
        description: Arc::from("echo"),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"value":{"type":"integer"}},"required":["value"],"type":"object"}"#,
        )
        .expect("input"),
        output_schema: None,
        execution: finstack_ai_kernel::ToolExecutionMode::Parallel,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
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
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    name: Some(Arc::from("echo")),
                    arguments_delta: Arc::from(arguments.as_str()),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([]),
                    tool_calls: Arc::from([ModelToolCall {
                        name: Arc::from("echo"),
                        arguments,
                    }]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::empty(),
                    completion_id: Arc::from("completion-tools"),
                    continuation_state: None,
                }))),
            ],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_500));
    let owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
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
        CounterRandom(AtomicU64::new(700)),
    )
    .await
    .expect("owner");
    drive_to_after_model(&owner.handle(), &store, Arc::clone(&tools)).await;
    owner
        .handle()
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    wait_state(&store, |state| {
        state.phase == Some(RunPhase::AwaitingInteraction)
    })
    .await;
    drop(owner);

    let mut driver = WorkflowSession::trusted(store.clone(), locator(), clock.clone(), 700)
        .await
        .expect("attach")
        .with_ports(Arc::clone(&model), locked_profile(), Some(catalog));
    let WorkflowWait::Interaction { interaction_id, .. } =
        driver.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected interaction");
    };
    driver.abort_owner();
    let mut resumed = WorkflowSession::trusted(store.clone(), locator(), clock.clone(), 701)
        .await
        .expect("resume")
        .with_ports(model, locked_profile(), None);
    let WorkflowWait::Interaction {
        interaction_id: again,
        ..
    } = resumed.drive_until_wait().await.expect("same interaction")
    else {
        panic!("same interaction after restart");
    };
    assert_eq!(again, interaction_id);
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
    resumed
        .resolve_interaction(command, timestamp(3_000))
        .await
        .expect("resolve");
}

#[tokio::test]
async fn cross_tenant_locator_is_unknown() {
    let store = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_plan("hello")],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(&model),
        clock.clone(),
        900,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    drop(owner);
    let driver = attach_driver(store, model, clock, 900).await;
    let foreign = OperationLocator::try_new("tenant-b", id(1), id(2), id(3)).expect("foreign");
    let command = ExternalEffectCompletionCommand::try_new(
        foreign,
        PrincipalRef::try_new("issuer", "subject", Some("tenant-b")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            id(103),
            "ext-x",
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "denied",
                    "denied",
                    finstack_ai_kernel::ErrorCategory::Validation,
                    false,
                )
                .expect("error"),
            },
        )
        .expect("completion"),
    )
    .expect("command");
    let error = driver
        .complete_external(command, timestamp(3_000))
        .await
        .expect_err("cross-tenant");
    assert_eq!(error.code(), "unknown_locator");
}
