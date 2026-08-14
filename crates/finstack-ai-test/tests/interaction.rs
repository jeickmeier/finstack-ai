//! PR-044 typed-interaction crash, router, and envelope proofs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendBatchTag, AuthorizationEvidence, BudgetPropagation,
    CancellationPropagation, ComponentId, ComponentRef, ContentBlock, Digest, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectTag, EventTag, Id, IdTag, InteractionCancelled,
    InteractionId, InteractionKind, InteractionRequest, InteractionResolution, InteractionSettled,
    InteractionTag, InteractionTerminalOutcome, KernelInput, LaneTag, Message, MessageRole,
    Metadata, OperationLocator, OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds,
    RawJson, RecordBody, RecordTag, ReducerStageOutcome, RequestInteraction, RetrySafety,
    RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation, RunSecurityContext,
    SessionTag, Stage, StageCursor, TextBlock, Timestamp, TransitionEnv, Version,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, CommitCoordinator, EventHubConfig, ExternalRouteOutcome,
    IdGenerationError, InteractionResolutionCommand, InteractionResumeAction, InteractionRouter,
    JournalStore, JsonSchemaToolValidatorCompiler, LoadRequest, LockedModelContextProfile, Model,
    ModelContextProfile, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamItem, ModelStreamLimits, ModelTaskConfig, ModelToolCall, RandomSource,
    ResolvedToolCatalog, RunHandle, RunHandleError, RunTaskConfig, RunTaskOwner,
    SecurityAuditError, SecurityAuditEvent, SecurityAuditGate, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink, SideEffectClass, TokenEstimatorRef,
    TokenEstimatorSource, ToolCallDelta, ToolExecutionPolicy, ToolFailurePolicy,
    ToolPolicyDecision, ToolResult, ToolSpec, ToolStreamItem, ToolStreamLimits, ToolTaskConfig,
    Toolset, ToolsetRegistration, interaction_resume_action, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    FixedClock, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction,
    ScriptedToolPlan, ScriptedToolset,
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

fn tool_spec() -> ToolSpec {
    ToolSpec {
        id: finstack_ai_kernel::ToolId::parse("finstack.tools.echo").expect("tool id"),
        model_name: Arc::from("echo"),
        title: Arc::from("echo"),
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
    }
}

fn approval_catalog(toolset: Arc<ScriptedToolset>) -> Arc<ResolvedToolCatalog> {
    let toolset_port: Arc<dyn Toolset> = toolset;
    let policies = toolset_port
        .tools()
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

fn model_plan() -> ScriptedModelPlan {
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    let response = ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: Arc::from([ModelToolCall {
            name: Arc::from("echo"),
            arguments: arguments.clone(),
        }]),
        usage: finstack_ai_kernel::Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("completion-tools"),
        continuation_state: None,
    };
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("echo")),
                arguments_delta: Arc::from(arguments.as_str()),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))),
        ],
    }
}

fn completed_tool() -> ScriptedToolPlan {
    ScriptedToolPlan {
        panic_on_call: None,
        actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(
            ToolResult {
                output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("result"),
                is_error: false,
            },
        )))],
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

fn request_env(now: i64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id::<RecordTag>(80), id::<RecordTag>(81)],
            vec![id::<EventTag>(80), id::<EventTag>(81)],
            vec![id::<EffectTag>(502)],
            vec![id::<InteractionTag>(501)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id::<AppendBatchTag>(180)],
            Vec::new(),
        )
        .expect("request ids"),
    }
}

fn resolve_env(now: i64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id::<RecordTag>(82), id::<RecordTag>(83)],
            vec![id::<EventTag>(82), id::<EventTag>(83)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id::<AppendBatchTag>(181)],
            Vec::new(),
        )
        .expect("resolve ids"),
    }
}

fn accepted(deadline: Option<Timestamp>) -> RunAccepted {
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
        deadline,
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

async fn drive_to_after_model(
    handle: &RunHandle,
    store: &Arc<MemoryJournalStore>,
    tools: Arc<[ToolSpec]>,
    deadline: Option<Timestamp>,
) {
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: accepted(deadline),
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
    wait_phase(store, RunPhase::AfterModel).await;
}

fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 128,
            records_per_session: 512,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    )
}

async fn spawn_owner(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    clock_ms: i64,
    random: u64,
) -> Result<RunTaskOwner, RunHandleError> {
    RunTaskOwner::spawn_with_model_and_tools(
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
        ToolTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        model,
        locked_profile(),
        catalog,
        FixedClock::new(timestamp(clock_ms)),
        CounterRandom(AtomicU64::new(random)),
    )
    .await
}

async fn recover_session(store: &Arc<MemoryJournalStore>) -> CommitCoordinator {
    CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover")
}

async fn wait_phase(store: &Arc<MemoryJournalStore>, phase: RunPhase) {
    wait_state(store, |state| state.phase == Some(phase)).await;
}

async fn wait_state(
    store: &Arc<MemoryJournalStore>,
    predicate: impl Fn(&finstack_ai_kernel::KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let recovered = recover_session(store).await;
            if predicate(recovered.state()) {
                return recovered;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("state wait")
}

struct ApprovalPorts {
    store: Arc<MemoryJournalStore>,
    toolset: Arc<ScriptedToolset>,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    tools: Arc<[ToolSpec]>,
}

fn approval_ports() -> ApprovalPorts {
    let tools: Arc<[ToolSpec]> = Arc::from([tool_spec()]);
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(&tools),
        vec![completed_tool()],
    ));
    let catalog = approval_catalog(Arc::clone(&toolset));
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(profile(), vec![model_plan()]));
    ApprovalPorts {
        store: memory_store(),
        toolset,
        model,
        catalog,
        tools,
    }
}

async fn park_on_approval(deadline: Option<Timestamp>) -> (ApprovalPorts, RunTaskOwner) {
    let ports = approval_ports();
    let owner = spawn_owner(
        CommitCoordinator::new(ports.store.clone()),
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_500,
        700,
    )
    .await
    .expect("owner");
    drive_to_after_model(
        &owner.handle(),
        &ports.store,
        Arc::clone(&ports.tools),
        deadline,
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
    wait_phase(&ports.store, RunPhase::AwaitingInteraction).await;
    (ports, owner)
}

async fn crash_owner(owner: RunTaskOwner, store: &Arc<MemoryJournalStore>) -> CommitCoordinator {
    drop(owner);
    recover_session(store).await
}

fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
}

fn principal() -> PrincipalRef {
    PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal")
}

fn authorization() -> AuthorizationEvidence {
    AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization")
}

fn approval_schema() -> RawJson {
    RawJson::parse(
        r#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
    )
    .expect("schema")
}

fn approval_response(approved: bool) -> RawJson {
    RawJson::parse(if approved {
        r#"{"approved":true}"#
    } else {
        r#"{"approved":false}"#
    })
    .expect("response")
}

fn typed_request(kind: InteractionKind) -> InteractionRequest {
    InteractionRequest::try_new(
        1,
        id::<InteractionTag>(501),
        id::<EffectTag>(502),
        kind,
        vec![ContentBlock::Text(
            TextBlock::try_new("resolve the outstanding interaction").expect("prompt"),
        )],
        approval_schema(),
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").expect("component"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        None,
        false,
        Metadata::empty(),
    )
    .expect("request")
}

fn resolution(interaction_id: InteractionId, approved: bool) -> InteractionResolution {
    InteractionResolution::try_new(
        interaction_id,
        "resolution-1",
        principal(),
        authorization(),
        approval_response(approved),
        None::<&str>,
    )
    .expect("resolution")
}

fn resolve_command(interaction_id: InteractionId, approved: bool) -> InteractionResolutionCommand {
    InteractionResolutionCommand::try_new(locator(), resolution(interaction_id, approved))
        .expect("command")
}

struct RecordingSink {
    events: std::sync::Mutex<Vec<SecurityAuditEvent>>,
}

impl RecordingSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: std::sync::Mutex::new(Vec::new()),
        })
    }
}

impl SecurityAuditSink for RecordingSink {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> finstack_ai_runtime::PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
        let event_id = Arc::<str>::from(event.event_id());
        let recorded_at = event.timestamp();
        self.events.lock().expect("lock").push(event);
        Box::pin(async move {
            Ok(SecurityAuditReceipt {
                event_id,
                recorded_at,
            })
        })
    }

    fn health(
        &self,
    ) -> finstack_ai_runtime::PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
        Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
    }
}

async fn router(store: Arc<MemoryJournalStore>) -> InteractionRouter {
    let journal: Arc<dyn JournalStore> = store;
    InteractionRouter::trusted(journal)
        .await
        .expect("trusted router")
}

async fn audited_router(
    store: Arc<MemoryJournalStore>,
    sink: Arc<RecordingSink>,
) -> InteractionRouter {
    let gate = SecurityAuditGate::enable(Some(sink), StdDuration::from_millis(100))
        .await
        .expect("gate");
    let journal: Arc<dyn JournalStore> = store;
    InteractionRouter::new(journal, gate)
}

async fn record_kinds(store: &Arc<MemoryJournalStore>) -> Vec<&'static str> {
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    loaded
        .committed_batches
        .iter()
        .flat_map(|batch| {
            batch
                .records
                .iter()
                .filter_map(|record| match record.body() {
                    RecordBody::InteractionRequested(_) => Some("interaction_requested"),
                    RecordBody::InteractionResolved(_) => Some("interaction_resolved"),
                    RecordBody::InteractionExpired(_) => Some("interaction_expired"),
                    RecordBody::InteractionCancelled(_) => Some("interaction_cancelled"),
                    RecordBody::EffectRequested(requested)
                        if requested.kind() == EffectKind::Interaction =>
                    {
                        Some("effect_requested_interaction")
                    }
                    RecordBody::EffectRequested(requested)
                        if requested.kind() == EffectKind::Tool =>
                    {
                        Some("effect_requested_tool")
                    }
                    RecordBody::EffectCompleted(_) => Some("effect_completed"),
                    RecordBody::EffectFailed(_) => Some("effect_failed"),
                    RecordBody::EffectCancelled(_) => Some("effect_cancelled"),
                    RecordBody::ExternalCommandRejected(_) => Some("external_command_rejected"),
                    _ => None,
                })
        })
        .collect()
}

fn pending_id(state: &finstack_ai_kernel::KernelState) -> InteractionId {
    state
        .pending_interaction
        .as_ref()
        .expect("pending")
        .request
        .interaction_id()
}

#[tokio::test]
async fn stop_after_approval_request_waits_without_dispatch() {
    let (ports, owner) = park_on_approval(None).await;
    assert_eq!(ports.toolset.call_count(), 0);
    let recovered = crash_owner(owner, &ports.store).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::AwaitingInteraction));
    assert_eq!(
        interaction_resume_action(recovered.state(), timestamp(2_500)),
        InteractionResumeAction::WaitResolution
    );
    assert!(
        !record_kinds(&ports.store)
            .await
            .contains(&"effect_requested_tool")
    );
    let listed = router(ports.store.clone())
        .await
        .list(&locator(), &principal(), &authorization(), timestamp(2_500))
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind(), &InteractionKind::Approval);
}

#[tokio::test]
async fn grant_after_stop_dispatches_the_protected_tool_once() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    let recovered = crash_owner(owner, &ports.store).await;
    let _ = recovered;
    let outcome = router(ports.store.clone())
        .await
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("grant");
    assert!(matches!(outcome, ExternalRouteOutcome::Committed(_)));
    let recovered = recover_session(&ports.store).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::BeforeToolBatch));
    assert_eq!(
        recovered
            .state()
            .last_interaction_terminal
            .as_ref()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Granted
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_700,
        800,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |_| ports.toolset.call_count() == 1).await;
    assert_eq!(ports.toolset.call_count(), 1);
    owner.shutdown().await;
}

#[tokio::test]
async fn denied_approval_never_dispatches_the_protected_tool() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    drop(owner);
    router(ports.store.clone())
        .await
        .route(resolve_command(interaction_id, false), timestamp(2_600))
        .await
        .expect("deny");
    let recovered = recover_session(&ports.store).await;
    assert_eq!(
        recovered
            .state()
            .last_interaction_terminal
            .as_ref()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Denied
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_700,
        801,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |state| {
        state.phase == Some(RunPhase::AfterToolBatch)
            || state
                .active_tool_batch
                .as_ref()
                .is_some_and(|batch| !batch.calls.is_empty())
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    assert!(
        !record_kinds(&ports.store)
            .await
            .contains(&"effect_requested_tool")
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn expired_resolution_never_dispatches_the_protected_tool() {
    let (ports, owner) = park_on_approval(Some(timestamp(3_000))).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    owner
        .handle()
        .submit(
            resolve_env(3_000),
            KernelInput::InteractionSettled(InteractionSettled::Expired(
                finstack_ai_kernel::InteractionExpired {
                    interaction_id,
                    expired_at: timestamp(3_000),
                },
            )),
        )
        .await
        .expect("live expire");
    drop(owner);
    let recovered = recover_session(&ports.store).await;
    assert_eq!(
        recovered
            .state()
            .last_interaction_terminal
            .as_ref()
            .expect("terminal")
            .outcome,
        InteractionTerminalOutcome::Expired
    );
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"interaction_expired")
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_700,
        802,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |state| {
        state.phase == Some(RunPhase::AfterToolBatch) || state.last_interaction_terminal.is_some()
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    owner.shutdown().await;
}

#[tokio::test]
async fn expire_if_due_on_restore_never_dispatches() {
    let (ports, owner) = park_on_approval(Some(timestamp(3_000))).await;
    let recovered = crash_owner(owner, &ports.store).await;
    assert_eq!(
        interaction_resume_action(recovered.state(), timestamp(4_000)),
        InteractionResumeAction::ExpireIfDue
    );
    let mut owner = spawn_owner(
        recovered,
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        4_000,
        803,
    )
    .await
    .expect("respawn");
    wait_state(&ports.store, |state| {
        state
            .last_interaction_terminal
            .as_ref()
            .is_some_and(|terminal| terminal.outcome == InteractionTerminalOutcome::Expired)
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"interaction_expired")
    );
    owner.shutdown().await;
}

#[tokio::test]
async fn duplicate_resolution_is_idempotent() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    drop(owner);
    let router = router(ports.store.clone()).await;
    let first = router
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("first");
    assert!(matches!(first, ExternalRouteOutcome::Committed(_)));
    let second = router
        .route(resolve_command(interaction_id, true), timestamp(2_700))
        .await
        .expect("duplicate");
    assert!(matches!(second, ExternalRouteOutcome::Idempotent { .. }));
    let kinds = record_kinds(&ports.store).await;
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == "interaction_resolved")
            .count(),
        1
    );
}

#[tokio::test]
async fn conflicting_resolution_fails_closed_and_is_audited() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    drop(owner);
    let sink = RecordingSink::new();
    let router = audited_router(ports.store.clone(), Arc::clone(&sink)).await;
    router
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("first");
    let outcome = router
        .route(resolve_command(interaction_id, false), timestamp(2_700))
        .await
        .expect("conflict");
    assert!(matches!(
        outcome,
        ExternalRouteOutcome::Rejected {
            reason_code: "conflicting_or_invalid_resolution",
            ..
        }
    ));
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"external_command_rejected")
    );
}

#[tokio::test]
async fn cancelled_while_waiting_never_dispatches() {
    let (ports, owner) = park_on_approval(None).await;
    let interaction_id = pending_id(recover_session(&ports.store).await.state());
    owner
        .handle()
        .submit(
            resolve_env(2_600),
            KernelInput::InteractionSettled(InteractionSettled::Cancelled(
                InteractionCancelled::try_new(interaction_id, None, None, Some("operator-cancel"))
                    .expect("cancelled"),
            )),
        )
        .await
        .expect("cancel");
    wait_state(&ports.store, |state| {
        state
            .last_interaction_terminal
            .as_ref()
            .is_some_and(|terminal| terminal.outcome == InteractionTerminalOutcome::Cancelled)
    })
    .await;
    assert_eq!(ports.toolset.call_count(), 0);
    assert!(
        record_kinds(&ports.store)
            .await
            .contains(&"interaction_cancelled")
    );
    drop(owner);
}

async fn park_on_kind(kind: InteractionKind) -> (ApprovalPorts, RunTaskOwner, InteractionId) {
    let ports = approval_ports();
    let owner = spawn_owner(
        CommitCoordinator::new(ports.store.clone()),
        Arc::clone(&ports.model),
        Arc::clone(&ports.catalog),
        2_500,
        710,
    )
    .await
    .expect("owner");
    drive_to_after_model(
        &owner.handle(),
        &ports.store,
        Arc::clone(&ports.tools),
        None,
    )
    .await;
    owner
        .handle()
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(kind),
            }),
        )
        .await
        .expect("request");
    wait_phase(&ports.store, RunPhase::AwaitingInteraction).await;
    (ports, owner, id::<InteractionTag>(501))
}

async fn resolve_kind_envelope(kind: InteractionKind) -> Vec<&'static str> {
    let (ports, owner, interaction_id) = park_on_kind(kind).await;
    drop(owner);
    router(ports.store.clone())
        .await
        .route(resolve_command(interaction_id, true), timestamp(2_600))
        .await
        .expect("resolve");
    record_kinds(&ports.store).await
}

fn assert_shared_envelope(kinds: &[&str]) {
    for required in [
        "effect_requested_interaction",
        "interaction_requested",
        "interaction_resolved",
        "effect_completed",
    ] {
        assert!(kinds.contains(&required), "missing {required} in {kinds:?}");
    }
}

#[tokio::test]
async fn approval_choice_and_review_share_the_same_envelope() {
    for kind in [
        InteractionKind::Approval,
        InteractionKind::Choice,
        InteractionKind::Review,
    ] {
        let kinds = resolve_kind_envelope(kind.clone()).await;
        assert_shared_envelope(&kinds);
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == "interaction_requested")
                .count(),
            1,
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn remaining_kinds_request_and_resolve() {
    for kind in [
        InteractionKind::Form,
        InteractionKind::FreeText,
        InteractionKind::Correction,
        InteractionKind::Custom {
            name: Arc::from("team.custom"),
        },
    ] {
        let kinds = resolve_kind_envelope(kind.clone()).await;
        assert_shared_envelope(&kinds);
    }
}

#[test]
fn unpaired_projection_is_uncertain() {
    let state = finstack_ai_kernel::KernelState {
        phase: Some(RunPhase::AwaitingInteraction),
        ..finstack_ai_kernel::KernelState::default()
    };
    assert_eq!(
        interaction_resume_action(&state, timestamp(1_000)),
        InteractionResumeAction::SuspendUncertain
    );
}
