//! Regression coverage for model-capability catalog preparation.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai::{Agent, AgentRunRequest, CapabilityActivation, CapabilitySpec, InstructionSpec};
use finstack_ai_kernel::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentRef, ContentBlock, ProviderIds,
    RunSecurityContext, TextBlock, Usage, ValidatedToolCall, Version,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::{
    Model, ModelContextProfile, ModelName, ModelResponse, ModelStreamItem, TokenEstimatorRef,
    TokenEstimatorSource, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, Toolset, ToolsetDescriptor,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_tools_calculator::CalculatorToolset;

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

struct CountingToolset {
    inner: CalculatorToolset,
    catalog_reads: Arc<AtomicUsize>,
}

impl Toolset for CountingToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.inner.descriptor()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        self.catalog_reads.fetch_add(1, Ordering::Relaxed);
        self.inner.tools()
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        self.inner.call(ctx, call)
    }
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("capability-cache-model").expect("model name"),
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

fn completed(text: &'static str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
            ModelResponse {
                assistant_content: Arc::from([ContentBlock::Text(
                    TextBlock::try_new(text).expect("assistant text"),
                )]),
                tool_calls: Arc::from([]),
                usage: Usage::empty(),
                provider_ids: ProviderIds::empty(),
                completion_id: Arc::from(text),
                continuation_state: None,
            },
        )))],
    }
}

fn request() -> AgentRunRequest {
    let security = RunSecurityContext::try_new(
        "tenant-capability-cache",
        finstack_ai_kernel::PrincipalRef::try_new(
            "capability-cache-test",
            "developer",
            Some("tenant-capability-cache"),
        )
        .expect("principal"),
        "local",
        "test",
        "policy-v1",
        "decision-v1",
        None,
    )
    .expect("security");
    let mut request = AgentRunRequest::try_new(
        ModelName::try_new("capability-cache-model").expect("model name"),
        "complete",
        security,
    )
    .expect("request");
    request.capability = Some(CapabilityId::parse("test.capability.research").expect("capability"));
    request
}

#[tokio::test]
async fn repeated_model_capability_runs_reuse_precompiled_tool_schemas() {
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![completed("first"), completed("second")],
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
    let catalog_reads = Arc::new(AtomicUsize::new(0));
    let toolset: Arc<dyn Toolset> = Arc::new(CountingToolset {
        inner: CalculatorToolset::try_new().expect("calculator"),
        catalog_reads: Arc::clone(&catalog_reads),
    });
    let toolset_ref = ComponentRef::new(
        ComponentId::parse("test.tools.capability-cache").expect("toolset"),
        Some(VERSION),
    );
    let capability = CapabilitySpec {
        id: CapabilityId::parse("test.capability.research").expect("capability"),
        description: Arc::from("Research notes"),
        instructions: Arc::from([
            InstructionSpec::try_new("Research instruction.").expect("instruction")
        ]),
        toolsets: Arc::from([toolset_ref.clone()]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation: CapabilityActivation::Model,
    };
    let agent = Agent::builder(
        AgentId::parse("test.agent.capability-cache").expect("agent"),
        BundleId::parse("test.bundle.capability-cache").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.capability-cache").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.capability-cache").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .capability_toolset(toolset_ref, toolset)
    .capability(capability)
    .build()
    .await
    .expect("agent");
    let construction_reads = catalog_reads.load(Ordering::Relaxed);

    let _first = agent.start(request()).expect("first selection");
    let _second = agent.start(request()).expect("second selection");
    let after_two_selections = catalog_reads.load(Ordering::Relaxed);

    println!(
        "catalog reads: construction={construction_reads}, after_two_selections={after_two_selections}"
    );

    assert_eq!(
        after_two_selections, construction_reads,
        "model-capability selection must not rebuild and recompile the immutable tool catalog"
    );
}
