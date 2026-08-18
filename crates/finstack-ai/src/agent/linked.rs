//! Rust-owned linked-provider constructors (ADR-045).
//!
//! Python and WASM only map arguments. `wasm-host` methods exist and return
//! [`crate::AGENT_RUN_UNSUPPORTED_PLAN`]. Constructors never read ambient env.

use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "native-tokio")]
use finstack_ai_kernel::{AgentId, BundleId, ComponentId, Version};
use finstack_ai_kernel::{CapabilityId, ComponentRef, RawJson};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::Model;
use finstack_ai_runtime::{
    ContextProvider, Middleware, ModelName, ModelSettings, Observer, Toolset,
};

#[cfg(feature = "native-tokio")]
use crate::RunPolicy;
use crate::{CapabilitySpec, ChildRunPolicy};

use super::handle::Agent;
#[cfg(feature = "native-tokio")]
use super::types::AGENT_RUN_INVALID_CONFIGURATION;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use super::types::AGENT_RUN_UNSUPPORTED_PLAN;
use super::types::AgentRunError;

#[cfg(feature = "native-tokio")]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(feature = "native-tokio")]
const OPENAI_TIMEOUT: Duration = Duration::from_mins(2);
#[cfg(feature = "native-tokio")]
const LINKED_CONTEXT_WINDOW_TOKENS: u64 = 1_050_000;
#[cfg(feature = "native-tokio")]
const LINKED_RESERVED_OUTPUT_TOKENS: u64 = 128_000;
#[cfg(feature = "native-tokio")]
const LINKED_ANTHROPIC_OUTPUT_TOKENS: u64 = 64_000;
#[cfg(feature = "native-tokio")]
const LINKED_PROVIDER_OVERHEAD_TOKENS: u64 = 64;
#[cfg(feature = "native-tokio")]
const REASONING_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
#[cfg(feature = "native-tokio")]
const REASONING_SUMMARIES: &[&str] = &["auto", "concise", "detailed"];
#[cfg(feature = "native-tokio")]
const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

/// Already-resolved port handles supplied by a language binding.
#[derive(Default)]
pub struct LinkedAgentPorts {
    /// Toolset registrations.
    pub toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    /// Context-provider registrations.
    pub context_providers: Vec<(ComponentRef, Arc<dyn ContextProvider>)>,
    /// Middleware registrations.
    pub middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    /// Observer registrations.
    pub observers: Vec<(ComponentRef, Arc<dyn Observer>)>,
    /// Optional compiled output schema. Bindings convert host schema types.
    pub output_schema: Option<RawJson>,
}

/// Constructed linked agent plus the run defaults the binding keeps.
pub struct LinkedAgent {
    /// Resolved facade.
    pub agent: Agent,
    /// Provider model name selected at construct time.
    pub model: ModelName,
    /// Canonical provider settings (`OpenAI` reasoning fields, otherwise `{}`).
    pub settings: ModelSettings,
    /// Default operational timeout for subsequent runs.
    pub default_timeout: Duration,
}

/// Arguments for [`Agent::openai`].
pub struct OpenAiAgentSpec {
    /// Responses model name.
    pub model: String,
    /// Explicit Bearer credential. Never read from the environment.
    pub api_key: String,
    /// Optional system instruction.
    pub instruction: Option<String>,
    /// Declarative catalog entries.
    pub capabilities: Vec<CapabilitySpec>,
    /// Application activations selected at construct time.
    pub active_capabilities: Vec<CapabilityId>,
    /// Optional Responses reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Optional Responses reasoning summary.
    pub reasoning_summary: Option<String>,
    /// Binding-resolved ports.
    pub ports: LinkedAgentPorts,
    /// Child-run policy. Bindings default this to Deny.
    pub child_runs: ChildRunPolicy,
}

/// Arguments for [`Agent::anthropic`].
pub struct AnthropicAgentSpec {
    /// Messages API base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Optional API key. HTTPS is required when set.
    pub api_key: Option<String>,
    /// Optional system instruction.
    pub instruction: Option<String>,
    /// Declarative catalog entries.
    pub capabilities: Vec<CapabilitySpec>,
    /// Application activations selected at construct time.
    pub active_capabilities: Vec<CapabilityId>,
    /// Binding-resolved ports.
    pub ports: LinkedAgentPorts,
    /// Child-run policy. Bindings default this to Deny.
    pub child_runs: ChildRunPolicy,
}

/// Arguments for [`Agent::ollama`].
pub struct OllamaAgentSpec {
    /// Native `/api/chat` base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Optional system instruction.
    pub instruction: Option<String>,
    /// Declarative catalog entries.
    pub capabilities: Vec<CapabilitySpec>,
    /// Application activations selected at construct time.
    pub active_capabilities: Vec<CapabilityId>,
    /// Binding-resolved ports.
    pub ports: LinkedAgentPorts,
    /// Child-run policy. Bindings default this to Deny.
    pub child_runs: ChildRunPolicy,
}

impl Agent {
    /// Construct an official `OpenAI` Responses agent.
    ///
    /// Always targets `https://api.openai.com`. Does not read environment
    /// variables.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_UNSUPPORTED_PLAN`] on `wasm-host`. Returns
    /// [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when credentials, model, or
    /// reasoning settings are invalid.
    pub async fn openai(spec: OpenAiAgentSpec) -> Result<LinkedAgent, AgentRunError> {
        openai_inner(spec).await
    }

    /// Construct an Anthropic Messages agent.
    ///
    /// Does not read environment variables. HTTPS is required when `api_key`
    /// is set.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_UNSUPPORTED_PLAN`] on `wasm-host`. Returns
    /// [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when the URL, model, or
    /// credential pairing is invalid.
    pub async fn anthropic(spec: AnthropicAgentSpec) -> Result<LinkedAgent, AgentRunError> {
        anthropic_inner(spec).await
    }

    /// Construct a keyless Ollama `/api/chat` agent.
    ///
    /// Does not read environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_UNSUPPORTED_PLAN`] on `wasm-host`. Returns
    /// [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when the URL or model is invalid.
    pub async fn ollama(spec: OllamaAgentSpec) -> Result<LinkedAgent, AgentRunError> {
        ollama_inner(spec).await
    }
}

#[cfg(feature = "native-tokio")]
async fn openai_inner(spec: OpenAiAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_provider_openai::{
        Authentication, OpenAiConfig, OpenAiModelConfig, OpenAiProvider, SecretString,
    };

    let settings = reasoning_settings(
        spec.reasoning_effort.as_deref(),
        spec.reasoning_summary.as_deref(),
    )?;
    let config = OpenAiConfig::try_new("https://api.openai.com")
        .map_err(|error| model_configuration_error(&error))?
        .with_authentication(Authentication::Bearer(
            SecretString::try_new(spec.api_key).map_err(|error| model_configuration_error(&error))?,
        ));
    let mut model_config = OpenAiModelConfig::try_new(
        &spec.model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_PROVIDER_OVERHEAD_TOKENS,
    )
    .map_err(|error| model_configuration_error(&error))?;
    model_config = model_config.with_reasoning(true);
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OpenAiProvider::try_new(config, vec![model_config])
            .map_err(|error| model_configuration_error(&error))?,
    );
    finish_linked_agent(
        "python.agent.openai",
        "python.bundle.openai",
        "python.model.openai",
        provider,
        model_name,
        spec.instruction,
        spec.capabilities,
        spec.active_capabilities,
        spec.ports,
        spec.child_runs,
        settings,
        OPENAI_TIMEOUT,
    )
    .await
}

#[cfg(feature = "native-tokio")]
async fn anthropic_inner(spec: AnthropicAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_provider_anthropic::{
        AnthropicConfig, AnthropicModelConfig, AnthropicProvider, Authentication, SecretString,
    };

    let mut config =
        AnthropicConfig::try_new(spec.base_url).map_err(|error| model_configuration_error(&error))?;
    if let Some(api_key) = spec.api_key {
        config = config.with_authentication(Authentication::ApiKey(
            SecretString::try_new(api_key).map_err(|error| model_configuration_error(&error))?,
        ));
    }
    let model_config = AnthropicModelConfig::try_new(
        &spec.model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_ANTHROPIC_OUTPUT_TOKENS,
        LINKED_ANTHROPIC_OUTPUT_TOKENS,
        LINKED_PROVIDER_OVERHEAD_TOKENS,
    )
    .map_err(|error| model_configuration_error(&error))?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        AnthropicProvider::try_new(config, vec![model_config])
            .map_err(|error| model_configuration_error(&error))?,
    );
    finish_linked_agent(
        "python.agent.anthropic",
        "python.bundle.anthropic",
        "python.model.anthropic",
        provider,
        model_name,
        spec.instruction,
        spec.capabilities,
        spec.active_capabilities,
        spec.ports,
        spec.child_runs,
        empty_model_settings()?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(feature = "native-tokio")]
async fn ollama_inner(spec: OllamaAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};

    let config =
        OllamaConfig::try_new(spec.base_url).map_err(|error| model_configuration_error(&error))?;
    let model_config = OllamaModelConfig::try_new(
        &spec.model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_PROVIDER_OVERHEAD_TOKENS,
    )
    .map_err(|error| model_configuration_error(&error))?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OllamaProvider::try_new(config, vec![model_config])
            .map_err(|error| model_configuration_error(&error))?,
    );
    finish_linked_agent(
        "python.agent.ollama",
        "python.bundle.ollama",
        "python.model.ollama",
        provider,
        model_name,
        spec.instruction,
        spec.capabilities,
        spec.active_capabilities,
        spec.ports,
        spec.child_runs,
        empty_model_settings()?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
#[expect(
    clippy::unused_async,
    reason = "wasm-host keeps the same async signature as native-tokio"
)]
async fn openai_inner(_spec: OpenAiAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported("openai")
}

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
#[expect(
    clippy::unused_async,
    reason = "wasm-host keeps the same async signature as native-tokio"
)]
async fn anthropic_inner(_spec: AnthropicAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported("anthropic")
}

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
#[expect(
    clippy::unused_async,
    reason = "wasm-host keeps the same async signature as native-tokio"
)]
async fn ollama_inner(_spec: OllamaAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported("ollama")
}

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
fn unsupported(name: &str) -> Result<LinkedAgent, AgentRunError> {
    Err(AgentRunError::configuration(
        AGENT_RUN_UNSUPPORTED_PLAN,
        format!("Agent::{name} is not supported on wasm-host"),
    ))
}

#[cfg(feature = "native-tokio")]
#[expect(
    clippy::too_many_arguments,
    reason = "finish keeps identity, ports, and run defaults contiguous"
)]
async fn finish_linked_agent(
    agent_id: &str,
    bundle_id: &str,
    model_id: &str,
    provider: Arc<dyn Model>,
    model_name: ModelName,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    ports: LinkedAgentPorts,
    child_runs: ChildRunPolicy,
    settings: ModelSettings,
    default_timeout: Duration,
) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 64,
            batches_per_session: 256,
            records_per_session: 4_096,
            snapshot_bytes: 64 * 1_024,
        })
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
    );
    let mut builder = Agent::builder(
        AgentId::parse(agent_id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        BundleId::parse(bundle_id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        (component(model_id)?, provider),
        (component("python.store.memory")?, store),
    );
    for (component, toolset) in ports.toolsets {
        builder = builder.toolset(component, toolset);
    }
    for (component, provider) in ports.context_providers {
        builder = builder.context_provider(component, provider);
    }
    for (component, middleware) in ports.middleware {
        builder = builder.middleware(component, middleware);
    }
    for (component, observer) in ports.observers {
        builder = builder.observer(component, observer);
    }
    if let Some(instruction) = instruction {
        builder = builder.try_instruction(instruction)?;
    }
    for capability in capabilities {
        builder = builder.capability(capability);
    }
    for capability in active_capabilities {
        builder = builder.activate_application(capability);
    }
    builder = builder.policy(RunPolicy {
        child_runs,
        ..RunPolicy::default()
    });
    let agent = builder.build().await?;
    let agent = if let Some(schema) = ports.output_schema {
        agent.try_with_output_schema(&schema)?
    } else {
        agent
    };
    Ok(LinkedAgent {
        agent,
        model: model_name,
        settings,
        default_timeout,
    })
}

#[cfg(feature = "native-tokio")]
fn component(id: &str) -> Result<ComponentRef, AgentRunError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        Some(PREVIEW_VERSION),
    ))
}

#[cfg(feature = "native-tokio")]
fn model_configuration_error(error: &finstack_ai_runtime::ModelError) -> AgentRunError {
    AgentRunError::configuration(
        AGENT_RUN_INVALID_CONFIGURATION,
        format!("{}: {}", error.code(), error.message()),
    )
}

#[cfg(feature = "native-tokio")]
fn empty_model_settings() -> Result<ModelSettings, AgentRunError> {
    Ok(ModelSettings {
        values: RawJson::parse(b"{}").map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
    })
}

#[cfg(feature = "native-tokio")]
fn reasoning_settings(
    effort: Option<&str>,
    summary: Option<&str>,
) -> Result<ModelSettings, AgentRunError> {
    let mut fields = Vec::new();
    if let Some(value) = effort {
        if !REASONING_EFFORTS.contains(&value) {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "reasoning_effort must be one of none, minimal, low, medium, high, xhigh, max",
            ));
        }
        fields.push(format!(r#""reasoning_effort":"{value}""#));
    }
    if let Some(value) = summary {
        if !REASONING_SUMMARIES.contains(&value) {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "reasoning_summary must be one of auto, concise, detailed",
            ));
        }
        fields.push(format!(r#""reasoning_summary":"{value}""#));
    }
    if fields.is_empty() {
        return empty_model_settings();
    }
    let payload = format!("{{{}}}", fields.join(","));
    RawJson::parse(payload.as_bytes())
        .map(|values| ModelSettings { values })
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })
}

#[cfg(all(test, feature = "native-tokio"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn openai_constructs_without_a_network_request() {
        let built = Agent::openai(OpenAiAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-openai-secret-canary-056".into(),
            instruction: Some("Answer concisely.".into()),
            capabilities: Vec::new(),
            active_capabilities: Vec::new(),
            reasoning_effort: None,
            reasoning_summary: None,
            ports: LinkedAgentPorts::default(),
            child_runs: ChildRunPolicy::Deny,
        })
        .await
        .expect("openai construct");
        assert!(built.agent.capability_catalog().is_empty());
        assert_eq!(built.default_timeout, OPENAI_TIMEOUT);
    }

    #[tokio::test]
    async fn openai_rejects_unknown_reasoning_effort() {
        let error = Agent::openai(OpenAiAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-openai-secret-canary-056".into(),
            instruction: None,
            capabilities: Vec::new(),
            active_capabilities: Vec::new(),
            reasoning_effort: Some("turbo".into()),
            reasoning_summary: None,
            ports: LinkedAgentPorts::default(),
            child_runs: ChildRunPolicy::Deny,
        })
        .await
        .err()
        .expect("unknown effort");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("reasoning_effort"));
    }

    #[tokio::test]
    async fn openai_empty_model_does_not_leak_the_api_key() {
        let canary = "sk-openai-secret-canary-056";
        let error = Agent::openai(OpenAiAgentSpec {
            model: String::new(),
            api_key: canary.into(),
            instruction: None,
            capabilities: Vec::new(),
            active_capabilities: Vec::new(),
            reasoning_effort: None,
            reasoning_summary: None,
            ports: LinkedAgentPorts::default(),
            child_runs: ChildRunPolicy::Deny,
        })
        .await
        .err()
        .expect("empty model");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(!error.to_string().contains(canary));
    }

    #[tokio::test]
    async fn anthropic_http_credentials_fail_closed_without_leaking_the_canary() {
        let canary = "sk-ant-secret-canary-055";
        let error = Agent::anthropic(AnthropicAgentSpec {
            base_url: "http://127.0.0.1:9".into(),
            model: "fixture-model".into(),
            api_key: Some(canary.into()),
            instruction: None,
            capabilities: Vec::new(),
            active_capabilities: Vec::new(),
            ports: LinkedAgentPorts::default(),
            child_runs: ChildRunPolicy::Deny,
        })
        .await
        .err()
        .expect("http + key");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(!error.to_string().contains(canary));
    }

    #[tokio::test]
    async fn ollama_constructs_without_a_network_request() {
        let built = Agent::ollama(OllamaAgentSpec {
            base_url: "http://127.0.0.1:11434".into(),
            model: "fixture-model".into(),
            instruction: None,
            capabilities: Vec::new(),
            active_capabilities: Vec::new(),
            ports: LinkedAgentPorts::default(),
            child_runs: ChildRunPolicy::Deny,
        })
        .await
        .expect("ollama construct");
        assert!(built.agent.capability_catalog().is_empty());
    }
}
