//! Rust-owned linked-provider constructors (ADR-045).
//!
//! Python and WASM only map arguments. `wasm-host` methods exist and return
//! [`crate::AGENT_RUN_UNSUPPORTED_PLAN`]. Constructors never read ambient env.

use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "native-tokio")]
use finstack_ai_kernel::ComponentId;
#[cfg(feature = "native-tokio")]
use finstack_ai_kernel::{AgentId, BundleId};
use finstack_ai_kernel::{CapabilityId, ComponentRef, RawJson};
use finstack_ai_runtime::{
    ContextProvider, Middleware, ModelName, ModelSettings, Observer, Toolset,
};
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::{JournalStore, Model};

use crate::{CapabilitySpec, ChildRunPolicy, RunPolicy};

use super::builder::NativeAgentBuilder;
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

/// Host-agnostic fields shared by every linked-provider constructor.
#[derive(Default)]
pub struct LinkedCommon {
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

/// Arguments for [`Agent::openai`].
pub struct OpenAiAgentSpec {
    /// Responses model name.
    pub model: String,
    /// Explicit Bearer credential. Never read from the environment.
    pub api_key: String,
    /// Optional Responses reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Optional Responses reasoning summary.
    pub reasoning_summary: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`Agent::anthropic`].
pub struct AnthropicAgentSpec {
    /// Messages API base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Optional API key. HTTPS is required when set.
    pub api_key: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`Agent::ollama`].
pub struct OllamaAgentSpec {
    /// Native `/api/chat` base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`Agent::gateway`].
pub struct GatewayAgentSpec {
    /// Provider endpoint URL. HTTPS is required off loopback.
    pub endpoint: String,
    /// Configured model name.
    pub model: String,
    /// Wire protocol: `openai_responses`, `anthropic_messages`, or `ollama_chat`.
    /// `openai_chat` is a configuration error.
    pub wire_protocol: String,
    /// Named credential reference. Never a secret literal.
    pub credential_name: String,
    /// Maximum canonical request bytes. Construction fails when omitted.
    pub hard_input_bytes: Option<u64>,
    /// Auth scheme: `none`, `bearer`, or `api_key`. Defaults from `api_key`.
    pub auth_kind: Option<String>,
    /// Explicit credential. Never read from the environment.
    pub api_key: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`Agent::e2b_sandbox`].
pub struct E2bSandboxAgentSpec {
    /// Catalog model name reserved for the constructed agent.
    pub model: String,
    /// Explicit E2B API key. Never read from the environment.
    pub api_key: String,
    /// Optional HTTPS product endpoint, or loopback HTTP for fixtures.
    pub endpoint: Option<String>,
    /// Optional sandbox template. Defaults to `base`.
    pub template: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

impl NativeAgentBuilder {
    /// Apply binding-resolved ports and finish as a [`LinkedAgent`].
    ///
    /// This is the single finish path for linked-provider constructors and
    /// host-handle factories. It does not read environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when composition,
    /// lock, or output-schema compilation fails.
    pub async fn build_linked(
        mut self,
        common: LinkedCommon,
        model_name: ModelName,
        settings: ModelSettings,
        default_timeout: Duration,
    ) -> Result<LinkedAgent, AgentRunError> {
        let LinkedCommon {
            instruction,
            capabilities,
            active_capabilities,
            ports,
            child_runs,
        } = common;
        for (component, toolset) in ports.toolsets {
            self = self.toolset(component, toolset);
        }
        for (component, provider) in ports.context_providers {
            self = self.context_provider(component, provider);
        }
        for (component, middleware) in ports.middleware {
            self = self.middleware(component, middleware);
        }
        for (component, observer) in ports.observers {
            self = self.observer(component, observer);
        }
        if let Some(instruction) = instruction {
            self = self.try_instruction(instruction)?;
        }
        for capability in capabilities {
            self = self.capability(capability);
        }
        for capability in active_capabilities {
            self = self.activate_application(capability);
        }
        self = self.policy(RunPolicy {
            child_runs,
            ..RunPolicy::default()
        });
        let agent = self.build().await?;
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

    /// Construct a config-driven gateway agent.
    ///
    /// Dispatches onto the dedicated openai, anthropic, or ollama provider
    /// under `native-tokio` only. Does not read environment variables. HTTPS
    /// is required off loopback and whenever a credential is set. `openai_chat`
    /// is a configuration error.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_UNSUPPORTED_PLAN`] on `wasm-host`. Returns
    /// [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when the route, protocol,
    /// `hard_input_bytes`, or credential pairing is invalid.
    pub async fn gateway(spec: GatewayAgentSpec) -> Result<LinkedAgent, AgentRunError> {
        gateway_inner(spec).await
    }

    /// Construct an agent that registers the T4 E2B sandbox Toolset.
    ///
    /// Reuses `finstack-ai-sandbox-e2b` under `native-tokio` only. Does not
    /// read environment variables. Construction fails without an explicit API
    /// key. Non-loopback endpoints must be HTTPS.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_UNSUPPORTED_PLAN`] on `wasm-host`. Returns
    /// [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when the API key or endpoint
    /// is invalid.
    pub async fn e2b_sandbox(spec: E2bSandboxAgentSpec) -> Result<LinkedAgent, AgentRunError> {
        e2b_sandbox_inner(spec).await
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
            SecretString::try_new(spec.api_key).map_err(|_| secret_configuration_error())?,
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
    compose_provider(
        (
            "python.agent.openai",
            "python.bundle.openai",
            "python.model.openai",
        ),
        provider,
        model_name,
        spec.common,
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

    let mut config = AnthropicConfig::try_new(spec.base_url)
        .map_err(|error| model_configuration_error(&error))?;
    if let Some(api_key) = spec.api_key {
        config = config.with_authentication(Authentication::ApiKey(
            SecretString::try_new(api_key).map_err(|_| secret_configuration_error())?,
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
    compose_provider(
        (
            "python.agent.anthropic",
            "python.bundle.anthropic",
            "python.model.anthropic",
        ),
        provider,
        model_name,
        spec.common,
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
    compose_provider(
        (
            "python.agent.ollama",
            "python.bundle.ollama",
            "python.model.ollama",
        ),
        provider,
        model_name,
        spec.common,
        empty_model_settings()?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(feature = "native-tokio")]
async fn gateway_inner(spec: GatewayAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_runtime::{Authentication, CredentialReference, CredentialStore};

    let hard_input_bytes = spec.hard_input_bytes.ok_or_else(|| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway model hard_input_bytes is required",
        )
    })?;
    let authentication = gateway_authentication(spec.auth_kind.as_deref(), spec.api_key)?;
    require_https_or_loopback(&spec.endpoint)?;
    require_https_for_credentials(
        &spec.endpoint,
        !matches!(authentication, Authentication::None),
    )?;
    if spec.wire_protocol == "openai_chat" {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway wire_protocol openai_chat is not supported",
        ));
    }
    let authentication = match spec.wire_protocol.as_str() {
        "openai_responses" | "ollama_chat" => match authentication {
            Authentication::ApiKey(secret) => Authentication::Bearer(secret),
            other => other,
        },
        "anthropic_messages" => match authentication {
            Authentication::Bearer(secret) => Authentication::ApiKey(secret),
            other => other,
        },
        _ => {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "gateway wire_protocol must be openai_responses, anthropic_messages, or ollama_chat",
            ));
        }
    };
    let reference = CredentialReference::try_new(&spec.credential_name).map_err(|_| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway credential_name is invalid",
        )
    })?;
    let mut store = CredentialStore::empty();
    store
        .insert(&spec.credential_name, authentication)
        .map_err(|_| {
            AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "gateway credential_name is invalid",
            )
        })?;
    let (provider, model_name) = gateway_provider(
        &spec.wire_protocol,
        &spec.endpoint,
        &spec.model,
        hard_input_bytes,
        store,
        reference,
    )?;
    compose_provider(
        (
            "python.agent.gateway",
            "python.bundle.gateway",
            "python.model.gateway",
        ),
        provider,
        model_name,
        spec.common,
        empty_model_settings()?,
        OPENAI_TIMEOUT,
    )
    .await
}

#[cfg(feature = "native-tokio")]
fn gateway_provider(
    wire_protocol: &str,
    endpoint: &str,
    model: &str,
    hard_input_bytes: u64,
    store: finstack_ai_runtime::CredentialStore,
    reference: finstack_ai_runtime::CredentialReference,
) -> Result<(Arc<dyn Model>, ModelName), AgentRunError> {
    match wire_protocol {
        "openai_responses" => {
            use finstack_ai_provider_openai::{OpenAiConfig, OpenAiModelConfig, OpenAiProvider};
            let config = OpenAiConfig::try_new(endpoint)
                .map_err(|error| model_configuration_error(&error))?
                .with_credential_store(store, reference);
            let model = OpenAiModelConfig::try_new(
                model,
                hard_input_bytes,
                LINKED_CONTEXT_WINDOW_TOKENS,
                LINKED_RESERVED_OUTPUT_TOKENS,
                LINKED_RESERVED_OUTPUT_TOKENS,
                LINKED_PROVIDER_OVERHEAD_TOKENS,
            )
            .map_err(|error| model_configuration_error(&error))?;
            let model_name = model.name.clone();
            Ok((
                Arc::new(
                    OpenAiProvider::try_new(config, vec![model])
                        .map_err(|error| model_configuration_error(&error))?,
                ),
                model_name,
            ))
        }
        "anthropic_messages" => {
            use finstack_ai_provider_anthropic::{
                AnthropicConfig, AnthropicModelConfig, AnthropicProvider,
            };
            let config = AnthropicConfig::try_new(endpoint)
                .map_err(|error| model_configuration_error(&error))?
                .with_credential_store(store, reference);
            let model = AnthropicModelConfig::try_new(
                model,
                hard_input_bytes,
                LINKED_CONTEXT_WINDOW_TOKENS,
                LINKED_ANTHROPIC_OUTPUT_TOKENS,
                LINKED_ANTHROPIC_OUTPUT_TOKENS,
                LINKED_PROVIDER_OVERHEAD_TOKENS,
            )
            .map_err(|error| model_configuration_error(&error))?;
            let model_name = model.name.clone();
            Ok((
                Arc::new(
                    AnthropicProvider::try_new(config, vec![model])
                        .map_err(|error| model_configuration_error(&error))?,
                ),
                model_name,
            ))
        }
        "ollama_chat" => {
            use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
            let config = OllamaConfig::try_new(endpoint)
                .map_err(|error| model_configuration_error(&error))?
                .with_credential_store(store, reference);
            let model = OllamaModelConfig::try_new(
                model,
                hard_input_bytes,
                LINKED_CONTEXT_WINDOW_TOKENS,
                LINKED_RESERVED_OUTPUT_TOKENS,
                LINKED_RESERVED_OUTPUT_TOKENS,
                LINKED_PROVIDER_OVERHEAD_TOKENS,
            )
            .map_err(|error| model_configuration_error(&error))?;
            let model_name = model.name.clone();
            Ok((
                Arc::new(
                    OllamaProvider::try_new(config, vec![model])
                        .map_err(|error| model_configuration_error(&error))?,
                ),
                model_name,
            ))
        }
        _ => Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway wire_protocol must be openai_responses, anthropic_messages, or ollama_chat",
        )),
    }
}

#[cfg(feature = "native-tokio")]
async fn e2b_sandbox_inner(spec: E2bSandboxAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_sandbox_e2b::{E2bSandboxConfig, E2bSandboxError, E2bSandboxToolset};

    if spec.api_key.is_empty() {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "e2b sandbox construction requires an explicit API key",
        ));
    }
    let model_name = ModelName::try_new(&spec.model).map_err(|error| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            format!("{}: {}", error.code(), error.message()),
        )
    })?;
    let toolset = E2bSandboxToolset::try_new(E2bSandboxConfig {
        api_key: spec.api_key,
        endpoint: spec.endpoint.unwrap_or_default(),
        template: spec.template,
    })
    .map_err(|error| match error {
        E2bSandboxError::CredentialRequired => AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "e2b sandbox construction requires an explicit API key",
        ),
        E2bSandboxError::EndpointInvalid { reason } => {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, reason)
        }
    })?;
    let mut common = spec.common;
    common
        .ports
        .toolsets
        .push((component("python.toolset.e2b")?, Arc::new(toolset)));
    let provider: Arc<dyn Model> = Arc::new(E2bCatalogModel {
        name: model_name.clone(),
    });
    compose_provider(
        ("python.agent.e2b", "python.bundle.e2b", "python.model.e2b"),
        provider,
        model_name,
        common,
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
#[expect(
    clippy::unused_async,
    reason = "wasm-host keeps the same async signature as native-tokio"
)]
async fn gateway_inner(_spec: GatewayAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported("gateway")
}

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
#[expect(
    clippy::unused_async,
    reason = "wasm-host keeps the same async signature as native-tokio"
)]
async fn e2b_sandbox_inner(_spec: E2bSandboxAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported("e2b_sandbox")
}

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
fn unsupported(name: &str) -> Result<LinkedAgent, AgentRunError> {
    Err(AgentRunError::configuration(
        AGENT_RUN_UNSUPPORTED_PLAN,
        format!("Agent::{name} is not supported on wasm-host"),
    ))
}

#[cfg(feature = "native-tokio")]
async fn compose_provider(
    (agent_id, bundle_id, model_id): (&str, &str, &str),
    provider: Arc<dyn Model>,
    model_name: ModelName,
    common: LinkedCommon,
    settings: ModelSettings,
    default_timeout: Duration,
) -> Result<LinkedAgent, AgentRunError> {
    Agent::builder(
        AgentId::parse(agent_id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        BundleId::parse(bundle_id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        (component(model_id)?, provider),
        memory_store()?,
    )
    .build_linked(common, model_name, settings, default_timeout)
    .await
}

#[cfg(feature = "native-tokio")]
fn memory_store() -> Result<(ComponentRef, Arc<dyn JournalStore>), AgentRunError> {
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
    Ok((component("python.store.memory")?, store))
}

#[cfg(feature = "native-tokio")]
fn component(id: &str) -> Result<ComponentRef, AgentRunError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        Some(super::PREVIEW_ENGINE_VERSION),
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
fn secret_configuration_error() -> AgentRunError {
    AgentRunError::configuration(
        AGENT_RUN_INVALID_CONFIGURATION,
        "provider secret is invalid",
    )
}

#[cfg(feature = "native-tokio")]
fn gateway_authentication(
    kind: Option<&str>,
    api_key: Option<String>,
) -> Result<finstack_ai_runtime::Authentication, AgentRunError> {
    use finstack_ai_runtime::{Authentication, SecretString};

    match (kind, api_key) {
        (None | Some("none"), None) => Ok(Authentication::None),
        (None | Some("bearer"), Some(key)) => Ok(Authentication::Bearer(
            SecretString::try_new(key).map_err(|_| secret_configuration_error())?,
        )),
        (Some("api_key"), Some(key)) => Ok(Authentication::ApiKey(
            SecretString::try_new(key).map_err(|_| secret_configuration_error())?,
        )),
        (Some("none"), Some(_)) => Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway auth none does not accept an API key",
        )),
        (Some("bearer" | "api_key"), None) => Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway bearer and api_key auth require an API key",
        )),
        (Some(_), _) => Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway auth must be none, bearer, or api_key",
        )),
    }
}

#[cfg(feature = "native-tokio")]
fn require_https_or_loopback(endpoint: &str) -> Result<(), AgentRunError> {
    let Some(rest) = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("HTTP://"))
    else {
        return Ok(());
    };
    let host = rest
        .split(['/', ':', '?'])
        .next()
        .unwrap_or("")
        .trim_start_matches('[')
        .trim_end_matches(']');
    if matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Ok(());
    }
    Err(AgentRunError::configuration(
        AGENT_RUN_INVALID_CONFIGURATION,
        "plaintext HTTP is allowed only for loopback endpoints",
    ))
}

#[cfg(feature = "native-tokio")]
fn require_https_for_credentials(endpoint: &str, has_secret: bool) -> Result<(), AgentRunError> {
    if !has_secret {
        return Ok(());
    }
    if endpoint.len() < 8 || !endpoint[..8].eq_ignore_ascii_case("https://") {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "provider credentials require HTTPS",
        ));
    }
    Ok(())
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

#[cfg(feature = "native-tokio")]
struct E2bCatalogModel {
    name: ModelName,
}

#[cfg(feature = "native-tokio")]
impl Model for E2bCatalogModel {
    fn descriptor(&self) -> finstack_ai_runtime::ModelDescriptor {
        finstack_ai_runtime::ModelDescriptor {
            provider: Arc::from("e2b"),
            models: Arc::from([self.name.clone()]),
            metadata: finstack_ai_kernel::Metadata::empty(),
        }
    }

    fn capabilities(&self, model: &ModelName) -> finstack_ai_runtime::ModelCapabilities {
        use std::collections::BTreeSet;

        use finstack_ai_runtime::{
            InputCapabilities, ModelCapabilities, ModelContextProfile, StructuredOutputCapability,
            TokenEstimatorRef, TokenEstimatorSource,
        };

        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: ModelContextProfile {
                provider: Arc::from("e2b"),
                model: model.clone(),
                hard_input_bytes: LINKED_CONTEXT_WINDOW_TOKENS,
                context_window_tokens: LINKED_CONTEXT_WINDOW_TOKENS,
                max_output_tokens: LINKED_RESERVED_OUTPUT_TOKENS,
                reserved_output_tokens: LINKED_RESERVED_OUTPUT_TOKENS,
                provider_overhead_tokens: LINKED_PROVIDER_OVERHEAD_TOKENS,
                estimator: TokenEstimatorRef {
                    id: Arc::from("e2b.utf8-byte-upper-bound"),
                    version: Arc::from("1"),
                    source: TokenEstimatorSource::ConservativeUpperBound,
                },
            },
            native_tool_calls: true,
            parallel_tool_calls: false,
            structured_output: StructuredOutputCapability::Unsupported,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities: BTreeSet::new(),
        }
    }

    fn estimate_input_tokens(
        &self,
        _model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<finstack_ai_runtime::ModelTokenEstimate, finstack_ai_runtime::ModelError> {
        use finstack_ai_runtime::{ModelTokenEstimate, TokenEstimatorRef, TokenEstimatorSource};

        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len()).unwrap_or(u64::MAX),
            estimator: TokenEstimatorRef {
                id: Arc::from("e2b.utf8-byte-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        })
    }

    fn request(
        &self,
        _request: finstack_ai_runtime::ModelRequest,
    ) -> finstack_ai_runtime::PortFuture<
        Result<finstack_ai_runtime::ModelEventStream, finstack_ai_runtime::ModelError>,
    > {
        use finstack_ai_kernel::ErrorCategory;
        use finstack_ai_runtime::{MODEL_REQUEST_INVALID, ModelError};

        Box::pin(async {
            Err(ModelError::try_new(
                MODEL_REQUEST_INVALID,
                ErrorCategory::Validation,
                false,
                "e2b_sandbox registers the T4 toolset and does not invoke a model",
                finstack_ai_kernel::Metadata::empty(),
            )
            .unwrap_or_else(Into::into))
        })
    }
}

#[cfg(all(test, feature = "native-tokio"))]
mod tests {
    use super::*;

    fn common() -> LinkedCommon {
        LinkedCommon::default()
    }

    #[tokio::test]
    async fn openai_constructs_without_a_network_request() {
        let built = Agent::openai(OpenAiAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-openai-secret-canary-056".into(),
            reasoning_effort: None,
            reasoning_summary: None,
            common: LinkedCommon {
                instruction: Some("Answer concisely.".into()),
                ..common()
            },
        })
        .await
        .expect("openai construct");
        assert!(built.agent.capability_catalog().is_empty());
        assert_eq!(built.default_timeout, OPENAI_TIMEOUT);
        let resolved = built.agent.re_resolve().await.expect("re_resolve");
        assert!(resolved.capability_catalog().is_empty());
    }

    #[tokio::test]
    async fn openai_rejects_unknown_reasoning_effort() {
        let error = Agent::openai(OpenAiAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-openai-secret-canary-056".into(),
            reasoning_effort: Some("turbo".into()),
            reasoning_summary: None,
            common: common(),
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
            reasoning_effort: None,
            reasoning_summary: None,
            common: common(),
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
            common: common(),
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
            common: common(),
        })
        .await
        .expect("ollama construct");
        assert!(built.agent.capability_catalog().is_empty());
    }

    fn gateway_spec() -> GatewayAgentSpec {
        GatewayAgentSpec {
            endpoint: "https://api.example.test/v1/responses".into(),
            model: "fixture-model".into(),
            wire_protocol: "openai_responses".into(),
            credential_name: "prod".into(),
            hard_input_bytes: Some(1_000_000),
            auth_kind: Some("bearer".into()),
            api_key: Some("sk-gateway-secret-canary-045".into()),
            common: common(),
        }
    }

    #[tokio::test]
    async fn gateway_constructs_without_a_network_request() {
        let built = Agent::gateway(gateway_spec())
            .await
            .expect("gateway construct");
        assert!(built.agent.capability_catalog().is_empty());
        assert_eq!(built.default_timeout, OPENAI_TIMEOUT);
    }

    #[tokio::test]
    async fn gateway_rejects_missing_hard_input_bytes() {
        let mut spec = gateway_spec();
        spec.hard_input_bytes = None;
        let error = Agent::gateway(spec)
            .await
            .err()
            .expect("missing hard_input_bytes");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("hard_input_bytes"));
    }

    #[tokio::test]
    async fn gateway_rejects_plaintext_non_loopback() {
        let mut spec = gateway_spec();
        spec.endpoint = "http://api.example.test/v1/responses".into();
        spec.api_key = None;
        spec.auth_kind = Some("none".into());
        let error = Agent::gateway(spec)
            .await
            .err()
            .expect("plaintext non-loopback");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("plaintext HTTP"));
    }

    #[tokio::test]
    async fn gateway_http_credentials_fail_closed_without_leaking_the_canary() {
        let canary = "sk-gateway-secret-canary-045";
        let mut spec = gateway_spec();
        spec.endpoint = "http://127.0.0.1:9/v1/responses".into();
        spec.api_key = Some(canary.into());
        let error = Agent::gateway(spec).await.err().expect("http + key");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("HTTPS"));
        assert!(!error.to_string().contains(canary));
    }

    #[tokio::test]
    async fn gateway_rejects_openai_chat() {
        let mut spec = gateway_spec();
        spec.wire_protocol = "openai_chat".into();
        let error = Agent::gateway(spec).await.err().expect("openai_chat");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("openai_chat"));
    }

    fn e2b_spec() -> E2bSandboxAgentSpec {
        E2bSandboxAgentSpec {
            model: "fixture-model".into(),
            api_key: "e2b-secret-canary-045".into(),
            endpoint: Some("https://api.e2b.dev".into()),
            template: None,
            common: common(),
        }
    }

    #[tokio::test]
    async fn e2b_sandbox_constructs_without_a_network_request() {
        let built = Agent::e2b_sandbox(e2b_spec()).await.expect("e2b construct");
        assert!(built.agent.capability_catalog().is_empty());
    }

    #[tokio::test]
    async fn e2b_sandbox_rejects_a_missing_api_key() {
        let mut spec = e2b_spec();
        spec.api_key.clear();
        let error = Agent::e2b_sandbox(spec).await.err().expect("missing key");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("API key"));
    }

    #[tokio::test]
    async fn e2b_sandbox_rejects_plaintext_non_loopback() {
        let mut spec = e2b_spec();
        spec.endpoint = Some("http://8.8.8.8".into());
        let error = Agent::e2b_sandbox(spec).await.err().expect("plaintext");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("plaintext HTTP"));
        assert!(!error.to_string().contains("e2b-secret-canary-045"));
    }
}
