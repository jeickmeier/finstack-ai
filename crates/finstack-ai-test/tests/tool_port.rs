//! PR-016 Toolset port, validation, scheduler, ordering, and panic proofs.

use std::collections::{BTreeMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancelRequested,
    CancellationInitiator, CancellationPropagation, CancellationRequestTag, CommittedBatch,
    ContentBlock, Digest, EffectOutputContract, EffectOutputKind, Id, IdTag, KernelInput, LaneTag,
    Message, MessageRole, Metadata, OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds,
    RawJson, RecordBody, ReducerStageOutcome, RetrySafety, RunAccepted, RunLimits, RunPhase,
    RunPropagationPolicy, RunRelation, RunSecurityContext, Sensitivity, SessionTag, Stage,
    StageCursor, StageSettled, TextBlock, Timestamp, ToolCallBlock, ToolCallId, ToolCallPlan,
    ToolCallTag, ToolExecutionMode, ToolFailurePolicy, ToolId, ToolProgress, ToolResultBlock,
    TransitionEnv, Usage, ValidationIssue, ValidationOutcome,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, CommitCoordinator, CommitCoordinatorError,
    IdGenerationError, JournalStore, JsonSchemaToolValidatorCompiler, LoadRequest, LoadedSession,
    LockedModelContextProfile, Model, ModelContextProfile, ModelRequestDraft, ModelRequestLimits,
    ModelResponse, ModelSettings, ModelStreamItem, ModelStreamLimits, ModelTaskConfig,
    ModelToolCall, PortFuture, RandomSource, ResolvedToolCatalog, RunHandle, RunHandleError,
    RunStatus, RunTaskConfig, RunTaskOwner, SideEffectClass, SnapshotReceipt, SnapshotRequest,
    StoreError, StoreHealth, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, ToolError,
    ToolEventStream, ToolExecutionPolicy, ToolPolicyDecision, ToolResult, ToolStreamAssembler,
    ToolStreamItem, ToolStreamLimits, ToolTaskConfig, ToolValidator, ToolValidatorCompiler,
    Toolset, ToolsetRegistration, UsageDelta, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    FixedClock, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction,
    ScriptedToolPlan, ScriptedToolset,
};
use futures_core::Stream;

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
        model: finstack_ai_runtime::ModelName::try_new("scripted-1").expect("model"),
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

fn tool_spec(per_name: &str) -> finstack_ai_runtime::ToolSpec {
    finstack_ai_runtime::ToolSpec {
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
    }
}

fn catalog(toolset: Arc<ScriptedToolset>, max_concurrency: usize) -> Arc<ResolvedToolCatalog> {
    catalog_with_failure_policy(toolset, max_concurrency, ToolFailurePolicy::ReturnToModel)
}

fn catalog_with_failure_policy(
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

fn model_response(call_count: usize, tool_name: &str) -> ModelResponse {
    ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: (0..call_count)
            .map(|index| ModelToolCall {
                name: Arc::from(tool_name),
                arguments: RawJson::parse(format!(r#"{{"value":{index}}}"#)).expect("arguments"),
            })
            .collect::<Vec<_>>()
            .into(),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("completion-tools"),
        continuation_state: None,
    }
}

fn model_plan(call_count: usize, tool_name: &str) -> ScriptedModelPlan {
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
            })))
        })
        .collect::<Vec<_>>();
    actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
        response,
    ))));
    ScriptedModelPlan { actions }
}

fn completed_tool(value: i64) -> ScriptedToolPlan {
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

fn failed_tool(code: &'static str) -> ScriptedToolPlan {
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

fn gated_tool(gate: &str, value: i64) -> ScriptedToolPlan {
    let mut plan = completed_tool(value);
    plan.actions
        .insert(0, ScriptedToolAction::Block(Arc::from(gate)));
    plan
}

fn tool_call(ordinal: u64, name: &str, arguments: &[u8]) -> ToolCallBlock {
    ToolCallBlock::try_new(
        id::<ToolCallTag>(ordinal),
        name,
        RawJson::parse(arguments).expect("arguments"),
    )
    .expect("tool call")
}

fn draft(
    messages: Arc<[Message]>,
    tools: Arc<[finstack_ai_runtime::ToolSpec]>,
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

struct CounterRandom(AtomicU64);

impl RandomSource for CounterRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let value = self.0.fetch_add(1, Ordering::AcqRel).to_be_bytes();
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = value[index % value.len()];
        }
        Ok(())
    }
}

struct FailNthAppendStore {
    inner: MemoryJournalStore,
    appends: AtomicUsize,
    fail_on: usize,
}

impl FailNthAppendStore {
    fn new(fail_on: usize) -> Self {
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

fn cancellation_env(now: i64, ordinal: u64) -> TransitionEnv {
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
            deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
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

async fn drive_to_after_model<S: JournalStore + 'static>(
    handle: &RunHandle,
    store: &Arc<S>,
    tools: Arc<[finstack_ai_runtime::ToolSpec]>,
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

async fn drive_to_tools(
    handle: &RunHandle,
    store: &Arc<MemoryJournalStore>,
    tools: Arc<[finstack_ai_runtime::ToolSpec]>,
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

async fn setup(
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

async fn setup_with_failure_policy(
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
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> = Arc::from([spec]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), tool_plans));
    let catalog = catalog_with_failure_policy(Arc::clone(&toolset), per_tool, failure_policy);
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![model_plan(call_count, "echo")],
    ));
    let model_port: Arc<dyn Model> = model;
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

async fn wait_for_phase(store: &Arc<MemoryJournalStore>, phase: RunPhase) {
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

struct CountingCompiler {
    compiles: Arc<AtomicUsize>,
    validations: Arc<AtomicUsize>,
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

struct CountingValidator {
    inner: Arc<dyn ToolValidator>,
    validations: Arc<AtomicUsize>,
}

impl ToolValidator for CountingValidator {
    fn validate(&self, instance: &RawJson) -> ValidationOutcome {
        self.validations.fetch_add(1, Ordering::AcqRel);
        self.inner.validate(instance)
    }
}

struct FixtureInputValidator;

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

struct FixtureCompiler;

impl ToolValidatorCompiler for FixtureCompiler {
    fn compile(
        &self,
        _schema: &RawJson,
        _resources: &BTreeMap<Arc<str>, RawJson>,
    ) -> Result<Arc<dyn ToolValidator>, ToolError> {
        Ok(Arc::new(FixtureInputValidator))
    }
}

fn fixture_invalid(instance: &str, schema: &str, keyword: &str) -> ValidationOutcome {
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

struct ItemStream(VecDeque<Result<ToolStreamItem, ToolError>>);

impl Stream for ItemStream {
    type Item = Result<ToolStreamItem, ToolError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.pop_front())
    }
}

fn stream(items: Vec<Result<ToolStreamItem, ToolError>>) -> ToolEventStream {
    Box::pin(ItemStream(items.into()))
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one acceptance fixture keeps compile-once counters and policy precedence on the same catalog"
)]
fn catalog_compiles_once_validates_at_one_boundary_and_enforces_approval_floor() {
    let mut required = tool_spec("catalog-required");
    required.approval.requirement = ApprovalRequirement::Required;
    required.approval.attributes =
        Metadata::parse(br#"{"approved":true,"role":"admin"}"#).expect("hostile metadata");
    let mut host_guard = tool_spec("catalog-host-guard");
    host_guard.approval.attributes =
        Metadata::parse(br#"{"approved":true}"#).expect("hostile metadata");
    let deadline_tool = tool_spec("catalog-deadline");
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> =
        Arc::from([required.clone(), host_guard.clone(), deadline_tool.clone()]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let toolset_port: Arc<dyn Toolset> = toolset;
    let compilation_counter = Arc::new(AtomicUsize::new(0));
    let validations = Arc::new(AtomicUsize::new(0));
    let adapter = CountingCompiler {
        compiles: Arc::clone(&compilation_counter),
        validations: Arc::clone(&validations),
    };
    let catalog = ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset: toolset_port,
            policies: BTreeMap::from([
                (
                    required.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::Allow,
                        max_concurrency: 1,
                    },
                ),
                (
                    host_guard.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::RequireApproval,
                        max_concurrency: 1,
                    },
                ),
                (
                    deadline_tool.id.clone(),
                    ToolExecutionPolicy {
                        failure_policy: ToolFailurePolicy::ReturnToModel,
                        approval: ToolPolicyDecision::Allow,
                        max_concurrency: 1,
                    },
                ),
            ]),
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &adapter,
    )
    .expect("catalog");
    assert_eq!(compilation_counter.load(Ordering::Acquire), 6);

    let planned = catalog.plan_call(
        tool_call(77, "catalog-required", br#"{"value":1}"#),
        None,
        None,
    );
    assert_eq!(validations.load(Ordering::Acquire), 1);
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("required approval must remain undispatched");
    };
    assert_eq!(closure.error.code.as_str(), "tool_approval_required");

    let planned = catalog.plan_call(
        tool_call(78, "catalog-host-guard", br#"{"value":1}"#),
        None,
        None,
    );
    assert_eq!(validations.load(Ordering::Acquire), 2);
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("host approval policy must override non-authoritative metadata");
    };
    assert_eq!(closure.error.code.as_str(), "tool_approval_required");

    let planned = catalog.plan_call(
        tool_call(79, "catalog-required", br#"{"value":1}"#),
        None,
        Some(ToolPolicyDecision::Deny),
    );
    assert_eq!(validations.load(Ordering::Acquire), 3);
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("stricter middleware denial must remain undispatched");
    };
    assert_eq!(closure.error.code.as_str(), "tool_policy_denied");

    let planned = catalog.plan_call(
        tool_call(80, "catalog-required", br#"{"value":"wrong"}"#),
        None,
        None,
    );
    assert_eq!(validations.load(Ordering::Acquire), 4);
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("invalid arguments must close synthetically");
    };
    assert_eq!(closure.error.code.as_str(), "tool_arguments_invalid");

    let planned = catalog.plan_call(
        tool_call(81, "catalog-unknown", br#"{"value":1}"#),
        None,
        None,
    );
    let ToolCallPlan::SyntheticClosure(closure) = planned else {
        panic!("unknown tools must close synthetically");
    };
    assert_eq!(closure.error.code.as_str(), "unknown_tool");
    assert_eq!(validations.load(Ordering::Acquire), 4);

    let planned = catalog.plan_call(
        tool_call(82, "catalog-deadline", br#"{"value":1}"#),
        Some(timestamp(9_000)),
        None,
    );
    let ToolCallPlan::Execute(call) = planned else {
        panic!("allowed call must remain executable");
    };
    assert_eq!(call.deadline, Some(timestamp(9_000)));
    assert_eq!(validations.load(Ordering::Acquire), 5);
    assert_eq!(compilation_counter.load(Ordering::Acquire), 6);
}

#[test]
fn default_validator_matches_independent_portable_fixture_outcomes_and_offline_refs() {
    let compiler = JsonSchemaToolValidatorCompiler;
    let spec = tool_spec("parity");
    let schema = spec.input_schema.clone();
    let validator = compiler
        .compile(&schema, &BTreeMap::new())
        .expect("validator");
    let fixture = FixtureInputValidator;
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> = Arc::from([spec.clone()]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let policy = BTreeMap::from([(
        spec.id.clone(),
        ToolExecutionPolicy {
            failure_policy: ToolFailurePolicy::ReturnToModel,
            approval: ToolPolicyDecision::Allow,
            max_concurrency: 1,
        },
    )]);
    let default_catalog = ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset: toolset.clone(),
            policies: policy.clone(),
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &compiler,
    )
    .expect("default catalog");
    let fixture_catalog = ResolvedToolCatalog::try_new(
        [ToolsetRegistration {
            toolset,
            policies: policy,
            components: BTreeMap::new(),
        }],
        &BTreeMap::new(),
        &FixtureCompiler,
    )
    .expect("fixture catalog");
    for (ordinal, candidate) in [br#"{"value":1}"#.as_slice(), br#"{"value":"x"}"#, b"{}"]
        .into_iter()
        .enumerate()
    {
        let candidate = RawJson::parse(candidate).expect("candidate");
        assert_eq!(validator.validate(&candidate), fixture.validate(&candidate));
        let ordinal = u64::try_from(ordinal).expect("ordinal");
        assert_eq!(
            default_catalog.plan_call(
                tool_call(900 + ordinal, "parity", candidate.as_bytes()),
                None,
                None,
            ),
            fixture_catalog.plan_call(
                tool_call(900 + ordinal, "parity", candidate.as_bytes()),
                None,
                None,
            ),
        );
    }

    let referencing = RawJson::parse(br#"{"$ref":"urn:finstack:positive"}"#).expect("ref schema");
    assert!(compiler.compile(&referencing, &BTreeMap::new()).is_err());
    let resources = BTreeMap::from([(
        Arc::<str>::from("urn:finstack:positive"),
        RawJson::parse(br#"{"minimum":0,"type":"integer"}"#).expect("resource"),
    )]);
    let resolved = compiler
        .compile(&referencing, &resources)
        .expect("offline ref");
    assert_eq!(
        resolved.validate(&RawJson::parse(b"1").expect("valid")),
        ValidationOutcome::Valid
    );
    assert!(matches!(
        resolved.validate(&RawJson::parse(b"-1").expect("invalid")),
        ValidationOutcome::Invalid { .. }
    ));
}

#[tokio::test]
async fn stream_normalization_rejects_invalid_output_duplicates_and_all_oversized_results() {
    let spec = tool_spec("stream");
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> = Arc::from([spec]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let resolved = catalog(toolset, 1)
        .by_name("stream")
        .expect("resolved")
        .clone();
    let invalid = ToolResult {
        output: RawJson::parse(br#"{"ok":"wrong","value":1}"#).expect("output"),
        is_error: false,
    };
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Ok(ToolStreamItem::Completed(invalid))]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect_err("invalid output");
    assert_eq!(error.code(), "tool_output_invalid");

    let valid = ToolResult {
        output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("output"),
        is_error: false,
    };
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![
                Ok(ToolStreamItem::Completed(valid.clone())),
                Ok(ToolStreamItem::Completed(valid)),
            ]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect_err("duplicate completion");
    assert_eq!(error.code(), "tool_stream_invalid");

    let oversized_error = ToolResult {
        output: RawJson::parse(br#"{"error":"application"}"#).expect("output"),
        is_error: true,
    };
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Ok(ToolStreamItem::Completed(oversized_error))]),
            resolved.output_validator.as_deref(),
            4,
        )
        .await
        .expect_err("error result is also bounded");
    assert_eq!(error.code(), "tool_result_limit_exceeded");

    let error = ToolStreamAssembler::default()
        .assemble(
            stream(Vec::new()),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect_err("completion is required");
    assert_eq!(error.code(), "tool_stream_invalid");
}

#[tokio::test]
async fn progress_usage_and_stream_limits_are_normalized_before_settlement() {
    let first =
        Usage::try_new(Some(1), Some(0), Some(1), None, BTreeMap::new()).expect("first usage");
    let second =
        Usage::try_new(Some(1), Some(1), Some(2), None, BTreeMap::new()).expect("second usage");
    let completed = ToolResult {
        output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("output"),
        is_error: false,
    };
    let assembled = ToolStreamAssembler::default()
        .assemble(
            stream(vec![
                Ok(ToolStreamItem::Progress(
                    ToolProgress::try_new("halfway", Some(50)).expect("progress"),
                )),
                Ok(ToolStreamItem::Usage(UsageDelta {
                    usage: first.clone(),
                })),
                Ok(ToolStreamItem::Usage(UsageDelta {
                    usage: second.clone(),
                })),
                Ok(ToolStreamItem::Completed(completed.clone())),
            ]),
            None,
            4_096,
        )
        .await
        .expect("normalized stream");
    assert_eq!(assembled.progress.len(), 1);
    assert_eq!(assembled.usage, Some(second.clone()));

    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![
                Ok(ToolStreamItem::Usage(UsageDelta { usage: second })),
                Ok(ToolStreamItem::Usage(UsageDelta { usage: first })),
                Ok(ToolStreamItem::Completed(completed.clone())),
            ]),
            None,
            4_096,
        )
        .await
        .expect_err("usage must not regress");
    assert_eq!(error.code(), "tool_stream_invalid");

    let error = ToolStreamAssembler::new(ToolStreamLimits {
        max_items: 1,
        max_stream_bytes: 1,
    })
    .assemble(
        stream(vec![
            Ok(ToolStreamItem::Progress(
                ToolProgress::try_new("too large", None).expect("progress"),
            )),
            Ok(ToolStreamItem::Completed(completed)),
        ]),
        None,
        4_096,
    )
    .await
    .expect_err("stream limits must apply");
    assert_eq!(error.code(), "tool_stream_limit_exceeded");
}

async fn next_model_sequence_for_plan(
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

#[tokio::test]
async fn tool_progress_advances_the_transient_sequence_without_entering_the_journal() {
    let completed = ToolResult {
        output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("output"),
        is_error: false,
    };
    let with_progress = ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![
            ScriptedToolAction::Emit(Ok(ToolStreamItem::Progress(
                ToolProgress::try_new("halfway", Some(50)).expect("progress"),
            ))),
            ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(completed))),
        ],
    };
    let (baseline, _, _) = Box::pin(next_model_sequence_for_plan(completed_tool(1))).await;
    let (with_progress, sensitivity, journal) =
        Box::pin(next_model_sequence_for_plan(with_progress)).await;
    assert_eq!(with_progress, baseline + 1);
    assert_eq!(sensitivity, Sensitivity::Confidential);
    let journal = serde_json::to_string(&journal).expect("journal JSON");
    assert!(!journal.contains("tool_progress"));
    assert!(!journal.contains("halfway"));
}

#[tokio::test]
async fn tool_reported_errors_remain_bounded_and_stably_classified() {
    let spec = tool_spec("reported-error");
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> = Arc::from([spec]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let resolved = catalog(toolset, 1)
        .by_name("reported-error")
        .expect("resolved")
        .clone();
    let reported = ToolError::try_new(
        "fixture_tool_reported",
        finstack_ai_kernel::ErrorCategory::Tool,
        true,
        "safe tool-reported failure",
        Metadata::empty(),
    )
    .expect("reported error");
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Err(reported)]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect_err("tool-reported stream error");
    assert_eq!(error.code(), "fixture_tool_reported");
    assert!(error.retryable());

    let application_error = ToolResult {
        output: RawJson::parse(br#"{"error":"application"}"#).expect("output"),
        is_error: true,
    };
    let assembled = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Ok(ToolStreamItem::Completed(application_error))]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect("bounded application errors bypass success schema validation");
    assert!(assembled.result.is_error);

    let deadline = ToolError::try_new(
        finstack_ai_runtime::TOOL_DEADLINE_EXCEEDED,
        finstack_ai_kernel::ErrorCategory::Deadline,
        false,
        "tool result arrived after its committed deadline",
        Metadata::empty(),
    )
    .expect("stable deadline classification");
    assert_eq!(deadline.code(), "tool_deadline_exceeded");
    assert_eq!(
        deadline
            .to_descriptor()
            .expect("deadline descriptor")
            .category,
        finstack_ai_kernel::ErrorCategory::Deadline
    );
}

#[tokio::test]
async fn failed_tool_batch_append_never_executes_a_tool() {
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> = Arc::from([tool_spec("echo")]);
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(&tools),
        vec![completed_tool(0)],
    ));
    let catalog = catalog(Arc::clone(&toolset), 1);
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![model_plan(1, "echo")],
    ));
    let store = Arc::new(FailNthAppendStore::new(7));
    let mut owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(store.clone()),
        RunTaskConfig {
            command_capacity: 8,
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
            job_capacity: 2,
            result_capacity: 2,
            global_max_concurrency: 1,
            stream_limits: ToolStreamLimits::default(),
        },
        model,
        locked_profile(),
        catalog,
        FixedClock::new(timestamp(2_000)),
        CounterRandom(AtomicU64::new(500)),
    )
    .await
    .expect("owner");
    let handle = owner.handle();
    drive_to_after_model(&handle, &store, tools).await;
    let error = handle
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect_err("tool batch append must fail");
    assert!(matches!(
        error,
        RunHandleError::Coordinator(CommitCoordinatorError::Store(StoreError::Unavailable {
            reason_code: "tool_batch_append_failed"
        }))
    ));
    assert_eq!(toolset.call_count(), 0);
    let loaded = JournalStore::load(
        store.as_ref(),
        LoadRequest {
            session_id: id::<SessionTag>(1),
        },
    )
    .await
    .expect("load");
    assert!(!loaded.committed_batches.iter().any(|batch| {
        batch.records.iter().any(|record| {
            matches!(
                record.body(),
                RecordBody::EffectRequested(requested)
                    if requested.kind() == finstack_ai_kernel::EffectKind::Tool
            )
        })
    }));
    assert_eq!(handle.status(), RunStatus::Running);
    owner.shutdown().await;
}

#[tokio::test]
async fn global_and_per_tool_limits_execute_an_admitted_parallel_group_in_waves() {
    let plans = (0..4).map(|value| gated_tool("wave", value)).collect();
    let (mut owner, store, toolset, handle) =
        setup(4, plans, 2, 2, ToolExecutionMode::Parallel).await;
    while toolset.control().entries("wave") < 2 {
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.control().entries("wave"), 2);
    assert_eq!(toolset.max_active_call_count(), 2);
    toolset.control().release("wave");
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    assert_eq!(toolset.call_count(), 4);
    assert_eq!(toolset.max_active_call_count(), 2);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Running);
    owner.shutdown().await;
}

#[tokio::test]
async fn cancellation_cleans_running_and_queued_calls_without_starting_the_queue() {
    let plans = vec![
        gated_tool("cancel-running", 0),
        completed_tool(1),
        completed_tool(2),
    ];
    let (mut owner, _, toolset, handle) = setup(3, plans, 1, 3, ToolExecutionMode::Parallel).await;
    while toolset.control().entries("cancel-running") == 0 {
        tokio::task::yield_now().await;
    }
    handle
        .submit(
            cancellation_env(2_200, 800),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("tool cancellation fixture")),
            }),
        )
        .await
        .expect("cancel run");
    while toolset.active_call_count() != 0 {
        tokio::task::yield_now().await;
    }
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.active_call_count(), 0);
    owner.shutdown().await;
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn shutdown_cancels_streaming_tool_children_without_leaks() {
    let (mut owner, _, toolset, handle) = setup(
        1,
        vec![gated_tool("shutdown-stream", 0)],
        1,
        1,
        ToolExecutionMode::Parallel,
    )
    .await;
    while toolset.control().entries("shutdown-stream") == 0 {
        tokio::task::yield_now().await;
    }
    owner.shutdown().await;
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Stopped);
}

#[tokio::test]
async fn dropping_the_owner_aborts_inner_tool_children_without_detaching_them() {
    let (owner, _, toolset, handle) = setup(
        1,
        vec![gated_tool("drop-stream", 0)],
        1,
        1,
        ToolExecutionMode::Parallel,
    )
    .await;
    while toolset.control().entries("drop-stream") == 0 {
        tokio::task::yield_now().await;
    }
    drop(owner);
    for _ in 0..64 {
        if toolset.active_call_count() == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Stopped);
}

async fn assert_kernel_exclusive_mode(execution: ToolExecutionMode) {
    let plans = (0..3)
        .map(|value| gated_tool(&format!("exclusive-{value}"), value))
        .collect();
    let (mut owner, store, toolset, _) = setup(3, plans, 3, 3, execution).await;
    while toolset.control().entries("exclusive-0") == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.control().entries("exclusive-1"), 0);
    assert_eq!(toolset.max_active_call_count(), 1);
    toolset.control().release("exclusive-0");
    while toolset.control().entries("exclusive-1") == 0 {
        tokio::task::yield_now().await;
    }
    assert_eq!(toolset.control().entries("exclusive-2"), 0);
    toolset.control().release("exclusive-1");
    while toolset.control().entries("exclusive-2") == 0 {
        tokio::task::yield_now().await;
    }
    toolset.control().release("exclusive-2");
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    assert_eq!(toolset.max_active_call_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn sequential_and_barrier_modes_remain_exclusive_under_larger_executor_limits() {
    assert_kernel_exclusive_mode(ToolExecutionMode::Sequential).await;
    assert_kernel_exclusive_mode(ToolExecutionMode::Barrier).await;
}

#[tokio::test]
async fn fail_run_closes_undispatched_calls_and_matches_exact_kernel_id_requirements() {
    let (mut owner, store, toolset, handle) = setup_with_failure_policy(
        3,
        vec![failed_tool("fixture_fail_run")],
        3,
        3,
        ToolExecutionMode::Sequential,
        ToolFailurePolicy::FailRun,
    )
    .await;
    for _ in 0..128 {
        tokio::task::yield_now().await;
    }
    assert_eq!(handle.status(), RunStatus::Running);
    assert_eq!(toolset.call_count(), 1);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Running);
    let recovered = CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover failed run");
    assert_eq!(recovered.state().phase, Some(RunPhase::AfterToolBatch));
    assert!(matches!(
        recovered
            .state()
            .last_tool_batch
            .as_ref()
            .map(|batch| &batch.outcome),
        Some(finstack_ai_kernel::ToolBatchOutcome::Failed { error })
            if error.code.as_str() == "fixture_fail_run"
    ));
    let tool_results = recovered
        .state()
        .messages
        .iter()
        .flat_map(Message::content)
        .filter(|content| matches!(content, ContentBlock::ToolResult(_)))
        .count();
    assert_eq!(tool_results, 3);
    owner.shutdown().await;
}

#[tokio::test]
async fn reverse_completion_commits_arrivals_but_finalizes_tool_messages_in_source_order() {
    let plans = (0..4)
        .map(|value| gated_tool(&format!("gate-{value}"), value))
        .collect();
    let (mut owner, store, toolset, _) = setup(4, plans, 4, 4, ToolExecutionMode::Parallel).await;
    for value in 0..4 {
        while toolset.control().entries(format!("gate-{value}")) == 0 {
            tokio::task::yield_now().await;
        }
    }
    for value in (0..4).rev() {
        toolset.control().release(format!("gate-{value}"));
        tokio::task::yield_now().await;
    }
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    let loaded = finstack_ai_runtime::JournalStore::load(
        store.as_ref(),
        finstack_ai_runtime::LoadRequest {
            session_id: id::<SessionTag>(1),
        },
    )
    .await
    .expect("load");
    let arrivals = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .filter_map(|record| match record.body() {
            RecordBody::EffectCompleted(completion)
                if completion.output_contract().kind == EffectOutputKind::ToolResult =>
            {
                serde_json::from_str::<ToolResultBlock>(completion.output().as_str())
                    .ok()
                    .map(|result| *result.tool_call_id())
            }
            _ => None,
        })
        .collect::<Vec<ToolCallId>>();
    assert_eq!(arrivals.len(), 4);
    assert_ne!(arrivals, {
        let mut sorted = arrivals.clone();
        sorted.sort();
        sorted
    });
    let recovered = CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover");
    let finalized = recovered
        .state()
        .messages
        .iter()
        .skip(1)
        .filter_map(|message| match message.content() {
            [ContentBlock::ToolResult(result)] => Some(*result.tool_call_id()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let source = recovered.state().messages[0]
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(*call.tool_call_id()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finalized, source);
    owner.shutdown().await;
}

#[tokio::test]
async fn native_panic_becomes_stable_call_failure_without_leaking_payload_or_killing_sibling() {
    let secret = "panic-payload-must-not-be-durable";
    let plans = vec![
        ScriptedToolPlan {
            panic_on_call: Some(Arc::from(secret)),
            actions: Vec::new(),
        },
        completed_tool(1),
    ];
    let (mut owner, store, toolset, handle) =
        setup(2, plans, 2, 2, ToolExecutionMode::Parallel).await;
    wait_for_phase(&store, RunPhase::AfterToolBatch).await;
    let loaded = finstack_ai_runtime::JournalStore::load(
        store.as_ref(),
        finstack_ai_runtime::LoadRequest {
            session_id: id::<SessionTag>(1),
        },
    )
    .await
    .expect("load");
    let bytes = serde_json::to_vec(&loaded.committed_batches).expect("journal JSON");
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(text.contains("tool_panicked"));
    assert!(!text.contains(secret));
    assert_eq!(toolset.call_count(), 2);
    assert_eq!(toolset.active_call_count(), 0);
    assert_eq!(handle.status(), RunStatus::Running);
    owner.shutdown().await;
}
