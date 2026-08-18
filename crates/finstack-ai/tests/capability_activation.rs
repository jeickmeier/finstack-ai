//! FR-06 lock-time union, dispatch mask, and mid-run activation.

use std::sync::Arc;

use finstack_ai::runtime::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentInvocation, ComponentRef,
    ContextContribution, ContextProvider, ContextProviderDescriptor, Digest, InvocationRecovery,
    JournalStore, Middleware, MiddlewareDescriptor, MiddlewareOrder, MiddlewareRole, Model,
    ModelContextProfile, ModelName, ModelStreamItem, ModelToolCall, OrderTier, Stage, StageMask,
    StageOutcome, TextDelta, TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta, Version,
};
use finstack_ai::{
    ActivationHostError, Agent, AgentRunRequest, CAPABILITY_ACTIVATION_BOUND, CapabilityActivation,
    CapabilitySpec, InstructionSpec, MAX_CONCURRENT_CAPABILITY_ACTIVATIONS, NativeCapabilityHost,
    PrincipalRef, RunSecurityContext,
};
use finstack_ai_kernel::{ActiveCapability, CapabilityActivationSource, RunId};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ScriptedContextAction, ScriptedContextProvider, ScriptedMiddleware, ScriptedMiddlewareAction,
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan,
};
use finstack_ai_tools_calculator::CalculatorToolset;
use finstack_ai_tools_skills::{SkillsHost, SkillsHostError, SkillsToolset};

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("preview-1").expect("model"),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 8_192,
        max_output_tokens: 512,
        reserved_output_tokens: 128,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: Arc::from("scripted.utf8"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn completed(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
                finstack_ai::runtime::ModelResponse {
                    assistant_content: Arc::from([finstack_ai::runtime::ContentBlock::Text(
                        finstack_ai::runtime::TextBlock::try_new(text).expect("text"),
                    )]),
                    tool_calls: Arc::from([]),
                    usage: finstack_ai::runtime::Usage::empty(),
                    provider_ids: finstack_ai::runtime::ProviderIds::empty(),
                    completion_id: Arc::from("capability-activation-1"),
                    continuation_state: None,
                },
            ))),
        ],
    }
}

fn activate_call(id: &str) -> ScriptedModelPlan {
    let arguments = finstack_ai::runtime::RawJson::parse(
        serde_json::to_vec(&serde_json::json!({ "id": id })).expect("arguments"),
    )
    .expect("raw");
    let response = finstack_ai::runtime::ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: Arc::from([ModelToolCall {
            name: Arc::from("capability_activate"),
            arguments: arguments.clone(),
            provider_call_id: Some(Arc::from("call-activate")),
        }]),
        usage: finstack_ai::runtime::Usage::empty(),
        provider_ids: finstack_ai::runtime::ProviderIds::empty(),
        completion_id: Arc::from("capability-activate-completion"),
        continuation_state: None,
    };
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("capability_activate")),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: Some(Arc::from("call-activate")),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response))),
        ],
    }
}

fn security() -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-activation",
        PrincipalRef::try_new("issuer", "subject", Some("tenant-activation")).expect("principal"),
        "local",
        "test",
        "activation-policy-v1",
        "activation-decision-v1",
        None,
    )
    .expect("security")
}

fn request(input: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("preview-1").expect("model name"),
        input,
        security(),
    )
    .expect("request")
}

fn store() -> Arc<dyn JournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("store"),
    )
}

fn component(id: &str) -> ComponentRef {
    ComponentRef::new(ComponentId::parse(id).expect("component"), Some(VERSION))
}

fn invocation(id: &str) -> ComponentInvocation {
    ComponentInvocation {
        component: ComponentId::parse(id).expect("component"),
        version: VERSION,
        configuration_digest: Digest::raw_json(b"{}"),
        recovery: InvocationRecovery::RecomputeSafe,
    }
}

fn compactor(id: &str) -> Arc<dyn Middleware> {
    Arc::new(ScriptedMiddleware::new(
        MiddlewareDescriptor {
            invocation: invocation(id),
            stages: StageMask::from_stages([Stage::BeforeModel]),
            order: MiddlewareOrder {
                tier: OrderTier::ContextCompaction,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::ContextCompactor {
                strategy_id: Arc::from("fixture.window"),
                strategy_version: 1,
            },
            metadata: finstack_ai::runtime::Metadata::empty(),
        },
        vec![ScriptedMiddlewareAction::Return(Ok(StageOutcome::Continue))],
    ))
}

fn standard_middleware(id: &str) -> Arc<dyn Middleware> {
    Arc::new(ScriptedMiddleware::new(
        MiddlewareDescriptor {
            invocation: invocation(id),
            stages: StageMask::from_stages([Stage::BeforeRun]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: finstack_ai::runtime::Metadata::empty(),
        },
        vec![ScriptedMiddlewareAction::Return(Ok(StageOutcome::Continue))],
    ))
}

fn capability(
    id: &str,
    description: &str,
    instruction: &str,
    activation: CapabilityActivation,
    toolsets: Vec<ComponentRef>,
    context_providers: Vec<ComponentRef>,
    middleware: Vec<ComponentRef>,
) -> CapabilitySpec {
    CapabilitySpec {
        id: CapabilityId::parse(id).expect("capability id"),
        description: Arc::from(description),
        instructions: Arc::from([InstructionSpec::try_new(instruction).expect("instruction")]),
        toolsets: toolsets.into(),
        context_providers: context_providers.into(),
        middleware: middleware.into(),
        activation,
    }
}

fn skills_host(host: &Arc<NativeCapabilityHost>) -> SkillsHost {
    let catalog = host.compact_catalog().to_owned();
    let active = Arc::clone(host);
    let activate = Arc::clone(host);
    SkillsHost {
        catalog,
        active: Arc::new(move |run| match active.active(run) {
            Ok(set) => Ok(set),
            Err(ActivationHostError::Bound) => Err(SkillsHostError::Bound),
            Err(ActivationHostError::Failed { reason }) => Err(SkillsHostError::Failed { reason }),
        }),
        activate: Arc::new(
            move |run, complete| match activate.queue_activation(run, complete) {
                Ok(()) => Ok(()),
                Err(ActivationHostError::Bound) => Err(SkillsHostError::Bound),
                Err(ActivationHostError::Failed { reason }) => {
                    Err(SkillsHostError::Failed { reason })
                }
            },
        ),
    }
}

fn base_builder(model: Arc<dyn Model>) -> finstack_ai::NativeAgentBuilder {
    Agent::builder(
        AgentId::parse("test.agent.activation").expect("agent"),
        BundleId::parse("test.bundle.activation").expect("bundle"),
        (component("test.model.activation"), model),
        (component("test.store.activation"), store()),
    )
}

#[tokio::test]
async fn second_compaction_owner_in_an_inactive_capability_fails_resolution() {
    let model: Arc<dyn Model> =
        Arc::new(ScriptedModel::from_plans(profile(), vec![completed("ok")]));
    let error = base_builder(model)
        .middleware(
            component("test.middleware.base-compactor"),
            compactor("test.middleware.base-compactor"),
        )
        .install_middleware(
            component("test.middleware.inactive-compactor"),
            compactor("test.middleware.inactive-compactor"),
        )
        .capability(capability(
            "test.capability.research",
            "Research notes",
            "Research instruction.",
            CapabilityActivation::Model,
            Vec::new(),
            Vec::new(),
            vec![component("test.middleware.inactive-compactor")],
        ))
        .build()
        .await
        .err()
        .expect("second compacting owner must fail resolution");
    assert!(
        error
            .to_string()
            .to_ascii_lowercase()
            .contains("middleware"),
        "{error}"
    );
}

#[tokio::test]
async fn resolved_agent_lock_covers_inactive_capabilities() {
    let model: Arc<dyn Model> =
        Arc::new(ScriptedModel::from_plans(profile(), vec![completed("ok")]));
    let research = component("test.middleware.research");
    let agent = base_builder(model)
        .middleware(
            component("test.middleware.base"),
            standard_middleware("test.middleware.base"),
        )
        .install_middleware(
            research.clone(),
            standard_middleware("test.middleware.research"),
        )
        .capability(capability(
            "test.capability.research",
            "Research notes",
            "Research instruction.",
            CapabilityActivation::Model,
            Vec::new(),
            Vec::new(),
            vec![research],
        ))
        .build()
        .await
        .expect("agent");
    let lock = agent.resolved().lock().expect("lock");
    assert!(
        lock.capabilities
            .iter()
            .any(
                |capability| capability.id.as_str() == "test.capability.research"
                    && !capability.active
            )
    );
    assert!(
        lock.components
            .iter()
            .any(|component| component.component.id().as_str() == "test.middleware.research")
    );
    let chain = agent.resolved().run_plan().middleware_chain().digest();
    assert_eq!(lock.middleware_chain_digest, chain);
}

#[tokio::test]
async fn untrusted_capability_cannot_set_trusted_application_instructions() {
    let model: Arc<dyn Model> =
        Arc::new(ScriptedModel::from_plans(profile(), vec![completed("ok")]));
    let provider: Arc<dyn ContextProvider> = Arc::new(ScriptedContextProvider::new(
        ContextProviderDescriptor {
            invocation: invocation("test.context.untrusted"),
            trusted_application_instructions: true,
            metadata: finstack_ai::runtime::Metadata::empty(),
        },
        vec![ScriptedContextAction::Return(ContextContribution::try_new(
            Vec::new(),
            None::<&str>,
        ))],
    ));
    let error = base_builder(model)
        .install_context_provider(component("test.context.untrusted"), provider)
        .capability(capability(
            "test.capability.research",
            "Research notes",
            "Research instruction.",
            CapabilityActivation::Model,
            Vec::new(),
            vec![component("test.context.untrusted")],
            Vec::new(),
        ))
        .build()
        .await
        .err()
        .expect("untrusted trusted_application_instructions must fail");
    assert!(
        error
            .to_string()
            .contains("trusted_application_instructions"),
        "{error}"
    );
}

#[test]
fn concurrent_activation_overflow_fails_without_eviction() {
    let host = NativeCapabilityHost::new("test.capability.research: Research notes");
    let run = RunId::from_bytes([7; 16]);
    let named = ActiveCapability {
        capability_id: CapabilityId::parse("test.capability.research").expect("id"),
        source: CapabilityActivationSource::Model,
    };
    for _ in 0..MAX_CONCURRENT_CAPABILITY_ACTIVATIONS {
        host.queue_activation(run, vec![named.clone()])
            .expect("within bound");
    }
    let error = host
        .queue_activation(run, vec![named.clone()])
        .expect_err("overflow");
    assert_eq!(error, ActivationHostError::Bound);
    assert_eq!(error.to_string(), CAPABILITY_ACTIVATION_BOUND);
    let pending = host.take_pending(run).expect("queued sets remain");
    assert_eq!(pending.len(), 1);
}

#[tokio::test]
async fn activating_a_model_capability_commits_before_tools_appear() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![activate_call("test.capability.research"), completed("done")],
    ));
    let host = Arc::new(NativeCapabilityHost::new(
        "test.capability.research: Research notes",
    ));
    let skills: Arc<dyn finstack_ai::runtime::Toolset> =
        Arc::new(SkillsToolset::try_new(skills_host(&host)).expect("skills"));
    let calculator: Arc<dyn finstack_ai::runtime::Toolset> =
        Arc::new(CalculatorToolset::try_new().expect("calculator"));
    let agent = base_builder(Arc::clone(&model) as Arc<dyn Model>)
        .capability_activation_host(Arc::clone(&host))
        .toolset(component("test.tools.skills"), skills)
        .install_toolset(component("test.tools.calculator"), calculator)
        .capability(capability(
            "test.capability.research",
            "Research notes",
            "Research instruction.",
            CapabilityActivation::Model,
            vec![component("test.tools.calculator")],
            Vec::new(),
            Vec::new(),
        ))
        .build()
        .await
        .expect("agent");
    let chain_before = agent
        .resolved()
        .lock()
        .expect("lock")
        .middleware_chain_digest;
    let output = agent.run(request("activate research")).await.expect("run");
    assert_eq!(model.request_count(), 2);
    assert!(
        output
            .active_capabilities
            .iter()
            .any(|item| item.capability_id.as_str() == "test.capability.research")
    );
    let second = model.last_request().expect("second request");
    let names: Vec<_> = second
        .draft
        .tools
        .iter()
        .map(|tool| tool.model_name.as_ref())
        .collect();
    assert!(names.contains(&"calculator"), "{names:?}");
    assert_eq!(
        agent
            .resolved()
            .lock()
            .expect("lock")
            .middleware_chain_digest,
        chain_before
    );
}
