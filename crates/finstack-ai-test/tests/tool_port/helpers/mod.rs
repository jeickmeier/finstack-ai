//! tool-port contract Toolset port, validation, scheduler, ordering, and panic proofs.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancellationPropagation,
    CancellationRequestTag, CommittedBatch, ContentBlock, Digest, EffectOutputContract,
    EffectOutputKind, Id, IdTag, KernelInput, LaneTag, Message, MessageRole, Metadata, OutputSpec,
    PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, ReducerStageOutcome, RetrySafety,
    RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation, RunSecurityContext,
    Sensitivity, SessionTag, Stage, StageCursor, StageSettled, TextBlock, Timestamp, ToolCallBlock,
    ToolCallTag, ToolExecutionMode, ToolFailurePolicy, ToolId, TransitionEnv, Usage,
    ValidationIssue, ValidationOutcome,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::events::EventHubConfig;
use finstack_ai_runtime::ids::{IdGenerationError, RandomSource};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::{
    JournalStore, JournalStoreDescriptor, LoadRequest, LoadedSession, SnapshotReceipt,
    SnapshotRequest, StoreError, StoreHealth,
};
use finstack_ai_runtime::ports::model::{
    ApprovalGrantMode, ApprovalMetadata, ApprovalRequirement, LockedModelContextProfile, Model,
    ModelContextProfile, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamItem, ModelStreamLimits, ModelToolCall, SideEffectClass, TokenEstimatorRef,
    TokenEstimatorSource, ToolCallDelta, ToolDeferralSupport, resolve_model_context_profile,
};
use finstack_ai_runtime::ports::tool::{
    JsonSchemaToolValidatorCompiler, ResolvedToolCatalog, ToolError, ToolEventStream,
    ToolExecutionPolicy, ToolPolicyDecision, ToolResult, ToolStreamItem, ToolStreamLimits,
    ToolValidator, ToolValidatorCompiler, Toolset, ToolsetRegistration,
};
use finstack_ai_runtime::run::{
    ModelTaskConfig, RunHandle, RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy,
    ToolTaskConfig,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    FixedClock, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction,
    ScriptedToolPlan, ScriptedToolset,
};
use futures_core::Stream;

mod resume;
pub(crate) use resume::*;

pub(crate) fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

pub(crate) fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

pub(crate) fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: finstack_ai_runtime::ports::model::ModelName::try_new("scripted-1").expect("model"),
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

pub(crate) fn locked_profile() -> LockedModelContextProfile {
    resolve_model_context_profile(profile(), None, None, false).expect("locked profile")
}

pub(crate) fn tool_spec(per_name: &str) -> finstack_ai_runtime::ports::model::ToolSpec {
    finstack_ai_runtime::ports::model::ToolSpec {
        id: ToolId::parse(format!("finstack.tools.{per_name}" )).expect("tool id"),
        model_name: Arc::from(per_name),
        title: Arc::from(per_name),
        description: Arc::from("scripted deterministic tool"),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"value":{"type":"integer"}},"required":["value"],"type":"object"}"#,
        )
        .expect("input schema"),
        output_schema: Some(
            RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"ok":{"type":"boolean"},"value":{"type":"integer"}},"required":["ok","value"],"type":"object"}"#,
            )
            .expect("output schema"),
        ),
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
    }
}

pub(crate) fn catalog(
    toolset: Arc<ScriptedToolset>,
    max_concurrency: usize,
) -> Arc<ResolvedToolCatalog> {
    catalog_with_failure_policy(toolset, max_concurrency, ToolFailurePolicy::ReturnToModel)
}

pub(crate) fn catalog_with_failure_policy(
    toolset: Arc<ScriptedToolset>,
    max_concurrency: usize,
    failure_policy: ToolFailurePolicy,
) -> Arc<ResolvedToolCatalog> {
    let toolset_port: Arc<dyn Toolset> = toolset;
    let policies = toolset_port
        .tools()
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                ToolExecutionPolicy {
                    failure_policy,
                    approval: ToolPolicyDecision::Allow,
                    max_concurrency,
                },
            )
        })
        .collect();
    Arc::new(
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
    )
}

pub(crate) fn model_response(call_count: usize, tool_name: &str) -> ModelResponse {
    ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: (0..call_count)
            .map(|index| ModelToolCall {
                name: Arc::from(tool_name),
                arguments: RawJson::parse(format!(r#"{{"value":{index}}}"#)).expect("arguments"),
                provider_call_id: None,
            })
            .collect::<Vec<_>>()
            .into(),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("completion-tools"),
        continuation_state: None,
    }
}

pub(crate) fn model_plan(call_count: usize, tool_name: &str) -> ScriptedModelPlan {
    let response = model_response(call_count, tool_name);
    let mut actions = response
        .tool_calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: u32::try_from(index).expect("index"),
                name: Some(Arc::clone(&call.name)),
                arguments_delta: Arc::from(call.arguments.as_str()),
                provider_call_id: None,
            })))
        })
        .collect::<Vec<_>>();
    actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
        response,
    ))));
    ScriptedModelPlan { actions }
}

pub(crate) fn completed_tool(value: i64) -> ScriptedToolPlan {
    ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(
            ToolResult {
                output: RawJson::parse(format!(r#"{{"ok":true,"value":{value}}}"#))
                    .expect("result"),
                is_error: false,
            },
        )))],
    }
}

pub(crate) fn failed_tool(code: &'static str) -> ScriptedToolPlan {
    ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Err(ToolError::try_new(
            code,
            finstack_ai_kernel::ErrorCategory::Tool,
            false,
            "scripted safe tool failure",
            Metadata::empty(),
        )
        .expect("tool failure")))],
    }
}

pub(crate) fn gated_tool(gate: &str, value: i64) -> ScriptedToolPlan {
    let mut plan = completed_tool(value);
    plan.actions
        .insert(0, ScriptedToolAction::Block(Arc::from(gate)));
    plan
}

pub(crate) fn tool_call(ordinal: u64, name: &str, arguments: &[u8]) -> ToolCallBlock {
    ToolCallBlock::try_new(
        id::<ToolCallTag>(ordinal),
        name,
        RawJson::parse(arguments).expect("arguments"),
    )
    .expect("tool call")
}

pub(crate) fn draft(
    messages: Arc<[Message]>,
    tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]>,
) -> ModelRequestDraft {
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

pub(crate) struct CounterRandom(pub(crate) AtomicU64);

impl RandomSource for CounterRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let value = self.0.fetch_add(1, Ordering::AcqRel).to_be_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = value[index % value.len()];
        }
        Ok(())
    }
}

pub(crate) struct FailNthAppendStore {
    inner: MemoryJournalStore,
    appends: AtomicUsize,
    fail_on: usize,
}

impl FailNthAppendStore {
    pub(crate) fn new(fail_on: usize) -> Self {
        Self {
            inner: MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 1,
                batches_per_session: 128,
                records_per_session: 512,
                snapshot_bytes: 1_024,
            })
            .expect("store"),
            appends: AtomicUsize::new(0),
            fail_on,
        }
    }
}

impl JournalStore for FailNthAppendStore {
    fn descriptor(&self) -> JournalStoreDescriptor {
        JournalStoreDescriptor::unspecified()
    }

    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        let call = self.appends.fetch_add(1, Ordering::AcqRel) + 1;
        if call == self.fail_on {
            return Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "tool_batch_append_failed",
                })
            });
        }
        self.inner.append(request)
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.inner.load(request)
    }

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        self.inner.write_snapshot(request)
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        self.inner.health()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn env(
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

pub(crate) fn cancellation_env(now: i64, ordinal: u64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id(ordinal)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(ordinal + 1)],
            vec![id::<CancellationRequestTag>(ordinal + 2)],
        )
        .expect("cancellation ids"),
    }
}

pub(crate) fn accepted() -> RunAccepted {
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
            deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(b"agent"),
        None,
    )
    .expect("accepted")
}

pub(crate) fn user_message() -> Message {
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

pub(crate) fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

pub(crate) async fn drive_to_after_model<S: JournalStore + 'static>(
    handle: &RunHandle,
    store: &Arc<S>,
    tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]>,
) {
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
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
    let request = draft(Arc::from([message]), tools);
    let raw = RawJson::parse(request.canonical_bytes().expect("canonical")).expect("raw");
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
    loop {
        let journal: Arc<dyn JournalStore> = store.clone();
        let recovered = CommitCoordinator::recover(journal, id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().phase == Some(RunPhase::AfterModel) {
            break;
        }
        tokio::task::yield_now().await;
    }
}

pub(crate) async fn drive_to_tools(
    handle: &RunHandle,
    store: &Arc<MemoryJournalStore>,
    tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]>,
) {
    drive_to_after_model(handle, store, tools).await;
    handle
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
}

pub(crate) async fn setup(
    call_count: usize,
    tool_plans: Vec<ScriptedToolPlan>,
    global: usize,
    per_tool: usize,
    execution: ToolExecutionMode,
) -> (
    RunTaskOwner,
    Arc<MemoryJournalStore>,
    Arc<ScriptedToolset>,
    RunHandle,
) {
    setup_with_failure_policy(
        call_count,
        tool_plans,
        global,
        per_tool,
        execution,
        ToolFailurePolicy::ReturnToModel,
    )
    .await
}

pub(crate) async fn setup_with_failure_policy(
    call_count: usize,
    tool_plans: Vec<ScriptedToolPlan>,
    global: usize,
    per_tool: usize,
    execution: ToolExecutionMode,
    failure_policy: ToolFailurePolicy,
) -> (
    RunTaskOwner,
    Arc<MemoryJournalStore>,
    Arc<ScriptedToolset>,
    RunHandle,
) {
    let mut spec = tool_spec("echo");
    spec.execution = execution;
    let tools: Arc<[finstack_ai_runtime::ports::model::ToolSpec]> = Arc::from([spec]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), tool_plans));
    let catalog = catalog_with_failure_policy(Arc::clone(&toolset), per_tool, failure_policy);
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![model_plan(call_count, "echo")],
    ));
    let model_port: Arc<dyn Model> = model;
    let model_port = Arc::new(
        finstack_ai_runtime::ports::model::ReadyModel::prepare(model_port)
            .await
            .expect("model readiness"),
    );
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 128,
            records_per_session: 512,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
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
            global_max_concurrency: global,
            stream_limits: ToolStreamLimits::default(),
        },
        model_port,
        locked_profile(),
        catalog,
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(1)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_tools(&handle, &store, tools).await;
    (owner, store, toolset, handle)
}

pub(crate) async fn wait_for_phase(store: &Arc<MemoryJournalStore>, phase: RunPhase) {
    loop {
        let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
            .await
            .expect("recover");
        if recovered.state().phase == Some(phase) {
            return;
        }
        tokio::task::yield_now().await;
    }
}

pub(crate) struct CountingCompiler {
    pub(crate) compiles: Arc<AtomicUsize>,
    pub(crate) validations: Arc<AtomicUsize>,
}

impl ToolValidatorCompiler for CountingCompiler {
    fn compile(
        &self,
        schema: &RawJson,
        resources: &BTreeMap<Arc<str>, RawJson>,
    ) -> Result<Arc<dyn ToolValidator>, ToolError> {
        self.compiles.fetch_add(1, Ordering::AcqRel);
        let inner = JsonSchemaToolValidatorCompiler.compile(schema, resources)?;
        Ok(Arc::new(CountingValidator {
            inner,
            validations: Arc::clone(&self.validations),
        }))
    }
}

pub(crate) struct CountingValidator {
    inner: Arc<dyn ToolValidator>,
    validations: Arc<AtomicUsize>,
}

impl ToolValidator for CountingValidator {
    fn validate(&self, instance: &RawJson) -> ValidationOutcome {
        self.validations.fetch_add(1, Ordering::AcqRel);
        self.inner.validate(instance)
    }
}

pub(crate) struct FixtureInputValidator;

impl ToolValidator for FixtureInputValidator {
    fn validate(&self, instance: &RawJson) -> ValidationOutcome {
        let value: serde_json::Value =
            serde_json::from_slice(instance.as_bytes()).expect("canonical JSON");
        match value.get("value") {
            Some(value) if value.is_i64() || value.is_u64() => ValidationOutcome::Valid,
            Some(_) => fixture_invalid("/value", "/properties/value/type", "type"),
            None => fixture_invalid("", "/required", "required"),
        }
    }
}

pub(crate) struct FixtureCompiler;

impl ToolValidatorCompiler for FixtureCompiler {
    fn compile(
        &self,
        _schema: &RawJson,
        _resources: &BTreeMap<Arc<str>, RawJson>,
    ) -> Result<Arc<dyn ToolValidator>, ToolError> {
        Ok(Arc::new(FixtureInputValidator))
    }
}

pub(crate) fn fixture_invalid(instance: &str, schema: &str, keyword: &str) -> ValidationOutcome {
    ValidationOutcome::try_invalid(
        Arc::from([ValidationIssue::try_new(
            Arc::<str>::from(instance),
            Arc::<str>::from(schema),
            Some(Arc::<str>::from(keyword)),
            Arc::<str>::from("value does not satisfy the JSON Schema constraint"),
        )
        .expect("issue")]),
        Arc::<str>::from(format!(
            "JSON Schema validation failed: {instance} ({keyword});"
        )),
    )
    .expect("outcome")
}

pub(crate) struct ItemStream(pub(crate) VecDeque<Result<ToolStreamItem, ToolError>>);

impl Stream for ItemStream {
    type Item = Result<ToolStreamItem, ToolError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.pop_front())
    }
}

pub(crate) fn stream(items: Vec<Result<ToolStreamItem, ToolError>>) -> ToolEventStream {
    Box::pin(ItemStream(items.into()))
}

pub(crate) async fn next_model_sequence_for_plan(
    plan: ScriptedToolPlan,
) -> (u64, Sensitivity, Vec<CommittedBatch>) {
    let (mut owner, store, toolset, handle) =
        setup(1, vec![plan], 1, 1, ToolExecutionMode::Parallel).await;
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    let after_tools = handle
        .submit(
            env(2_300, &[9_000], &[], &[], &[], &[], &[], 9_000),
            stage(Stage::AfterToolBatch, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after tools");
    assert!(after_tools.events.is_empty());
    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover before next model");
    assert_eq!(recovered.state().phase, Some(RunPhase::PreparingContext));
    let cycle = recovered.state().cycle;
    let context: Arc<[Message]> = Arc::from(recovered.state().messages.as_slice());
    handle
        .submit(
            env(2_350, &[9_001, 9_002], &[], &[], &[9_003], &[], &[], 9_001),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle,
                    stage: Stage::PrepareContext,
                },
                outcome: ReducerStageOutcome::ContextPrepared {
                    messages: Arc::clone(&context),
                },
            }),
        )
        .await
        .expect("prepare next context");
    let request = draft(context, toolset.tools());
    let request =
        RawJson::parse(request.canonical_bytes().expect("canonical request")).expect("raw request");
    let requested = handle
        .submit(
            env(
                2_400,
                &[9_004, 9_005],
                &[9_004],
                &[9_006],
                &[],
                &[9_007],
                &[],
                9_004,
            ),
            KernelInput::StageSettled(StageSettled {
                cursor: StageCursor {
                    cycle,
                    stage: Stage::BeforeModel,
                },
                outcome: ReducerStageOutcome::ModelRequestPrepared {
                    request,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(5_000)),
                },
            }),
        )
        .await
        .expect("next model request");
    let event = requested.events.last().expect("effect-requested event");
    let loaded = JournalStore::load(
        store.as_ref(),
        LoadRequest {
            session_id: id::<SessionTag>(1),
        },
    )
    .await
    .expect("load");
    owner.shutdown().await;
    (
        event.transient_sequence(),
        event.sensitivity(),
        loaded.committed_batches.as_ref().to_vec(),
    )
}
