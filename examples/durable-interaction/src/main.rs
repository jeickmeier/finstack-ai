//! Typed interaction that survives a simulated worker restart.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
    ContentBlock, DeadlinePropagation, Digest, EffectOutputContract, EffectOutputKind, Id, IdTag,
    InteractionResolution, InteractionResolutionCommand, KernelInput, KernelState, Message,
    MessageRole, Metadata, OperationLocator, OutputSpec, PrincipalPropagation, PrincipalRef,
    ProviderIds, RawJson, ReducerStageOutcome, RetrySafety, RunAccepted, RunLimits, RunPhase,
    RunPropagationPolicy, RunRelation, RunSecurityContext, Stage, StageCursor, TextBlock,
    Timestamp, TransitionEnv, Usage,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, CommitCoordinator, EventHubConfig, ExternalClock,
    IdGenerationError, JsonSchemaToolValidatorCompiler, LocalWorkflowDriver,
    LockedModelContextProfile, Model, ModelContextProfile, ModelName, ModelRequestDraft,
    ModelRequestLimits, ModelResponse, ModelSettings, ModelStreamItem, ModelStreamLimits,
    ModelTaskConfig, ModelToolCall, RandomSource, ResolvedToolCatalog, RunHandle, RunTaskConfig,
    RunTaskOwner, SideEffectClass, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta,
    ToolExecutionPolicy, ToolFailurePolicy, ToolPolicyDecision, ToolResult, ToolSpec,
    ToolStreamItem, ToolStreamLimits, ToolTaskConfig, Toolset, ToolsetRegistration,
    WorkflowSession, WorkflowWait, resolve_model_context_profile,
};
use finstack_ai_store_sqlite::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteJournalStore, SqliteStoreConfig,
    SqliteStoreLimits,
};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
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

fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
}

fn tools() -> Arc<[ToolSpec]> {
    Arc::from([ToolSpec {
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
    }])
}

fn catalog(tools: &Arc<[ToolSpec]>) -> Arc<ResolvedToolCatalog> {
    let toolset: Arc<dyn Toolset> = Arc::new(ScriptedToolset::new(
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
    Arc::new(
        ResolvedToolCatalog::try_new(
            [ToolsetRegistration {
                toolset,
                policies,
                components: BTreeMap::new(),
            }],
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .expect("catalog"),
    )
}

fn model() -> Arc<dyn Model> {
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    Arc::new(ScriptedModel::from_plans(
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
    ))
}

async fn spawn_owner(
    store: Arc<SqliteJournalStore>,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    clock: ExternalClock,
    random: u64,
) -> RunTaskOwner {
    RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(store),
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
        model,
        locked_profile(),
        catalog,
        clock,
        CounterRandom(AtomicU64::new(random)),
    )
    .await
    .expect("owner")
}

async fn drive_to_after_model(
    handle: &RunHandle,
    store: &Arc<SqliteJournalStore>,
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
    store: &Arc<SqliteJournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let recovered = CommitCoordinator::recover(Arc::clone(store) as _, id(1))
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

fn open_store(path: &std::path::Path) -> Arc<SqliteJournalStore> {
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: path.to_path_buf(),
            durability: SqliteDurability::Durable,
            limits: SqliteStoreLimits {
                sessions: 1,
                batches_per_session: 128,
                records_per_session: 512,
                snapshot_bytes: 64 * 1024,
            },
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("sqlite"),
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("journal.sqlite");
    let tools = tools();
    let catalog = catalog(&tools);
    let model = model();
    let clock = ExternalClock::new(timestamp(2_500));
    let store = open_store(&path);
    let owner = spawn_owner(
        Arc::clone(&store),
        Arc::clone(&model),
        Arc::clone(&catalog),
        clock.clone(),
        700,
    )
    .await;
    drive_to_after_model(&owner.handle(), &store, tools).await;
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
    drop(store);

    let store = open_store(&path);
    let mut driver: LocalWorkflowDriver =
        WorkflowSession::trusted(Arc::clone(&store) as _, locator(), clock.clone(), 701)
            .await
            .expect("resume")
            .with_ports(Arc::clone(&model), locked_profile(), Some(catalog));
    let WorkflowWait::Interaction { interaction_id, .. } =
        driver.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected interaction after worker restart");
    };
    driver
        .resolve_interaction(
            InteractionResolutionCommand::try_new(
                locator(),
                InteractionResolution::try_new(
                    interaction_id,
                    "resolution-1",
                    PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                        .expect("principal"),
                    AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
                    RawJson::parse(r#"{"approved":true}"#).expect("response"),
                    None::<&str>,
                )
                .expect("resolution"),
            )
            .expect("command"),
            timestamp(3_000),
        )
        .await
        .expect("resolve");
    println!("durable interaction {interaction_id} survived worker restart");
}
