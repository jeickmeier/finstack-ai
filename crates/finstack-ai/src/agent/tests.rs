use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AgentId, AuthorizationEvidence, BundleId, CapabilityId, ChildPlacement, ComponentId,
    ComponentRef, ContentBlock, ExternalEffectCompletion, ExternalEffectCompletionCommand,
    ExternalEffectOutcome, ProviderIds, RawJson, RunEventClass, RunSecurityContext, TerminalState,
    TextBlock, Usage, Version,
};
use finstack_ai_runtime::{
    CommitCoordinator, ExternalHandleRef, ExternalRouteOutcome, JournalStore, LoadRequest,
    Metadata, Model, ModelContextProfile, ModelDeferral, ModelName, ModelResponse, ModelStreamItem,
    ModelToolCall, NoopObserver, Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode,
    ReconciliationPolicy, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, Toolset,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ManualGate, ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedObserver,
    ScriptedObserverAction,
};
use finstack_ai_tools_calculator::CalculatorToolset;

use super::builder::validate_compact_catalog;
use super::types::MAX_COMPACT_CATALOG_BYTES;
use super::*;
use crate::{
    AgentBuilder, AgentConstructionContext, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleDefaults,
    BundleResolver, BundleSpec, CapabilityActivation, CapabilitySpec, ChildRunPolicy,
    CompatibilityRequirements, Extension, ExtensionDescriptor, ReadyComponent, Registrar,
    RegistrationError, RegistrationMetadata, RunPolicy, RuntimeServices, Session,
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
                completion_id: Arc::from("preview-completion"),
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

async fn run_with_observer(observer_id: &str, observer: Arc<dyn Observer>) -> AgentRunOutput {
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
    Agent::builder(
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
    .expect("build")
    .run(request("observe me"))
    .await
    .expect("run")
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
    let noop_out = run_with_observer("test.observer.noop", noop).await;
    let fail_out = run_with_observer("test.observer.fail", failing).await;
    let stall_out = run_with_observer("test.observer.stall", stalled).await;
    assert_eq!(noop_out.text(), "observer-ok");
    assert_eq!(fail_out.text(), noop_out.text());
    assert_eq!(stall_out.text(), noop_out.text());
    assert_eq!(fail_out.record_kinds(), noop_out.record_kinds());
    assert_eq!(stall_out.record_kinds(), noop_out.record_kinds());
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
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("lane ready")],
    ));
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
    lane.resume(&agent).await.expect("resume");
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
