//! Durable host admission, reconstruction and worker/SDK integration.
use super::*;
use crate::durable::{DurableHost, DurableHostBuilder};
use finstack_ai_runtime::ports::journal::StoreLimits;

fn limits() -> StoreLimits {
    StoreLimits {
        sessions: 16,
        batches_per_session: 256,
        records_per_session: 2048,
        snapshot_bytes: 64 * 1024,
    }
}

#[tokio::test]
async fn durable_recovery_preserves_the_original_deadline_and_expires_before_dispatch() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("deadline.sqlite");
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![]));
    let original = host(&path, Arc::clone(&model)).await;
    let mut input = request("deadline input");
    input.timeout = Duration::from_millis(10);
    let locator = original.start("assistant", input).await.expect("start");
    let accepted = original
        .inspect(&locator)
        .await
        .expect("inspect")
        .state
        .accepted()
        .expect("accepted")
        .clone();
    original.shutdown().await;
    let deadline = accepted.effective_deadline().expect("deadline");
    let now = super::super::prepare::NativeIds::now().expect("now");
    let remaining = deadline
        .as_unix_ms()
        .saturating_sub(now.as_unix_ms())
        .max(0);
    tokio::time::sleep(Duration::from_millis(
        u64::try_from(remaining).expect("positive") + 2,
    ))
    .await;
    let recovered = host(&path, Arc::clone(&model)).await;
    assert_eq!(
        recovered
            .inspect(&locator)
            .await
            .expect("inspect")
            .state
            .accepted(),
        Some(&accepted)
    );
    let tick = recovered.tick().await.expect("expiry tick");
    let inspection = recovered.inspect(&locator).await.expect("terminal");
    assert_eq!(
        tick.failures,
        0,
        "{tick:?} phase={:?} terminal={:?}",
        inspection.state.phase(),
        inspection.state.terminal()
    );
    assert!(matches!(
        inspection.state.terminal(),
        Some(TerminalState::Cancelled(_))
    ));
    assert_eq!(
        inspection
            .state
            .cancellation()
            .expect("deadline cancellation")
            .request
            .initiator,
        finstack_ai_kernel::CancellationInitiator::Deadline
    );
    assert_eq!(inspection.state.accepted(), Some(&accepted));
    assert_eq!(model.request_count(), 0);
    recovered.shutdown().await;
}

async fn capability_definition() -> (Agent, Arc<NativeCapabilityHost>) {
    use finstack_ai_tools_skills::{SkillsHost, SkillsHostError, SkillsToolset};

    let mut activate = echo_tool_call();
    for action in &mut activate.actions {
        if let ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(delta))) = action {
            delta.name = Some(Arc::from("capability_activate"));
            delta.arguments_delta = Arc::from(r#"{"id":"test.capability.research"}"#);
            delta.provider_call_id = Some(Arc::from("activate"));
        }
        if let ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))) = action {
            response.tool_calls = Arc::from([ModelToolCall {
                name: Arc::from("capability_activate"),
                arguments: RawJson::parse(br#"{"id":"test.capability.research"}"#).expect("json"),
                provider_call_id: Some(Arc::from("activate")),
            }]);
        }
    }
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![activate, completed("activated")],
    ));
    let proposal_host = Arc::new(NativeCapabilityHost::new(
        "test.capability.research: Research",
    ));
    let active = Arc::clone(&proposal_host);
    let activate = Arc::clone(&proposal_host);
    let skills = SkillsToolset::try_new(SkillsHost {
        catalog: proposal_host.compact_catalog().to_owned(),
        active: Arc::new(move |run| active.active(run).map_err(|_| SkillsHostError::Bound)),
        activate: Arc::new(move |run, complete| {
            activate
                .queue_activation(run, complete)
                .map_err(|_| SkillsHostError::Bound)
        }),
    })
    .expect("skills");
    let base = definition(model).await;
    let agent = base
        .rebuild
        .as_ref()
        .expect("builder")
        .as_ref()
        .clone()
        .capability_activation_host(Arc::clone(&proposal_host))
        .toolset(
            ComponentRef::new(
                ComponentId::parse("test.tools.skills").expect("id"),
                Some(VERSION),
            ),
            Arc::new(skills),
        )
        .capability(CapabilitySpec {
            id: CapabilityId::parse("test.capability.research").expect("id"),
            description: Arc::from("Research"),
            instructions: Arc::from([
                InstructionSpec::try_new("Research instruction.").expect("instruction")
            ]),
            toolsets: Arc::from([]),
            context_providers: Arc::from([]),
            middleware: Arc::from([]),
            activation: CapabilityActivation::Model,
        })
        .build()
        .await
        .expect("agent");
    (agent, proposal_host)
}

#[tokio::test]
async fn durable_capability_prefix_requires_a_committed_mask_or_live_proposal() {
    use super::super::durable::capability::validate_activation_prefix;

    let (agent, proposal_host) = capability_definition().await;
    let dir = tempfile::tempdir().expect("dir");
    let builder =
        DurableHostBuilder::try_open("tenant-preview", dir.path().join("caps.sqlite"), limits())
            .expect("open");
    let journal = builder.journal_store();
    let host = builder
        .register("assistant", agent)
        .await
        .expect("register")
        .build()
        .expect("host");
    let locator = host
        .start("assistant", request("activate research"))
        .await
        .expect("start");
    assert_eq!(host.tick().await.expect("tick").failures, 0);
    let loaded = journal
        .load(LoadRequest {
            session_id: locator.session_id,
        })
        .await
        .expect("load");
    let records = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .collect::<Vec<_>>();
    let committed = records
        .iter()
        .position(|record| matches!(record.body(), RecordBody::CapabilitiesActivated(_)))
        .expect("activation committed");
    let fresh = NativeCapabilityHost::new(proposal_host.compact_catalog());
    let missing = validate_activation_prefix(
        Some(&fresh),
        locator.run_id,
        records[..committed].iter().copied(),
    )
    .expect_err("proposal lost at crash boundary");
    assert_eq!(missing.code.as_ref(), "durable_capability_uncertain");
    validate_activation_prefix(Some(&fresh), locator.run_id, records.iter().copied())
        .expect("committed mask recovered");
    fresh
        .queue_activation(
            locator.run_id,
            vec![ActiveCapability {
                capability_id: CapabilityId::parse("test.capability.research").expect("id"),
                source: CapabilityActivationSource::Model,
            }],
        )
        .expect("live proposal");
    validate_activation_prefix(
        Some(&fresh),
        locator.run_id,
        records[..committed].iter().copied(),
    )
    .expect("live proposal remains usable");
    host.shutdown().await;
}

async fn definition(model: Arc<ScriptedModel>) -> Agent {
    let journal = Arc::new(MemoryJournalStore::try_new(limits()).expect("memory"));
    Agent::builder(
        AgentId::parse("test.host.agent").expect("id"),
        BundleId::parse("test.host.bundle").expect("id"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.preview").expect("id"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.preview").expect("id"),
                Some(VERSION),
            ),
            journal,
        ),
    )
    .build()
    .await
    .expect("definition")
}

async fn host(path: &std::path::Path, model: Arc<ScriptedModel>) -> DurableHost {
    let agent = definition(model).await;
    DurableHostBuilder::try_open("tenant-preview", path, limits())
        .expect("open")
        .register("assistant", agent)
        .await
        .expect("register")
        .build()
        .expect("host")
}

#[tokio::test]
async fn durable_structured_schema_survives_rebinding_and_rejects_recovery_drift() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("structured.sqlite");
    let schema = RawJson::parse(br#"{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false}"#).expect("schema");
    let original = definition(Arc::new(ScriptedModel::from_plans(profile(), vec![])))
        .await
        .try_with_output_schema(&schema)
        .expect("schema config");
    let tools = original.live_tool_specs(&[]);
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools[0].model_name.as_ref(),
        finstack_ai_kernel::SUBMIT_FINAL_OUTPUT_TOOL
    );
    assert_eq!(tools[0].input_schema, schema);
    let original = DurableHostBuilder::try_open("tenant-preview", &path, limits())
        .expect("open")
        .register("assistant", original)
        .await
        .expect("register")
        .build()
        .expect("host");
    let locator = original
        .start("assistant", request("answer"))
        .await
        .expect("start");
    original.shutdown().await;
    drop(original);
    let drifted = host(
        &path,
        Arc::new(ScriptedModel::from_plans(profile(), vec![])),
    )
    .await;
    assert_eq!(
        drifted
            .inspect(&locator)
            .await
            .expect_err("schema drift")
            .code
            .as_ref(),
        "durable_configuration_drift"
    );
    drifted.shutdown().await;
    drop(drifted);
    let mut plan = completed("unused");
    plan.actions.remove(0);
    if let ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))) =
        &mut plan.actions[0]
    {
        response.assistant_content = Arc::from([ContentBlock::Json(
            finstack_ai_kernel::JsonBlock::new(RawJson::parse(br#"{"answer":7}"#).expect("json")),
        )]);
    }
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![plan]));
    let agent = definition(Arc::clone(&model))
        .await
        .try_with_output_schema(&schema)
        .expect("schema");
    let recovered = DurableHostBuilder::try_open("tenant-preview", &path, limits())
        .expect("open")
        .register("assistant", agent)
        .await
        .expect("register")
        .build()
        .expect("host");
    assert_eq!(recovered.tick().await.expect("tick").failures, 0);
    let inspection = recovered.inspect(&locator).await.expect("inspect");
    assert!(matches!(
        inspection.state.terminal(),
        Some(TerminalState::Completed(_))
    ));
    assert_eq!(model.request_count(), 1);
    recovered.shutdown().await;
}

#[tokio::test]
async fn durable_admission_dispatches_only_on_tick_and_reopens_without_input() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("host.sqlite");
    let first = Arc::new(ScriptedModel::from_plans(profile(), vec![]));
    let original = host(&path, Arc::clone(&first)).await;
    let locator = original
        .start("assistant", request("original input"))
        .await
        .expect("admit");
    assert_eq!(first.request_count(), 0);
    assert_eq!(
        original
            .inspect(&locator)
            .await
            .expect("inspect")
            .state
            .phase(),
        Some(RunPhase::BeforeRun)
    );
    original.shutdown().await;
    drop(original);
    let second = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("recovered reply")],
    ));
    let recovered = host(&path, Arc::clone(&second)).await;
    let tick = recovered.tick().await.expect("tick");
    assert_eq!(tick.failures, 0);
    assert_eq!(tick.sessions_resumed, 1);
    let inspected = recovered.inspect(&locator).await.expect("inspect");
    assert!(matches!(
        inspected.state.terminal(),
        Some(TerminalState::Completed(_))
    ));
    assert_eq!(second.request_count(), 1);
    assert_eq!(recovered.tick().await.expect("repeat").sessions_resumed, 0);
    assert_eq!(second.request_count(), 1);
    recovered.shutdown().await;
    assert_eq!(
        recovered.tick().await.expect_err("closed").code.as_ref(),
        "durable_host_closed"
    );
}

#[tokio::test]
async fn durable_reconstruction_rejects_changed_definition_and_wrong_tenant() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("host.sqlite");
    let model = Arc::new(ScriptedModel::from_plans(profile(), vec![]));
    let original = host(&path, Arc::clone(&model)).await;
    let locator = original
        .start("assistant", request("input"))
        .await
        .expect("admit");
    original.shutdown().await;
    drop(original);
    let agent = definition(Arc::clone(&model)).await;
    let changed = agent
        .rebuild
        .as_ref()
        .expect("builder")
        .as_ref()
        .clone()
        .try_instruction("changed definition")
        .expect("instruction")
        .build()
        .await
        .expect("agent");
    let recovered = DurableHostBuilder::try_open("tenant-preview", &path, limits())
        .expect("open")
        .register("assistant", changed)
        .await
        .expect("register")
        .build()
        .expect("host");
    assert_eq!(
        recovered.tick().await.expect_err("drift").code.as_ref(),
        "durable_configuration_drift"
    );
    assert_eq!(model.request_count(), 0);
    let mut wrong = locator;
    wrong.tenant_scope = Arc::from("tenant-b");
    assert_eq!(
        recovered
            .inspect(&wrong)
            .await
            .expect_err("tenant")
            .code
            .as_ref(),
        "durable_tenant_mismatch"
    );
}

async fn approval_definition(model: Arc<ScriptedModel>) -> (Agent, Arc<ScriptedToolset>) {
    let mut spec = echo_tool_spec();
    spec.approval.requirement = ApprovalRequirement::Required;
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::from([spec]),
        vec![ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(
                finstack_ai_runtime::ports::tool::ToolResult {
                    output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("json"),
                    is_error: false,
                },
            )))],
        }],
    ));
    let base = definition(model).await;
    let agent = base
        .rebuild
        .as_ref()
        .expect("builder")
        .as_ref()
        .clone()
        .toolset(
            ComponentRef::new(
                ComponentId::parse("test.tools.echo").expect("component"),
                Some(VERSION),
            ),
            Arc::clone(&toolset) as Arc<dyn Toolset>,
        )
        .build()
        .await
        .expect("agent");
    (agent, toolset)
}

struct DurableRetry;

impl Middleware for DurableRetry {
    fn descriptor(&self) -> MiddlewareDescriptor {
        FinalizeVerifierRetry::new().descriptor()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let retry = matches!(input, StageInput::BeforeFinalize { result_message: Some(message), .. }
            if message.as_str().contains("retry me"));
        Box::pin(async move {
            Ok(if retry {
                StageOutcome::Retry(
                    RetryDirective::try_new(
                        RetryClassification::Framework,
                        finstack_ai_kernel::Duration::from_millis(100),
                        "durable-retry-v1",
                    )
                    .expect("retry"),
                )
            } else {
                StageOutcome::Continue
            })
        })
    }
}

async fn retry_host(path: &std::path::Path, model: Arc<ScriptedModel>) -> DurableHost {
    let base = definition(model).await;
    let agent = base
        .rebuild
        .as_ref()
        .expect("builder")
        .as_ref()
        .clone()
        .middleware(Arc::new(DurableRetry))
        .build()
        .await
        .expect("agent");
    DurableHostBuilder::try_open("tenant-preview", path, limits())
        .expect("open")
        .register("assistant", agent)
        .await
        .expect("register")
        .build()
        .expect("host")
}

#[tokio::test]
async fn durable_retry_timer_reopens_at_original_due_time() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("timer.sqlite");
    let first = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_with_id("retry me", "retry-first")],
    ));
    let original = retry_host(&path, Arc::clone(&first)).await;
    let locator = original
        .start("assistant", request("answer"))
        .await
        .expect("start");
    assert_eq!(original.tick().await.expect("park").sessions_reparked, 1);
    let wait = original
        .inspect(&locator)
        .await
        .expect("wait")
        .wait
        .expect("timer");
    let finstack_ai_runtime::workflow::WorkflowWait::Timer { due_at, .. } = wait else {
        panic!("expected timer")
    };
    original.shutdown().await;
    let now = super::super::prepare::NativeIds::now().expect("now");
    let remaining = due_at.as_unix_ms().saturating_sub(now.as_unix_ms()).max(0);
    tokio::time::sleep(Duration::from_millis(
        u64::try_from(remaining).expect("positive") + 2,
    ))
    .await;
    let second = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed_with_id("final reply", "retry-second")],
    ));
    let recovered = retry_host(&path, Arc::clone(&second)).await;
    assert_eq!(
        recovered
            .inspect(&locator)
            .await
            .expect("original wait")
            .wait,
        Some(wait)
    );
    let tick = recovered.tick().await.expect("resume");
    assert_eq!(tick.failures, 0);
    assert_eq!(tick.sessions_reparked, 0);
    assert!(matches!(
        recovered
            .inspect(&locator)
            .await
            .expect("completed")
            .state
            .terminal(),
        Some(TerminalState::Completed(_))
    ));
    assert_eq!(first.request_count(), 1);
    assert_eq!(second.request_count(), 1);
    recovered.shutdown().await;
}

#[tokio::test]
async fn durable_approval_reopens_and_finishes_subsequent_model_cycle() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("approval.sqlite");
    let first = Arc::new(ScriptedModel::from_plans(profile(), vec![echo_tool_call()]));
    let (agent, first_tool) = approval_definition(Arc::clone(&first)).await;
    let original = DurableHostBuilder::try_open("tenant-preview", &path, limits())
        .expect("open")
        .register("approval", agent)
        .await
        .expect("register")
        .build()
        .expect("host");
    let locator = original
        .start("approval", request("use the approved echo tool"))
        .await
        .expect("start");
    let report = original.tick().await.expect("park tick");
    assert_eq!(report.failures, 0);
    assert_eq!(report.sessions_reparked, 1);
    assert_eq!(first.request_count(), 1);
    assert_eq!(first_tool.call_count(), 0);
    assert_eq!(original.pending().expect("pending").len(), 1);
    original.shutdown().await;
    drop(original);
    drop(first_tool);
    drop(first);
    let second = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("approved and recovered")],
    ));
    let (agent, second_tool) = approval_definition(Arc::clone(&second)).await;
    let recovered = DurableHostBuilder::try_open("tenant-preview", &path, limits())
        .expect("open")
        .register("approval", agent)
        .await
        .expect("register")
        .build()
        .expect("host");
    let row = recovered.pending().expect("pending").remove(0);
    recovered
        .resolve(
            &row.interaction_id,
            crate::durable::ResolutionInput {
                resolution_id: Arc::from("decision-1"),
                principal: security().principal().clone(),
                evidence: AuthorizationEvidence::try_new(
                    security().authorization_policy_version(),
                    security().authorization_decision_id(),
                )
                .expect("evidence"),
                payload: RawJson::parse(br#"{"approved":true}"#).expect("json"),
                note: None,
            },
        )
        .expect("resolve");
    let report = recovered.tick().await.expect("resume tick");
    assert_eq!(report.failures, 0);
    assert_eq!(report.sessions_resumed, 1);
    let inspected = recovered.inspect(&locator).await.expect("inspect");
    assert!(matches!(
        inspected.state.terminal(),
        Some(TerminalState::Completed(_))
    ));
    assert_eq!(second_tool.call_count(), 1);
    assert_eq!(second.request_count(), 1);
    assert!(recovered.pending().expect("pending").is_empty());
    let model_request = second.last_request().expect("resumed request");
    let original_inputs = model_request
        .draft
        .messages
        .iter()
        .filter(|message| message.role() == finstack_ai_kernel::MessageRole::User)
        .count();
    assert_eq!(
        original_inputs, 1,
        "recovery must not append the original input twice"
    );
    assert_eq!(
        recovered.tick().await.expect("idempotent").sessions_resumed,
        0
    );
    assert_eq!(second_tool.call_count(), 1);
}
