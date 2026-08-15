//! Catalog and activation-prefix stability across model handles.

use std::sync::Arc;

use finstack_ai::runtime::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentRef, JournalStore, Model,
    ModelContextProfile, ModelName, ModelStreamItem, TextDelta, TokenEstimatorRef,
    TokenEstimatorSource, Version,
};
use finstack_ai::{
    Agent, AgentRunRequest, CapabilityActivation, CapabilitySpec, InstructionSpec, PrincipalRef,
    RunSecurityContext,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

#[tokio::test]
async fn capability_catalog_bytes_are_identical_when_only_the_model_handle_changes() {
    let first = catalog_agent(scripted("first")).await;
    let second = catalog_agent(scripted("second")).await;
    assert_eq!(
        first.compact_capability_catalog(),
        second.compact_capability_catalog()
    );
    assert_eq!(
        first.compact_capability_catalog(),
        "test.capability.research: Research financial statements"
    );
    assert_eq!(first.capability_catalog(), second.capability_catalog());
}

#[tokio::test]
async fn capability_activation_prefix_is_stable_across_repeated_runs() {
    let model = scripted("stable");
    let agent = catalog_agent(Arc::clone(&model) as Arc<dyn Model>).await;
    let first = agent.run(request("say hello")).await.expect("first run");
    let first_prefix = instruction_prefix(&model);
    let mut research = request("research financial statements");
    research.capability =
        Some(CapabilityId::parse("test.capability.research").expect("research capability"));
    let second = agent.run(research).await.expect("second run");
    let second_prefix = instruction_prefix(&model);
    assert_eq!(
        first_prefix,
        [
            "Stable prefix.",
            "Always instruction.",
            "Application instruction."
        ]
    );
    assert_eq!(
        &second_prefix[..first_prefix.len()],
        first_prefix.as_slice()
    );
    assert_eq!(
        first.active_capabilities.len() + 1,
        second.active_capabilities.len()
    );
}

fn instruction_prefix(model: &ScriptedModel) -> Vec<String> {
    let request = model.last_request().expect("model request");
    request
        .draft
        .messages
        .iter()
        .filter(|message| message.role() == finstack_ai::runtime::MessageRole::System)
        .map(|message| {
            message
                .content()
                .iter()
                .filter_map(|block| match block {
                    finstack_ai::runtime::ContentBlock::Text(text) => Some(text.text().to_owned()),
                    _ => None,
                })
                .collect::<String>()
        })
        .collect()
}

async fn catalog_agent(model: Arc<dyn Model>) -> Agent {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 512,
            snapshot_bytes: 8_192,
        })
        .expect("store"),
    );
    let mut builder = Agent::builder(
        AgentId::parse("test.agent.capability-catalog").expect("agent"),
        BundleId::parse("test.bundle.capability-catalog").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.capability-catalog").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.capability-catalog").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .try_instruction("Stable prefix.")
    .expect("instruction");
    for capability in catalog_capabilities() {
        builder = builder.capability(capability);
    }
    builder
        .activate_application(
            CapabilityId::parse("test.capability.application").expect("application"),
        )
        .build()
        .await
        .expect("agent")
}

fn catalog_capabilities() -> Vec<CapabilitySpec> {
    vec![
        capability(
            "test.capability.always",
            "Baseline safety guidance",
            "Always instruction.",
            CapabilityActivation::Always,
        ),
        capability(
            "test.capability.application",
            "Application selected accounting guidance",
            "Application instruction.",
            CapabilityActivation::Application,
        ),
        capability(
            "test.capability.research",
            "Research financial statements",
            "Research instruction.",
            CapabilityActivation::Model,
        ),
    ]
}

fn capability(
    id: &str,
    description: &str,
    instruction: &str,
    activation: CapabilityActivation,
) -> CapabilitySpec {
    CapabilitySpec {
        id: CapabilityId::parse(id).expect("capability id"),
        description: Arc::from(description),
        instructions: Arc::from([InstructionSpec::try_new(instruction).expect("instruction")]),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation,
    }
}

fn scripted(label: &str) -> Arc<ScriptedModel> {
    let _ = label;
    Arc::new(ScriptedModel::from_plans(
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
        },
        vec![completed("ok"), completed("ok")],
    ))
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
                    completion_id: Arc::from("capability-1"),
                    continuation_state: None,
                },
            ))),
        ],
    }
}

fn request(input: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("preview-1").expect("model name"),
        input,
        RunSecurityContext::try_new(
            "catalog-test",
            PrincipalRef::try_new("issuer", "subject", Some("catalog-test")).expect("principal"),
            "local",
            "test",
            "catalog-policy-v1",
            "catalog-decision-v1",
            None,
        )
        .expect("security"),
    )
    .expect("request")
}
