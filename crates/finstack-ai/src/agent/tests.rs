use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use finstack_ai_kernel::{
    ActiveCapability, ActiveToolCallStatus, AgentId, ArtifactId, ArtifactRef,
    AuthorizationEvidence, BlobRef, BundleId, CapabilityActivationSource, CapabilityId,
    ChildPlacement, ChildRunLocator, ComponentId, ComponentInvocation, ComponentRef, ContentBlock,
    Digest, EffectId, EffectTag, ExternalEffectCompletion, ExternalEffectCompletionCommand,
    ExternalEffectOutcome, InvocationRecovery, MediaRef, OperationLocator, ProviderIds, RawJson,
    RecordBody, RetryClassification, RetryDirective, RunEventClass, RunPhase, RunSecurityContext,
    RunTag, Stage, TerminalState, TextBlock, ToolExecutionMode, ToolId, Usage, Version,
};
use finstack_ai_kernel::{
    BudgetRequest, ExternalHandleRef, Metadata, ReconciliationPolicy, RetrySafety,
};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, AgentRef, ApprovalMetadata, ApprovalRequirement,
    ChildRunContext, ChildRunHandle, ChildRunRequest, CommitCoordinator, ExternalRouteOutcome,
    JournalStore, LoadRequest, Middleware, MiddlewareContext, MiddlewareDescriptor,
    MiddlewareError, MiddlewareOrder, MiddlewareRole, Model, ModelContextProfile, ModelDeferral,
    ModelName, ModelResponse, ModelStreamItem, ModelToolCall, NoopObserver, Observer,
    ObserverDescriptor, ObserverError, ObserverPayloadMode, OrderTier, PortFuture, SideEffectClass,
    StageInput, StageMask, StageOutcome, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta,
    ToolDeferral, ToolDeferralSupport, ToolSpec, ToolStreamItem, Toolset, child_relation_digest,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ManualGate, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedObserver,
    ScriptedObserverAction, ScriptedToolAction, ScriptedToolPlan, ScriptedToolset,
};
use finstack_ai_tools_calculator::CalculatorToolset;

use super::builder::validate_compact_catalog;
use super::types::MAX_COMPACT_CATALOG_BYTES;
use super::*;
use crate::{
    AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleDefaults,
    BundleResolver, BundleSpec, CapabilityActivation, CapabilitySpec, ChildRunPolicy,
    CompatibilityRequirements, Extension, ExtensionDescriptor, InstructionSpec, ReadyComponent,
    Registrar, RegistrationError, RegistrationMetadata, RunPolicy, RuntimeServices, Session,
};

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

struct PreviewExtension {
    model_id: ComponentId,
    store_id: ComponentId,
    model: Arc<ScriptedModel>,
    store: Arc<MemoryJournalStore>,
    calculator: Option<Arc<CalculatorToolset>>,
}

impl Extension for PreviewExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor::trusted_in_process(
            ComponentId::parse("test.extension.preview").expect("extension id"),
            VERSION,
        )
    }

    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
        let model: Arc<dyn Model> = self.model.clone();
        registrar.model(
            RegistrationMetadata::new(self.model_id.clone(), VERSION),
            ReadyComponent::new(model),
        )?;
        if let Some(calculator) = &self.calculator {
            let toolset: Arc<dyn Toolset> = calculator.clone();
            registrar.toolset(
                RegistrationMetadata::new(
                    ComponentId::parse("test.tools.calculator").expect("toolset id"),
                    VERSION,
                ),
                ReadyComponent::new(toolset),
            )?;
        }
        let store: Arc<dyn JournalStore> = self.store.clone();
        registrar.store(
            RegistrationMetadata::new(self.store_id.clone(), VERSION),
            ReadyComponent::new(store),
        )
    }
}

fn profile() -> finstack_ai_runtime::ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("preview-1").expect("model name"),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 1_048_576,
        max_output_tokens: 256,
        reserved_output_tokens: 256,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: Arc::from("bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn deferred(job: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
            ModelDeferral {
                handle: ExternalHandleRef::try_new(
                    ComponentId::parse("test.model.preview").expect("component"),
                    job,
                    RawJson::parse(b"{}").expect("metadata"),
                )
                .expect("handle"),
                reconciliation: ReconciliationPolicy::CallbackOnly,
                next_poll_at: None,
                expires_at: None,
            },
        )))],
    }
}

fn completed(text: &str) -> ScriptedModelPlan {
    completed_with_id(text, "preview-completion")
}

fn completed_with_id(text: &str, completion_id: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                finstack_ai_runtime::TextDelta {
                    text: Arc::from(text),
                },
            ))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([ContentBlock::Text(
                    TextBlock::try_new(text).expect("assistant text"),
                )]),
                tool_calls: Arc::from([]),
                usage: Usage::empty(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from(completion_id),
                continuation_state: None,
            }))),
        ],
    }
}

fn calculator_call() -> ScriptedModelPlan {
    let arguments = RawJson::parse(br#"{"operation":"add","operands":[2,3]}"#).expect("arguments");
    let response = ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: Arc::from([ModelToolCall {
            name: Arc::from("calculator"),
            arguments: arguments.clone(),
            provider_call_id: Some(Arc::from("call-preview-calculator")),
        }]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("preview-tool-completion"),
        continuation_state: Some(
            RawJson::parse(br#"{"provider":"test","value":1}"#).expect("continuation"),
        ),
    };
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("calculator")),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: Some(Arc::from("call-preview-calculator")),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))),
        ],
    }
}

fn assert_continuation_reaches_second_request(model: &ScriptedModel) {
    let second_request = model.last_request().expect("second request");
    assert_eq!(
        second_request
            .continuation_state
            .as_ref()
            .map(RawJson::as_str),
        Some(r#"{"provider":"test","value":1}"#)
    );
    let provider_call_ids = second_request
        .draft
        .messages
        .iter()
        .flat_map(finstack_ai_kernel::Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => call.provider_call_id(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(provider_call_ids, ["call-preview-calculator"]);
}

fn security() -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-preview",
        finstack_ai_kernel::PrincipalRef::try_new(
            "preview-tests",
            "developer",
            Some("tenant-preview"),
        )
        .expect("principal"),
        "local",
        "test",
        "preview-policy-v1",
        "preview-decision-v1",
        None,
    )
    .expect("security")
}

async fn model_only_agent(model: Arc<ScriptedModel>) -> (Agent, Arc<MemoryJournalStore>) {
    model_only_agent_with_child_policy(model, ChildRunPolicy::Deny).await
}

async fn child_capable_agent(model: Arc<ScriptedModel>) -> (Agent, Arc<MemoryJournalStore>) {
    model_only_agent_with_child_policy(model, ChildRunPolicy::Allow { max_depth: 1 }).await
}

async fn model_only_agent_with_child_policy(
    model: Arc<ScriptedModel>,
    child_runs: ChildRunPolicy,
) -> (Agent, Arc<MemoryJournalStore>) {
    let model_id = ComponentId::parse("test.model.preview").expect("model id");
    let store_id = ComponentId::parse("test.store.preview").expect("store id");
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&PreviewExtension {
            model_id: model_id.clone(),
            store_id: store_id.clone(),
            model,
            store: Arc::clone(&store),
            calculator: None,
        })
        .expect("registration");
    let mut registry = registrar.into_registry();
    let agent_id = AgentId::parse("test.agent.preview").expect("agent id");
    let spec = AgentBuilder::new(
        agent_id.clone(),
        ComponentRef::new(model_id, Some(VERSION)),
        ComponentRef::new(store_id, Some(VERSION)),
    )
    .policy(RunPolicy {
        child_runs,
        ..RunPolicy::default()
    })
    .build()
    .expect("spec");
    let bundle_id = BundleId::parse("test.bundle.preview").expect("bundle id");
    let mut catalog = BundleCatalog::default();
    catalog
        .install(BundleSpec {
            schema_version: BUNDLE_SCHEMA_VERSION,
            id: bundle_id.clone(),
            version: VERSION,
            agents: Arc::from([spec]),
            capabilities: Arc::from([]),
            requirements: Arc::from([]),
            conflicts: Arc::from([]),
            defaults: BundleDefaults::default(),
            config_schema: None,
            compatibility: CompatibilityRequirements::default(),
        })
        .expect("bundle");
    let composed_agent = BundleResolver::new(
        &catalog,
        VERSION,
        BTreeSet::new(),
        RuntimeServices::default(),
    )
    .resolve_agent(
        &mut registry,
        &bundle_id,
        &agent_id,
        BTreeMap::new(),
        AgentConstructionContext::new(),
    )
    .await
    .expect("resolved");
    (
        Agent::try_from_resolved(Arc::new(composed_agent)).expect("Agent"),
        store,
    )
}

fn request(input: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("preview-1").expect("model name"),
        input,
        security(),
    )
    .expect("request")
}

fn test_attachment() -> AttachmentInput {
    let content = b"attachment bytes";
    let digest = Digest::blob_content(content);
    let blob = BlobRef::try_new(
        "blob-attachment-1",
        "text/plain",
        u64::try_from(content.len()).expect("length"),
        Some(digest),
        Some("notes.txt"),
    )
    .expect("blob");
    let artifact = ArtifactRef::try_new(
        ArtifactId::from_bytes([9; 16]),
        "document",
        blob,
        digest,
        Digest::raw_json(b"scope"),
        Metadata::empty(),
    )
    .expect("artifact");
    AttachmentInput { artifact }
}

#[test]
fn run_request_defaults_to_no_attachments() {
    let run_request = request("unused");
    assert!(run_request.attachments.is_empty());
}

#[test]
fn run_request_rejects_more_than_max_attachments() {
    let mut run_request = request("unused");
    let attachment = test_attachment();
    run_request.attachments = std::iter::repeat_with(|| attachment.clone())
        .take(MAX_RUN_ATTACHMENTS + 1)
        .collect();
    assert!(run_request.validate().is_err());
}

#[tokio::test]
async fn prepared_user_message_carries_file_blocks_for_attachments() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("preview ready")],
    ));
    let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
    let attachment = test_attachment();
    let mut run_request = request("Say hello");
    run_request.attachments = Arc::from([attachment.clone()]);
    agent.run(run_request).await.expect("run");
    let sent_request = model.last_request().expect("first request");
    let file_blocks: Vec<&MediaRef> = sent_request
        .draft
        .messages
        .iter()
        .filter(|message| message.role() == finstack_ai_kernel::MessageRole::User)
        .flat_map(finstack_ai_kernel::Message::content)
        .filter_map(|block| match block {
            ContentBlock::File(media) => Some(media),
            _ => None,
        })
        .collect();
    assert_eq!(file_blocks.len(), 1);
    assert_eq!(file_blocks[0].blob(), attachment.artifact.blob());
}

#[test]
fn compact_model_catalog_is_bounded_and_tokenized_deterministically() {
    let oversized = CapabilitySpec {
        id: CapabilityId::parse("test.capability.oversized").expect("capability id"),
        description: Arc::from("x".repeat(MAX_COMPACT_CATALOG_BYTES + 1)),
        instructions: Arc::from([]),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation: CapabilityActivation::Model,
    };
    let error = validate_compact_catalog(&[oversized]).expect_err("catalog must be bounded");
    assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
    assert!(
        error
            .to_string()
            .contains("compact capability catalog exceeds")
    );
    assert_eq!(request("unused").capability, None);
}

#[tokio::test]
async fn unknown_request_capability_fails_closed() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("unused")],
    ));
    let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
    let mut run_request = request("Say hello");
    run_request.capability =
        Some(CapabilityId::parse("test.capability.missing").expect("capability id"));
    let Err(error) = agent.start(run_request) else {
        panic!("unknown capability must fail closed");
    };
    assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
    assert!(error.to_string().contains("unknown model capability"));
}

#[tokio::test]
async fn model_only_agent_completes_without_double_warmup() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("preview ready")],
    ));
    let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
    assert_eq!(model.warmup_count(), 1);
    let output = agent.run(request("Say hello")).await.expect("run");
    assert_eq!(output.text(), "preview ready");
    assert_eq!(model.request_count(), 1);
    assert_eq!(model.warmup_count(), 1);
}

#[tokio::test]
async fn started_run_retains_result_and_delivers_bounded_batches() {
    let mut plan = completed("batched");
    plan.actions.splice(
        0..1,
        ["b", "a", "t", "c", "h", "e", "d"].map(|text| {
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
                finstack_ai_runtime::TextDelta {
                    text: Arc::from(text),
                },
            )))
        }),
    );
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
    let run = agent.start(request("batch events")).expect("start");
    let observer = run.clone();

    let batches = tokio::time::timeout(Duration::from_secs(3), async move {
        let mut batches = Vec::new();
        while let Some(batch) = observer.next_event_batch().await.expect("event batch") {
            assert_eq!(
                batch.first_sequence(),
                batch
                    .events()
                    .first()
                    .expect("first event")
                    .transient_sequence()
            );
            assert_eq!(
                batch.last_sequence(),
                batch
                    .events()
                    .last()
                    .expect("last event")
                    .transient_sequence()
            );
            assert!(
                batch
                    .events()
                    .windows(2)
                    .all(|events| events[0].transient_sequence() < events[1].transient_sequence())
            );
            batches.push(batch);
        }
        batches
    })
    .await
    .expect("event delivery settled");
    let event_count = batches
        .iter()
        .map(|batch| batch.events().len())
        .sum::<usize>();
    assert!(batches.iter().any(|batch| batch.events().len() > 1));
    assert!(event_count > batches.len());
    assert!(batches.iter().any(|batch| {
        batch
            .events()
            .iter()
            .any(|event| event.class() == RunEventClass::Transient)
    }));

    let first = run.result().await.expect("first retained result");
    let second = run.result().await.expect("second retained result");
    assert_eq!(first.text(), "batched");
    assert_eq!(second.text(), "batched");
    assert_eq!(first.locator, second.locator);
    assert_eq!(model.request_count(), 1);
}

#[tokio::test]
async fn cancelled_event_wait_restores_the_single_consumer() {
    let control_name = Arc::<str>::from("cancelled-event-wait");
    let mut plan = completed("event delivery resumed");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&control_name)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let control = model.control();
    let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
    let run = agent
        .start(request("resume event delivery"))
        .expect("start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&control_name) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model reached gate");

    let mut cancelled_wait = false;
    for _ in 0..32 {
        match tokio::time::timeout(Duration::from_millis(20), run.next_event_batch()).await {
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) => panic!("event stream closed before model release"),
            Ok(Err(error)) => panic!("event delivery failed: {error}"),
            Err(_) => {
                cancelled_wait = true;
                break;
            }
        }
    }
    assert!(
        cancelled_wait,
        "expected one pending batch wait to be cancelled"
    );

    control.release(&control_name);
    tokio::time::timeout(Duration::from_secs(3), async {
        while run.next_event_batch().await.expect("event batch").is_some() {}
    })
    .await
    .expect("event delivery resumed after waiter cancellation");
    assert_eq!(
        run.result().await.expect("retained result").text(),
        "event delivery resumed"
    );
}

#[tokio::test]
async fn event_lock_poison_fail_closes_result_and_poll() {
    let control_name = Arc::<str>::from("poisoned-event-lock");
    let mut plan = completed("must not settle after poison");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&control_name)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
    let run = agent.start(request("poison event lock")).expect("start");
    run.poison_events_lock();
    run.close_events();

    let poll = tokio::time::timeout(Duration::from_millis(200), run.next_event_batch())
        .await
        .expect("event poll must not hang")
        .expect_err("event poll fail-closed");
    assert_eq!(poll.code(), AGENT_RUN_RUNTIME_FAILURE);
    let result = tokio::time::timeout(Duration::from_millis(200), run.result())
        .await
        .expect("result must not hang")
        .expect_err("result fail-closed");
    assert_eq!(result.code(), AGENT_RUN_RUNTIME_FAILURE);
}

#[tokio::test]
async fn explicit_cancellation_is_idempotent_and_terminal() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::AwaitCancellation],
        }],
    ));
    let (agent, _store) = model_only_agent(Arc::clone(&model)).await;
    let run = agent
        .start(request("wait for cancellation"))
        .expect("start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while model.request_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model request started");

    run.cancel().await.expect("first cancellation");
    run.cancel().await.expect("idempotent cancellation");
    let error = run.result().await.expect_err("cancelled result");
    assert_eq!(error.code(), AGENT_RUN_CANCELLED);
    assert!(!error.retryable());
    assert_eq!(model.cancellation_acknowledgement_count(), 1);
    assert_eq!(model.active_stream_count(), 0);
}

#[tokio::test]
async fn dropping_last_handle_detaches_without_cancelling_execution() {
    let control_name = Arc::<str>::from("detached-run");
    let mut plan = completed("detached completion");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&control_name)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let control = model.control();
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let run = agent.start(request("detach")).expect("start");
    let session_id = run.locator().session_id;
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&control_name) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model reached gate");

    drop(run);
    control.release(&control_name);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let journal: Arc<dyn JournalStore> = store.clone();
            let state = CommitCoordinator::recover(journal, session_id)
                .await
                .expect("recover detached run");
            if matches!(state.state().terminal, Some(TerminalState::Completed(_))) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached run completed");
    assert_eq!(model.cancellation_acknowledgement_count(), 0);
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
}

#[tokio::test]
async fn tool_loop_executes_read_only_calculator_then_completes() {
    let model_id = ComponentId::parse("test.model.preview-tool").expect("model id");
    let store_id = ComponentId::parse("test.store.preview-tool").expect("store id");
    let toolset_id = ComponentId::parse("test.tools.calculator").expect("toolset id");
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![calculator_call(), completed("five")],
    ));
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let mut registrar = Registrar::new();
    registrar
        .register_extension(&PreviewExtension {
            model_id: model_id.clone(),
            store_id: store_id.clone(),
            model: Arc::clone(&model),
            store,
            calculator: Some(Arc::new(CalculatorToolset::try_new().expect("calculator"))),
        })
        .expect("registration");
    let mut registry = registrar.into_registry();
    let agent_id = AgentId::parse("test.agent.preview-tool").expect("agent id");
    let spec = AgentBuilder::new(
        agent_id.clone(),
        ComponentRef::new(model_id, Some(VERSION)),
        ComponentRef::new(store_id, Some(VERSION)),
    )
    .toolsets(Arc::from([ComponentRef::new(toolset_id, Some(VERSION))]))
    .build()
    .expect("spec");
    let bundle_id = BundleId::parse("test.bundle.preview-tool").expect("bundle id");
    let mut catalog = BundleCatalog::default();
    catalog
        .install(BundleSpec {
            schema_version: BUNDLE_SCHEMA_VERSION,
            id: bundle_id.clone(),
            version: VERSION,
            agents: Arc::from([spec]),
            capabilities: Arc::from([]),
            requirements: Arc::from([]),
            conflicts: Arc::from([]),
            defaults: BundleDefaults::default(),
            config_schema: None,
            compatibility: CompatibilityRequirements::default(),
        })
        .expect("bundle");
    let bundle_resolver = BundleResolver::new(
        &catalog,
        VERSION,
        BTreeSet::new(),
        RuntimeServices::default(),
    );
    let composed_agent = bundle_resolver
        .resolve_agent(
            &mut registry,
            &bundle_id,
            &agent_id,
            BTreeMap::new(),
            AgentConstructionContext::new(),
        )
        .await
        .expect("resolved");
    let output = Agent::try_from_resolved(Arc::new(composed_agent))
        .expect("Agent")
        .run(
            AgentRunRequest::try_new(
                ModelName::try_new("preview-1").expect("model name"),
                "What is two plus three?",
                security(),
            )
            .expect("request"),
        )
        .await
        .expect("run");
    assert_eq!(output.text(), "five");
    assert_eq!(model.request_count(), 2);
    assert_eq!(model.warmup_count(), 1);
    assert_continuation_reaches_second_request(&model);
}

fn observer_descriptor(id: &str) -> ObserverDescriptor {
    ObserverDescriptor {
        component: ComponentRef::new(ComponentId::parse(id).expect("observer id"), Some(VERSION)),
        payload_mode: ObserverPayloadMode::Redacted,
        metadata: Metadata::empty(),
    }
}

async fn run_with_observer(
    observer_id: &str,
    observer: Arc<dyn Observer>,
) -> (AgentRunOutput, finstack_ai_runtime::ObserverDiagnostics) {
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("observer-ok")],
    ));
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let agent = Agent::builder(
        AgentId::parse("test.agent.observer").expect("agent"),
        BundleId::parse("test.bundle.observer").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.preview").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.preview").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .observer(
        ComponentRef::new(
            ComponentId::parse(observer_id).expect("observer component"),
            Some(VERSION),
        ),
        observer,
    )
    .build()
    .await
    .expect("build");
    let run = agent.start(request("observe me")).expect("start");
    let output = run.result().await.expect("run");
    let diagnostics = run.observer_diagnostics().await.expect("diagnostics");
    (output, diagnostics)
}

#[tokio::test]
async fn failing_or_stalled_observer_does_not_change_journal_prefix() {
    let noop: Arc<dyn Observer> =
        Arc::new(NoopObserver::new(observer_descriptor("test.observer.noop")));
    let failing: Arc<dyn Observer> = Arc::new(
        ScriptedObserver::try_new(
            observer_descriptor("test.observer.fail"),
            64,
            vec![ScriptedObserverAction::Return(Err(
                ObserverError::Unavailable,
            ))],
        )
        .expect("failing"),
    );
    let gate = ManualGate::default();
    let stalled: Arc<dyn Observer> = Arc::new(
        ScriptedObserver::try_new(
            observer_descriptor("test.observer.stall"),
            64,
            vec![ScriptedObserverAction::Wait {
                gate,
                outcome: Ok(()),
            }],
        )
        .expect("stalled"),
    );
    let (noop_out, noop_diagnostics) = run_with_observer("test.observer.noop", noop).await;
    let (fail_out, fail_diagnostics) = run_with_observer("test.observer.fail", failing).await;
    let (stall_out, stall_diagnostics) = run_with_observer("test.observer.stall", stalled).await;
    assert_eq!(noop_out.text(), "observer-ok");
    assert_eq!(fail_out.text(), noop_out.text());
    assert_eq!(stall_out.text(), noop_out.text());
    assert_eq!(fail_out.record_kinds(), noop_out.record_kinds());
    assert_eq!(stall_out.record_kinds(), noop_out.record_kinds());
    assert_eq!(noop_diagnostics.total, 0);
    assert_eq!(fail_diagnostics.total, 1);
    assert_eq!(fail_diagnostics.dropped, 0);
    assert_eq!(
        fail_diagnostics.recent[0].code,
        finstack_ai_runtime::OBSERVER_DELIVERY_FAILED
    );
    assert_eq!(stall_diagnostics.total, 1);
    assert_eq!(
        stall_diagnostics.recent[0].code,
        finstack_ai_runtime::OBSERVER_SHUTDOWN_TIMEOUT
    );
}

async fn wait_active_run(lane: &crate::Lane) -> finstack_ai_kernel::RunId {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(inspect) = lane.inspect().await
                && let Some(run_id) = inspect.active_run_id
            {
                return run_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("lane accepted a live run")
}

#[tokio::test]
async fn idle_lane_run_returns_a_live_run() {
    let gate = Arc::<str>::from("lane-live-handle");
    let mut plan = completed("lane ready");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let control = model.control();
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");
    let run = lane.run(&agent, request("Say hello")).expect("run");
    assert_eq!(run.locator().session_id, session.session_id());
    assert_eq!(run.locator().lane_id, lane.lane_id());
    let accepted = wait_active_run(&lane).await;
    assert_eq!(accepted, run.locator().run_id);
    control.release(&gate);
    let output = tokio::time::timeout(Duration::from_secs(3), run.result())
        .await
        .expect("result timeout")
        .expect("result");
    assert_eq!(output.text(), "lane ready");
    assert_eq!(model.request_count(), 1);
}

#[tokio::test]
async fn suspend_parks_without_dropping_the_journal() {
    let gate = Arc::<str>::from("lane-suspend");
    let mut plan = completed("should stay parked");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let control = model.control();
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let session = Session::create(Arc::clone(&store) as _, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");
    let run = lane.run(&agent, request("park me")).expect("run");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model reached gate");
    let run_id = wait_active_run(&lane).await;
    assert_eq!(run_id, run.locator().run_id);
    lane.suspend().await.expect("suspend");
    let inspect = lane.inspect().await.expect("inspect after suspend");
    assert_eq!(inspect.active_run_id, Some(run_id));
    let journal: Arc<dyn JournalStore> = store.clone();
    let loaded = journal
        .load(finstack_ai_runtime::LoadRequest {
            session_id: session.session_id(),
        })
        .await
        .expect("journal load");
    assert!(
        loaded.head_sequence > 0 && !loaded.committed_batches.is_empty(),
        "suspend must keep the accepted journal"
    );
    let reopened = Session::open(
        Arc::clone(&store) as _,
        session.session_id(),
        "tenant-preview",
    )
    .await
    .expect("reopen");
    let restored = reopened
        .lane("main")
        .await
        .expect("restored main")
        .inspect()
        .await
        .expect("restored inspect");
    assert_eq!(restored.active_run_id, Some(run_id));
}

#[tokio::test]
async fn resume_respawns_run_task_owner() {
    let gate = Arc::<str>::from("lane-resume");
    let mut plan = completed("resumed");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let control = model.control();
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");
    let _run = lane.run(&agent, request("resume me")).expect("run");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model reached gate");
    let run_id = wait_active_run(&lane).await;
    lane.suspend().await.expect("suspend");
    assert!(!lane.workflow_owner_is_live());
    let warmups_before_resume = model.warmup_count();
    lane.resume(&agent).await.expect("resume");
    assert_eq!(
        model.warmup_count(),
        warmups_before_resume,
        "resume must reuse the already-warmed model"
    );
    assert!(
        lane.workflow_owner_is_live(),
        "resume must respawn RunTaskOwner"
    );
    let inspect = lane.inspect().await.expect("inspect after resume");
    assert_eq!(inspect.active_run_id, Some(run_id));
}

#[tokio::test]
async fn append_text_does_not_start_a_run() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("unused")],
    ));
    let (_agent, store) = model_only_agent(model).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");
    let entry = lane.append_text("note only").await.expect("append");
    let inspect = lane.inspect().await.expect("inspect");
    assert!(inspect.active_run_id.is_none());
    assert_eq!(inspect.leaf_id, Some(entry));
    assert_eq!(inspect.history.len(), 1);
}

#[tokio::test]
async fn sequential_lane_run_replays_prior_history_once_and_stays_run_scoped() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("first answer"), completed("second answer")],
    ));
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");

    lane.run(&agent, request("first question"))
        .expect("first start")
        .result()
        .await
        .expect("first result");
    lane.run(&agent, request("second question"))
        .expect("second start")
        .result()
        .await
        .expect("second result");

    let sent = model.last_request().expect("second request");
    let transcript = sent
        .draft
        .messages
        .iter()
        .filter(|message| message.role() != finstack_ai_kernel::MessageRole::System)
        .flat_map(|message| {
            message
                .content()
                .iter()
                .filter_map(move |block| match block {
                    ContentBlock::Text(text) => Some((message.role(), text.text().to_owned())),
                    _ => None,
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        transcript,
        vec![
            (
                finstack_ai_kernel::MessageRole::User,
                "first question".into()
            ),
            (
                finstack_ai_kernel::MessageRole::Assistant,
                "first answer".into(),
            ),
            (
                finstack_ai_kernel::MessageRole::User,
                "second question".into(),
            ),
        ]
    );
}

#[tokio::test]
async fn append_text_reports_invalid_message_text() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("unused")],
    ));
    let (_agent, store) = model_only_agent(model).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");
    let oversized = "x".repeat(finstack_ai_kernel::TEXT_MAX_BYTES + 1);
    let error = lane
        .append_text(&oversized)
        .await
        .expect_err("oversized text must fail");
    assert_eq!(error.code(), "invalid_message_text");
}

#[tokio::test]
async fn lane_cancel_uses_explicit_principal_authorization() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::AwaitCancellation],
        }],
    ));
    let (agent, store) = model_only_agent(Arc::clone(&model)).await;
    let session = Session::create(store, "tenant-preview")
        .await
        .expect("session");
    let lane = session.lane("main").await.expect("main");
    let run = lane.run(&agent, request("cancel me")).expect("run starts");
    tokio::time::timeout(Duration::from_secs(3), async {
        while model.request_count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("model reached gate");
    let security = security();
    let authorization = AuthorizationEvidence::try_new(
        security.authorization_policy_version(),
        security.authorization_decision_id(),
    )
    .expect("authorization");
    lane.cancel(security.principal().clone(), authorization)
        .await
        .expect("authenticated cancellation");
    let error = run.result().await.expect_err("run must cancel");
    assert_eq!(error.code(), AGENT_RUN_CANCELLED);
}

async fn journal_kinds(
    store: &Arc<MemoryJournalStore>,
    session_id: finstack_ai_kernel::SessionId,
) -> Vec<String> {
    let loaded = store
        .load(LoadRequest { session_id })
        .await
        .expect("load journal");
    loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(|record| record.body().kind_name().to_string())
        .collect()
}

#[tokio::test]
async fn child_accept_and_cancel_fans_out_against_journal_fixture() {
    let parent_gate = Arc::<str>::from("parent-child-fanout");
    let child_gate = Arc::<str>::from("child-child-fanout");
    let mut parent_plan = completed("parent unused");
    parent_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&parent_gate)));
    let mut child_plan = completed("child unused");
    child_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&child_gate)));
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![parent_plan, child_plan],
    ));
    let control = model.control();
    let (agent, store) = child_capable_agent(Arc::clone(&model)).await;
    let parent = agent.start(request("parent work")).expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&parent_gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent reached gate");
    let child = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &agent,
            request("child work"),
            ChildPlacement::IsolatedChildSession,
            None,
        )),
    )
    .await
    .expect("start_child timeout")
    .expect("start_child");
    assert_ne!(child.locator().run_id, parent.locator().run_id);
    assert_ne!(child.locator().session_id, parent.locator().session_id);
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&child_gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("child reached gate");
    tokio::time::timeout(Duration::from_secs(3), parent.cancel())
        .await
        .expect("parent cancel timeout")
        .expect("parent cancel");
    let child_error = tokio::time::timeout(Duration::from_secs(3), child.result())
        .await
        .expect("child result timeout")
        .expect_err("child must cancel when the parent fans out");
    assert_eq!(child_error.code(), AGENT_RUN_CANCELLED);

    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/compatibility/child-run/v1/valid--accept-cancel-fanout.json"
    ))
    .expect("fixture");
    let mut kinds = journal_kinds(&store, parent.locator().session_id).await;
    kinds.extend(journal_kinds(&store, child.locator().session_id).await);
    for required in fixture["required_kinds"]
        .as_array()
        .expect("required_kinds")
    {
        let kind = required.as_str().expect("kind");
        assert!(
            kinds.iter().any(|actual| actual == kind),
            "journal missing {kind}: {kinds:?}"
        );
    }
    let accepted = kinds.iter().filter(|kind| *kind == "run_accepted").count();
    let cancelled = kinds
        .iter()
        .filter(|kind| *kind == "cancellation_requested")
        .count();
    assert!(
        accepted >= 2,
        "parent and child must both accept: {kinds:?}"
    );
    assert!(
        cancelled >= 2,
        "parent cancel must fan out to the child: {kinds:?}"
    );
}

#[tokio::test]
async fn compatible_start_while_parent_mid_turn_fails_closed() {
    let parent_gate = Arc::<str>::from("parent-compatible-mid-turn");
    let child_gate = Arc::<str>::from("child-isolated-mid-turn");
    let mut parent_plan = completed("parent unused");
    parent_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&parent_gate)));
    let mut child_plan = completed("child unused");
    child_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&child_gate)));
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![parent_plan, child_plan],
    ));
    let control = model.control();
    let (agent, store) = child_capable_agent(Arc::clone(&model)).await;
    let parent = agent.start(request("parent work")).expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&parent_gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent reached gate");
    let isolated = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &agent,
            request("isolated work"),
            ChildPlacement::IsolatedChildSession,
            None,
        )),
    )
    .await
    .expect("isolated timeout")
    .expect("isolated");
    assert_ne!(isolated.locator().session_id, parent.locator().session_id);
    let error = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &agent,
            request("compatible work"),
            ChildPlacement::CompatibleLaneInParentSession,
            None,
        )),
    )
    .await
    .expect("compatible timeout")
    .err()
    .expect("compatible mid-turn must fail closed");
    assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
    let kinds = journal_kinds(&store, parent.locator().session_id).await;
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| *kind == "child_run_prepared")
            .count(),
        1,
        "compatible mid-turn must write no mapping: {kinds:?}"
    );
}

#[tokio::test]
async fn cancel_child_locator_and_journal_recover_do_not_require_live_handles() {
    let parent_gate = Arc::<str>::from("parent-journal-fanout");
    let mut parent_plan = completed("parent unused");
    parent_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&parent_gate)));
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![parent_plan, completed("child done")],
    ));
    let control = model.control();
    let (agent, store) = child_capable_agent(Arc::clone(&model)).await;
    let parent = agent.start(request("parent work")).expect("parent start");
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries(&parent_gate) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent reached gate");
    let child = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &agent,
            request("child work"),
            ChildPlacement::IsolatedChildSession,
            None,
        )),
    )
    .await
    .expect("start_child timeout")
    .expect("start_child");
    tokio::time::timeout(Duration::from_secs(8), child.result())
        .await
        .expect("child timeout")
        .expect("child result");
    parent
        .cancel_child_locator(&ChildRunLocator {
            operation: child.locator().clone(),
            remote: None,
        })
        .await
        .expect("locator cancel after child completed");
    parent
        .recover_children()
        .await
        .expect("recover isolated session");
    parent.clear_child_handles();
    parent
        .recover_children()
        .await
        .expect("recover after dropping live handles");
    tokio::time::timeout(Duration::from_secs(3), parent.cancel())
        .await
        .expect("parent cancel timeout")
        .expect("parent cancel");
    let kinds = journal_kinds(&store, child.locator().session_id).await;
    assert!(
        kinds.iter().any(|kind| kind == "run_accepted"),
        "isolated child mapping remains durable after recover: {kinds:?}"
    );
}

#[tokio::test]
async fn complete_external_routes_a_deferred_parent_effect() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![
            deferred("job-1"),
            completed("child done"),
            completed("unused parent retry"),
        ],
    ));
    let (agent, _store) = child_capable_agent(Arc::clone(&model)).await;
    let parent = agent.start(request("defer me")).expect("parent start");
    let effect_id = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(commit) =
                CommitCoordinator::recover(agent.journal_store(), parent.locator().session_id).await
                && let Some(pending) = commit.state().pending_model_effect.as_ref()
                && let Some(deferred) = pending.deferred.as_ref()
            {
                return deferred.effect_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("deferred effect");
    let child = tokio::time::timeout(
        Duration::from_secs(3),
        Box::pin(parent.start_child(
            &agent,
            request("child work"),
            ChildPlacement::IsolatedChildSession,
            None,
        )),
    )
    .await
    .expect("start_child timeout")
    .expect("start_child");
    let child_out = tokio::time::timeout(Duration::from_secs(8), child.result())
        .await
        .expect("child timeout")
        .expect("child result");
    assert_eq!(child_out.text(), "child done");
    let command = ExternalEffectCompletionCommand::try_new(
        parent.locator().clone(),
        security().principal().clone(),
        AuthorizationEvidence::try_new(
            security().authorization_policy_version(),
            security().authorization_decision_id(),
        )
        .expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            "ext-1",
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    finstack_ai_kernel::ErrorCategory::Model,
                    true,
                )
                .expect("error"),
            },
        )
        .expect("completion"),
    )
    .expect("command");
    let outcome = Box::pin(parent.complete_external(command))
        .await
        .expect("complete_external");
    assert!(
        matches!(
            outcome,
            ExternalRouteOutcome::Committed(_) | ExternalRouteOutcome::Rejected { .. }
        ),
        "external completion must route, not stay data-only"
    );
}

fn research_capability(toolset: ComponentRef) -> CapabilitySpec {
    CapabilitySpec {
        id: CapabilityId::parse("test.capability.research").expect("capability id"),
        description: Arc::from("Research notes"),
        instructions: Arc::from([InstructionSpec::try_new("Research instruction.").expect("text")]),
        toolsets: Arc::from([toolset]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation: CapabilityActivation::Model,
    }
}

#[tokio::test]
async fn restore_after_mid_run_activation_reconstructs_the_same_mask_or_fails_closed() {
    let model: Arc<dyn Model> =
        Arc::new(ScriptedModel::from_plans(profile(), vec![completed("ok")]));
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let calculator: Arc<dyn Toolset> = Arc::new(CalculatorToolset::try_new().expect("calculator"));
    let toolset = ComponentRef::new(
        ComponentId::parse("test.tools.calculator").expect("toolset"),
        Some(VERSION),
    );
    let with_research = Agent::builder(
        AgentId::parse("test.agent.mask-restore").expect("agent"),
        BundleId::parse("test.bundle.mask-restore").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.mask-restore").expect("model"),
                Some(VERSION),
            ),
            Arc::clone(&model),
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.mask-restore").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .capability_toolset(toolset.clone(), calculator)
    .capability(research_capability(toolset))
    .build()
    .await
    .expect("agent with research");
    let activated = [ActiveCapability {
        capability_id: CapabilityId::parse("test.capability.research").expect("id"),
        source: CapabilityActivationSource::Model,
    }];
    with_research
        .validate_restored_mask(&activated)
        .expect("journaled id in lock");
    let live = with_research.live_tool_specs(&activated);
    assert!(
        live.iter()
            .any(|tool| tool.model_name.as_ref() == "calculator")
    );
    let hidden = with_research.live_tool_specs(&[]);
    assert!(
        hidden
            .iter()
            .all(|tool| tool.model_name.as_ref() != "calculator")
    );

    let missing = Agent::builder(
        AgentId::parse("test.agent.mask-missing").expect("agent"),
        BundleId::parse("test.bundle.mask-missing").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.mask-missing").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.mask-missing").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .build()
    .await
    .expect("agent without research");
    let error = missing
        .validate_restored_mask(&activated)
        .expect_err("missing lock member fails closed");
    assert!(error.to_string().contains("capability_mask_not_in_lock"));
}

#[tokio::test]
async fn remote_start_child_without_a_route_fails_closed() {
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![completed("p")]));
    let (agent, _store) = child_capable_agent(model).await;
    let parent = agent.start(request("parent work")).expect("parent start");
    let error = Box::pin(parent.start_child(
        &agent,
        request("child work"),
        ChildPlacement::RemoteChildSession,
        None,
    ))
    .await
    .err()
    .expect("missing route");
    assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
    assert!(error.to_string().contains("explicit route"));
}

struct RecordingEffectInvoker {
    starts: AtomicUsize,
}

impl AgentInvoker for RecordingEffectInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let relation_digest = match child_relation_digest(&context, &request) {
            Ok(digest) => digest,
            Err(error) => {
                return Box::pin(async move {
                    Err(AgentInvokeError::InvalidRequest {
                        message: Arc::from(error.to_string()),
                    })
                });
            }
        };
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

async fn wait_accepted(store: &Arc<dyn JournalStore>, parent: &AgentRun) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(commit) =
                CommitCoordinator::recover(Arc::clone(store), parent.locator().session_id).await
                && commit
                    .state()
                    .accepted
                    .as_ref()
                    .is_some_and(|accepted| accepted.run_id() == parent.locator().run_id)
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent accepted");
}

async fn isolated_child_request(
    store: Arc<dyn JournalStore>,
    parent: &AgentRun,
) -> ChildRunRequest {
    let session = Session::create(store, Arc::clone(&parent.locator().tenant_scope))
        .await
        .expect("child session");
    let lane = session.lane("main").await.expect("child lane");
    let run_id = super::prepare::NativeIds::generate::<RunTag>().expect("child run");
    let locator = ChildRunLocator {
        operation: OperationLocator::try_new(
            parent.locator().tenant_scope.as_ref(),
            session.session_id(),
            lane.lane_id(),
            run_id,
        )
        .expect("child locator"),
        remote: None,
    };
    ChildRunRequest {
        agent: AgentRef {
            id: AgentId::parse("finstack.agent.child").expect("agent"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
        },
        input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("work").expect("text"),
        )]),
        placement: ChildPlacement::IsolatedChildSession,
        locator,
        requested_deadline: None,
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    }
}

fn echo_tool_spec() -> ToolSpec {
    ToolSpec {
        id: ToolId::parse("finstack.tools.echo").expect("tool id"),
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
    }
}

fn echo_tool_call() -> ScriptedModelPlan {
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("echo")),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: Some(Arc::from("call-echo")),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                assistant_content: Arc::from([]),
                tool_calls: Arc::from([ModelToolCall {
                    name: Arc::from("echo"),
                    arguments,
                    provider_call_id: Some(Arc::from("call-echo")),
                }]),
                usage: Usage::empty(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from("echo-tool-completion"),
                continuation_state: None,
            }))),
        ],
    }
}

async fn wait_deferred_tool_effect(store: &Arc<dyn JournalStore>, parent: &AgentRun) -> EffectId {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(commit) =
                CommitCoordinator::recover(Arc::clone(store), parent.locator().session_id).await
                && commit.state().phase == Some(RunPhase::AwaitingExternal)
                && let Some(batch) = commit.state().active_tool_batch.as_ref()
                && let Some(call) = batch.calls.first()
                && matches!(
                    &call.status,
                    ActiveToolCallStatus::Requested {
                        deferred: Some(_),
                        ..
                    }
                )
            {
                return call.assigned.effect_id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("deferred tool effect")
}

#[tokio::test]
async fn start_or_attach_child_is_idempotent_for_an_equal_request() {
    let gate = Arc::<str>::from("child-idempotency-parent");
    let mut plan = completed("parent");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let control = model.control();
    let (agent, store) = child_capable_agent(model).await;
    let store: Arc<dyn JournalStore> = store;
    let parent = agent.start(request("parent work")).expect("parent start");
    wait_accepted(&store, &parent).await;
    let effect_id = super::prepare::NativeIds::generate::<EffectTag>().expect("effect");
    let child_request = isolated_child_request(Arc::clone(&store), &parent).await;
    let invoker = Arc::new(RecordingEffectInvoker {
        starts: AtomicUsize::new(0),
    });
    let first = parent
        .start_or_attach_child(
            Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
            effect_id,
            child_request.clone(),
        )
        .await
        .expect("first start_or_attach_child");
    let attached = parent
        .start_or_attach_child(
            Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
            effect_id,
            child_request,
        )
        .await
        .expect("equal retry");
    assert_eq!(first, attached);
    assert_eq!(invoker.starts.load(Ordering::SeqCst), 2);
    control.release(&gate);
    parent.result().await.expect("parent completes");
}

#[tokio::test]
async fn start_or_attach_child_rejects_a_conflicting_digest() {
    let gate = Arc::<str>::from("child-conflict-parent");
    let mut plan = completed("parent");
    plan.actions
        .insert(0, ScriptedModelAction::Block(Arc::clone(&gate)));
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let control = model.control();
    let (agent, store) = child_capable_agent(model).await;
    let store: Arc<dyn JournalStore> = store;
    let parent = agent.start(request("parent work")).expect("parent start");
    wait_accepted(&store, &parent).await;
    let effect_id = super::prepare::NativeIds::generate::<EffectTag>().expect("effect");
    let child_request = isolated_child_request(Arc::clone(&store), &parent).await;
    let invoker = Arc::new(RecordingEffectInvoker {
        starts: AtomicUsize::new(0),
    });
    parent
        .start_or_attach_child(
            Arc::clone(&invoker) as Arc<dyn AgentInvoker>,
            effect_id,
            child_request.clone(),
        )
        .await
        .expect("first start_or_attach_child");
    let mut conflicting = child_request;
    conflicting.request_digest = Digest::raw_json(br#"{"request":"child-b"}"#);
    let error = parent
        .start_or_attach_child(invoker, effect_id, conflicting)
        .await
        .expect_err("conflicting digest");
    assert_eq!(error.code(), AGENT_RUN_RUNTIME_FAILURE);
    assert!(
        error.to_string().contains("sidecar conflicts"),
        "conflicting digest must fail closed: {error}"
    );
    control.release(&gate);
    parent.result().await.expect("parent completes");
}

#[tokio::test]
async fn complete_external_routes_a_deferred_tool_effect() {
    let mut spec = echo_tool_spec();
    spec.deferral = ToolDeferralSupport::Supported;
    let toolset: Arc<dyn Toolset> = Arc::new(ScriptedToolset::new(
        Arc::from([spec]),
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
                    reconciliation: ReconciliationPolicy::CallbackOrPoll,
                    next_poll_at: None,
                    expires_at: None,
                },
            )))],
        }],
    ));
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![echo_tool_call(), completed("unused parent retry")],
    ));
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let agent = Agent::builder(
        AgentId::parse("test.agent.tool-defer").expect("agent"),
        BundleId::parse("test.bundle.tool-defer").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.tool-defer").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.tool-defer").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store),
        ),
    )
    .toolset(
        ComponentRef::new(
            ComponentId::parse("test.tools.echo").expect("toolset"),
            Some(VERSION),
        ),
        toolset,
    )
    .build()
    .await
    .expect("agent");
    let parent = agent
        .start(request("defer the tool"))
        .expect("parent start");
    let effect_id = wait_deferred_tool_effect(&store, &parent).await;
    let command = ExternalEffectCompletionCommand::try_new(
        parent.locator().clone(),
        security().principal().clone(),
        AuthorizationEvidence::try_new(
            security().authorization_policy_version(),
            security().authorization_decision_id(),
        )
        .expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            "ext-tool-1",
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    finstack_ai_kernel::ErrorCategory::Model,
                    true,
                )
                .expect("error"),
            },
        )
        .expect("completion"),
    )
    .expect("command");
    let outcome = Box::pin(parent.complete_external(command))
        .await
        .expect("complete_external");
    assert!(
        matches!(
            outcome,
            ExternalRouteOutcome::Committed(_) | ExternalRouteOutcome::Rejected { .. }
        ),
        "external completion must route a deferred tool effect"
    );
}

/// `before_finalize` middleware that supersedes the first finalize candidate
/// with a framework-classified retry, then lets every subsequent candidate through.
struct FinalizeVerifierRetry {
    fired: AtomicBool,
}

impl FinalizeVerifierRetry {
    fn new() -> Self {
        Self {
            fired: AtomicBool::new(false),
        }
    }
}

impl Middleware for FinalizeVerifierRetry {
    fn descriptor(&self) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("test.middleware.finalize-verification-retry")
                    .expect("component id"),
                version: VERSION,
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::RecomputeSafe,
            },
            stages: StageMask::from_stages([Stage::BeforeFinalize]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: Metadata::empty(),
        }
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let already_fired = self.fired.swap(true, Ordering::AcqRel);
        Box::pin(async move {
            if already_fired {
                return Ok(StageOutcome::Continue);
            }
            let directive = RetryDirective::try_new(
                RetryClassification::Framework,
                finstack_ai_kernel::Duration::from_millis(1),
                "test-finalize-verification-retry-v1",
            )
            .expect("retry directive");
            Ok(StageOutcome::Retry(directive))
        })
    }
}

#[tokio::test]
async fn drive_loop_continues_past_a_superseded_finalize() {
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![
            completed_with_id("first answer", "finalize-retry-completion-1"),
            completed_with_id("second answer", "finalize-retry-completion-2"),
        ],
    ));
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let middleware: Arc<dyn Middleware> = Arc::new(FinalizeVerifierRetry::new());
    let agent = Agent::builder(
        AgentId::parse("test.agent.finalize-retry").expect("agent"),
        BundleId::parse("test.bundle.finalize-retry").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.finalize-retry").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.finalize-retry").expect("store"),
                Some(VERSION),
            ),
            Arc::clone(&store) as Arc<dyn JournalStore>,
        ),
    )
    .middleware(
        ComponentRef::new(
            ComponentId::parse("test.middleware.finalize-verification-retry").expect("component"),
            Some(VERSION),
        ),
        middleware,
    )
    .build()
    .await
    .expect("agent");
    let output = agent
        .run(request("answer twice"))
        .await
        .expect("run survives a superseded finalize");
    assert_eq!(output.text(), "second answer");

    let loaded = store
        .load(LoadRequest {
            session_id: output.locator.session_id,
        })
        .await
        .expect("load journal");
    let verifier_retries = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .filter(|record| {
            matches!(
                record.body(),
                RecordBody::RetryScheduled(retry)
                    if retry.classification == RetryClassification::Framework
            )
        })
        .count();
    assert_eq!(
        verifier_retries, 1,
        "journal must show exactly one framework verifier retry"
    );
}
