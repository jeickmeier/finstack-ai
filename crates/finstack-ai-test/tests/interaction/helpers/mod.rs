//! PR-044 typed-interaction crash, router, and envelope proofs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendBatchTag, AuthorizationEvidence, BudgetPropagation,
    CancellationPropagation, ComponentId, ComponentRef, ContentBlock, Digest, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectTag, EventTag, Id, IdTag, InteractionId,
    InteractionKind, InteractionRequest, InteractionResolution, InteractionTag, KernelInput,
    LaneTag, Message, MessageRole, Metadata, OperationLocator, OutputSpec, PrincipalPropagation,
    PrincipalRef, ProviderIds, RawJson, RecordBody, RecordTag, ReducerStageOutcome, RetrySafety,
    RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation, RunSecurityContext,
    SessionTag, Stage, StageCursor, TextBlock, Timestamp, TransitionEnv, Version,
};
use finstack_ai_kernel::{InteractionResolutionCommand, ToolFailurePolicy};
use finstack_ai_runtime::{
    ApprovalGrantMode, ApprovalMetadata, ApprovalRequirement, CommitCoordinator, EventHubConfig,
    IdGenerationError, InteractionRouter, JournalStore, JsonSchemaToolValidatorCompiler,
    LoadRequest, LockedModelContextProfile, Model, ModelContextProfile, ModelRequestDraft,
    ModelRequestLimits, ModelResponse, ModelSettings, ModelStreamItem, ModelStreamLimits,
    ModelTaskConfig, ModelToolCall, RandomSource, ResolvedToolCatalog, RunHandle, RunHandleError,
    RunTaskConfig, RunTaskOwner, SameIdentityRetryPolicy, SecurityAuditError, SecurityAuditEvent,
    SecurityAuditGate, SecurityAuditHealth, SecurityAuditReceipt, SecurityAuditSink,
    SideEffectClass, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, ToolDeferralSupport,
    ToolExecutionPolicy, ToolPolicyDecision, ToolResult, ToolSpec, ToolStreamItem,
    ToolStreamLimits, ToolTaskConfig, Toolset, ToolsetRegistration, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    FixedClock, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction,
    ScriptedToolPlan, ScriptedToolset,
};

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

pub(crate) fn locked_profile() -> LockedModelContextProfile {
    resolve_model_context_profile(profile(), None, None, false).expect("locked profile")
}

pub(crate) fn tool_spec() -> ToolSpec {
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
        deferral: ToolDeferralSupport::Never,
    }
}

pub(crate) fn approval_catalog(toolset: Arc<ScriptedToolset>) -> Arc<ResolvedToolCatalog> {
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

pub(crate) fn model_plan() -> ScriptedModelPlan {
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    let response = ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: Arc::from([ModelToolCall {
            name: Arc::from("echo"),
            arguments: arguments.clone(),
            provider_call_id: None,
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
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))),
        ],
    }
}

pub(crate) fn completed_tool() -> ScriptedToolPlan {
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

pub(crate) struct CounterRandom(AtomicU64);

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

pub(crate) fn request_env(now: i64) -> TransitionEnv {
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

pub(crate) fn resolve_env(now: i64) -> TransitionEnv {
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

pub(crate) fn accepted(deadline: Option<Timestamp>) -> RunAccepted {
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

pub(crate) fn draft(messages: Arc<[Message]>, tools: Arc<[ToolSpec]>) -> ModelRequestDraft {
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

pub(crate) async fn drive_to_after_model(
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

pub(crate) fn memory_store() -> Arc<MemoryJournalStore> {
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

pub(crate) async fn spawn_owner(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
    clock_ms: i64,
    random: u64,
) -> Result<RunTaskOwner, RunHandleError> {
    Box::pin(RunTaskOwner::spawn_with_model_and_tools(
        coordinator,
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
        model,
        locked_profile(),
        catalog,
        FixedClock::new(timestamp(clock_ms)),
        CounterRandom(AtomicU64::new(random)),
    ))
    .await
}

pub(crate) async fn recover_session(store: &Arc<MemoryJournalStore>) -> CommitCoordinator {
    CommitCoordinator::recover(store.clone(), id::<SessionTag>(1))
        .await
        .expect("recover")
}

pub(crate) async fn wait_phase(store: &Arc<MemoryJournalStore>, phase: RunPhase) {
    wait_state(store, |state| state.phase == Some(phase)).await;
}

pub(crate) async fn wait_state(
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

pub(crate) struct ApprovalPorts {
    pub(crate) store: Arc<MemoryJournalStore>,
    pub(crate) toolset: Arc<ScriptedToolset>,
    pub(crate) model: Arc<dyn Model>,
    pub(crate) catalog: Arc<ResolvedToolCatalog>,
    pub(crate) tools: Arc<[ToolSpec]>,
}

pub(crate) fn approval_ports() -> ApprovalPorts {
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

pub(crate) async fn park_on_approval(deadline: Option<Timestamp>) -> (ApprovalPorts, RunTaskOwner) {
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

pub(crate) async fn crash_owner(
    owner: RunTaskOwner,
    store: &Arc<MemoryJournalStore>,
) -> CommitCoordinator {
    drop(owner);
    recover_session(store).await
}

pub(crate) fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
}

pub(crate) fn principal() -> PrincipalRef {
    PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal")
}

pub(crate) fn authorization() -> AuthorizationEvidence {
    AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization")
}

pub(crate) fn approval_schema() -> RawJson {
    RawJson::parse(
        r#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
    )
    .expect("schema")
}

pub(crate) fn approval_response(approved: bool) -> RawJson {
    RawJson::parse(if approved {
        r#"{"approved":true}"#
    } else {
        r#"{"approved":false}"#
    })
    .expect("response")
}

pub(crate) fn typed_request(kind: InteractionKind) -> InteractionRequest {
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

pub(crate) fn resolution(interaction_id: InteractionId, approved: bool) -> InteractionResolution {
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

pub(crate) fn resolve_command(
    interaction_id: InteractionId,
    approved: bool,
) -> InteractionResolutionCommand {
    InteractionResolutionCommand::try_new(locator(), resolution(interaction_id, approved))
        .expect("command")
}

pub(crate) fn free_text_schema() -> RawJson {
    RawJson::parse(
        r#"{"additionalProperties":false,"properties":{"answer":{"type":"string"}},"required":["answer"],"type":"object"}"#,
    )
    .expect("schema")
}

pub(crate) fn request_with_schema(schema: RawJson) -> InteractionRequest {
    InteractionRequest::try_new(
        1,
        id::<InteractionTag>(501),
        id::<EffectTag>(502),
        InteractionKind::FreeText,
        vec![ContentBlock::Text(
            TextBlock::try_new("resolve the outstanding interaction").expect("prompt"),
        )],
        schema,
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

pub(crate) fn resolve_command_with_response(
    interaction_id: InteractionId,
    response: RawJson,
) -> InteractionResolutionCommand {
    InteractionResolutionCommand::try_new(
        locator(),
        InteractionResolution::try_new(
            interaction_id,
            "resolution-1",
            principal(),
            authorization(),
            response,
            None::<&str>,
        )
        .expect("resolution"),
    )
    .expect("command")
}

pub(crate) struct RecordingSink {
    events: std::sync::Mutex<Vec<SecurityAuditEvent>>,
}

impl RecordingSink {
    pub(crate) fn new() -> Arc<Self> {
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

pub(crate) async fn router(store: Arc<MemoryJournalStore>) -> InteractionRouter {
    let journal: Arc<dyn JournalStore> = store;
    InteractionRouter::trusted(journal)
        .await
        .expect("trusted router")
}

pub(crate) async fn audited_router(
    store: Arc<MemoryJournalStore>,
    sink: Arc<RecordingSink>,
) -> InteractionRouter {
    let gate = SecurityAuditGate::enable(Some(sink), StdDuration::from_millis(100))
        .await
        .expect("gate");
    let journal: Arc<dyn JournalStore> = store;
    InteractionRouter::new(journal, gate)
}

pub(crate) async fn record_kinds(store: &Arc<MemoryJournalStore>) -> Vec<&'static str> {
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

pub(crate) fn pending_id(state: &finstack_ai_kernel::KernelState) -> InteractionId {
    state
        .pending_interaction
        .as_ref()
        .expect("pending")
        .request
        .interaction_id()
}
