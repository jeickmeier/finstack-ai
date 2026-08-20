//! Cross-provider catalog identity and leaf metadata refresh.

use std::sync::Arc;

use finstack_ai::runtime::{JournalStore, Model, ModelName};
use finstack_ai::{Agent, CapabilityActivation, CapabilitySpec, InstructionSpec};
use finstack_ai_kernel::CapabilityId;
use finstack_ai_kernel::{AgentId, BundleId, ComponentId, ComponentRef, Version};
use finstack_ai_provider_anthropic::{AnthropicConfig, AnthropicModelConfig, AnthropicProvider};
use finstack_ai_provider_gemini::{GeminiConfig, GeminiModelConfig, GeminiProvider};
use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_provider_openai::{OpenAiConfig, OpenAiModelConfig, OpenAiProvider};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::ScriptedModel;

const VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

#[tokio::test]
async fn capability_catalog_is_identical_across_scripted_compatible_anthropic_and_ollama() {
    let catalogs = [
        catalog_for(scripted()).await,
        catalog_for(openai_model()).await,
        catalog_for(anthropic_model()).await,
        catalog_for(ollama_model()).await,
        catalog_for(gemini_model()).await,
    ];
    for catalog in &catalogs[1..] {
        assert_eq!(&catalogs[0], catalog);
    }
    assert_eq!(
        catalogs[0],
        "test.capability.research: Research financial statements"
    );
}

#[test]
fn leaf_metadata_refresh_updates_advertised_flags_without_a_network() {
    let anthropic = AnthropicProvider::try_new(
        AnthropicConfig::try_new("http://127.0.0.1:9").expect("config"),
        vec![
            AnthropicModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model"),
        ],
    )
    .expect("provider");
    let name = ModelName::try_new("fixture-model").expect("name");
    assert!(!anthropic.capabilities(&name).reasoning);
    assert!(!anthropic.capabilities(&name).prompt_cache);
    let mut update = anthropic.capabilities(&name);
    update.reasoning = true;
    update.prompt_cache = true;
    anthropic
        .refresh_model_metadata(&name, update)
        .expect("refresh");
    assert!(anthropic.capabilities(&name).reasoning);
    assert!(anthropic.capabilities(&name).prompt_cache);

    let openai = OpenAiProvider::try_new(
        OpenAiConfig::try_new("http://127.0.0.1:9").expect("config"),
        vec![
            OpenAiModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model"),
        ],
    )
    .expect("provider");
    assert!(!openai.capabilities(&name).reasoning);
    let mut update = openai.capabilities(&name);
    update.reasoning = true;
    openai
        .refresh_model_metadata(&name, update)
        .expect("refresh");
    assert!(openai.capabilities(&name).reasoning);
}

async fn catalog_for(model: Arc<dyn Model>) -> String {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 16,
            records_per_session: 64,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    );
    let mut builder = Agent::builder(
        AgentId::parse("test.agent.provider-catalog").expect("agent"),
        BundleId::parse("test.bundle.provider-catalog").expect("bundle"),
        (
            ComponentRef::new(
                ComponentId::parse("test.model.provider-catalog").expect("model"),
                Some(VERSION),
            ),
            model,
        ),
        (
            ComponentRef::new(
                ComponentId::parse("test.store.provider-catalog").expect("store"),
                Some(VERSION),
            ),
            store,
        ),
    )
    .try_instruction("Stable prefix.")
    .expect("instruction");
    for capability in [
        spec(
            "test.capability.always",
            "Baseline safety guidance",
            "Always instruction.",
            CapabilityActivation::Always,
        ),
        spec(
            "test.capability.application",
            "Application selected accounting guidance",
            "Application instruction.",
            CapabilityActivation::Application,
        ),
        spec(
            "test.capability.research",
            "Research financial statements",
            "Research instruction.",
            CapabilityActivation::Model,
        ),
    ] {
        builder = builder.capability(capability);
    }
    builder
        .activate_application(
            CapabilityId::parse("test.capability.application").expect("application"),
        )
        .build()
        .await
        .expect("agent")
        .compact_capability_catalog()
}

fn spec(
    id: &str,
    description: &str,
    instruction: &str,
    activation: CapabilityActivation,
) -> CapabilitySpec {
    CapabilitySpec {
        id: CapabilityId::parse(id).expect("id"),
        description: Arc::from(description),
        instructions: Arc::from([InstructionSpec::try_new(instruction).expect("instruction")]),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation,
    }
}

fn scripted() -> Arc<dyn Model> {
    Arc::new(ScriptedModel::from_plans(
        finstack_ai::runtime::ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("preview-1").expect("model"),
            hard_input_bytes: 1_048_576,
            context_window_tokens: 8_192,
            max_output_tokens: 512,
            reserved_output_tokens: 128,
            provider_overhead_tokens: 32,
            estimator: finstack_ai::runtime::TokenEstimatorRef {
                id: Arc::from("scripted.utf8"),
                version: Arc::from("1"),
                source: finstack_ai::runtime::TokenEstimatorSource::ConservativeUpperBound,
            },
        },
        Vec::new(),
    ))
}

fn openai_model() -> Arc<dyn Model> {
    Arc::new(
        OpenAiProvider::try_new(
            OpenAiConfig::try_new("http://127.0.0.1:9").expect("config"),
            vec![
                OpenAiModelConfig::try_new("preview-1", 1_000_000, 128_000, 4_096, 4_096, 256)
                    .expect("model"),
            ],
        )
        .expect("provider"),
    )
}

fn ollama_model() -> Arc<dyn Model> {
    Arc::new(
        OllamaProvider::try_new(
            OllamaConfig::try_new("http://127.0.0.1:9").expect("config"),
            vec![
                OllamaModelConfig::try_new("preview-1", 1_000_000, 128_000, 4_096, 4_096, 256)
                    .expect("model"),
            ],
        )
        .expect("provider"),
    )
}

fn anthropic_model() -> Arc<dyn Model> {
    Arc::new(
        AnthropicProvider::try_new(
            AnthropicConfig::try_new("http://127.0.0.1:9").expect("config"),
            vec![
                AnthropicModelConfig::try_new("preview-1", 1_000_000, 128_000, 4_096, 4_096, 256)
                    .expect("model"),
            ],
        )
        .expect("provider"),
    )
}

fn gemini_model() -> Arc<dyn Model> {
    Arc::new(
        GeminiProvider::try_new(
            GeminiConfig::try_new("http://127.0.0.1:9").expect("config"),
            vec![
                GeminiModelConfig::try_new("preview-1", 1_000_000, 128_000, 4_096).expect("model"),
            ],
        )
        .expect("provider"),
    )
}
