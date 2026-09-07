//! Rust-owned linked-provider constructors.
//!
//! Python and WASM only map arguments. `wasm-host` methods exist and return
//! [`crate::AGENT_RUN_UNSUPPORTED_PLAN`]. Constructors never read ambient env.

use std::sync::Arc;
use std::time::Duration;

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use finstack_ai_kernel::ComponentId;
use finstack_ai_kernel::{CapabilityId, ComponentRef, RawJson};
use finstack_ai_runtime::artifact::ArtifactStore;
use finstack_ai_runtime::ports::context::ContextProvider;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::middleware::Middleware;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use finstack_ai_runtime::ports::model::Model;
use finstack_ai_runtime::ports::model::{ModelName, ModelSettings};
use finstack_ai_runtime::ports::observer::Observer;
use finstack_ai_runtime::ports::tool::Toolset;

use crate::{ApprovalGrantMode, CapabilitySpec, ChildRunPolicy};

use super::handle::Agent;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use super::linked_composition::build_linked_provider;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use super::types::AGENT_RUN_INVALID_CONFIGURATION;
use super::types::AGENT_RUN_UNSUPPORTED_PLAN;
use super::types::AgentRunError;

#[cfg(any(
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
const OPENAI_TIMEOUT: Duration = Duration::from_mins(2);
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
const LINKED_CONTEXT_WINDOW_TOKENS: u64 = 1_050_000;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-ollama"
))]
const LINKED_RESERVED_OUTPUT_TOKENS: u64 = 128_000;
#[cfg(any(feature = "provider-anthropic", feature = "provider-gemini"))]
const LINKED_ANTHROPIC_OUTPUT_TOKENS: u64 = 64_000;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
const LINKED_PROVIDER_OVERHEAD_TOKENS: u64 = 64;
#[cfg(any(feature = "provider-openai", feature = "provider-openrouter"))]
const REASONING_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
#[cfg(any(feature = "provider-openai", feature = "provider-openrouter"))]
const REASONING_SUMMARIES: &[&str] = &["auto", "concise", "detailed"];

/// Already-resolved port handles supplied by a language binding.
#[derive(Default)]
pub struct LinkedAgentPorts {
    /// Toolset registrations.
    pub toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    /// Context-provider registrations.
    pub context_providers: Vec<Arc<dyn ContextProvider>>,
    /// Middleware registrations.
    pub middleware: Vec<Arc<dyn Middleware>>,
    /// Observer registrations.
    pub observers: Vec<Arc<dyn Observer>>,
    /// Store for toolsets that stage bytes instead of inlining them. Without
    /// it, generated audio and images travel as base64 the model cannot use.
    pub artifact_store: Option<Arc<dyn ArtifactStore>>,
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
    /// Explicit versioned journal registration. Defaults to a bounded memory store
    /// in provider factories; overrides the builder store in `build_linked`.
    pub journal_store: Option<(ComponentRef, Arc<dyn JournalStore>)>,
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
    /// Paid-tool approval grant mode. Bindings default this to `PerCall`.
    pub approval_grant: ApprovalGrantMode,
}

/// Provider selection for [`Agent::linked`], the single linked-provider
/// construction path.
///
/// Each variant carries the full typed spec for one wire protocol. All
/// variants funnel into the same composition pipeline
/// ([`super::NativeAgentBuilder::build_linked`]), so provider choice never changes
/// registration, policy, or output-schema semantics.
pub enum LinkedProviderSpec {
    /// Official `OpenAI` Responses agent (always `https://api.openai.com`).
    OpenAi(OpenAiAgentSpec),
    /// `OpenRouter` Responses agent.
    OpenRouter(OpenRouterAgentSpec),
    /// Anthropic Messages agent.
    Anthropic(AnthropicAgentSpec),
    /// Gemini `generateContent` agent.
    Gemini(GeminiAgentSpec),
    /// Keyless Ollama `/api/chat` agent.
    Ollama(OllamaAgentSpec),
    /// Config-driven gateway agent dispatching onto one of the dedicated
    /// providers.
    Gateway(GatewayAgentSpec),
}

/// Arguments for [`LinkedProviderSpec::OpenAi`].
pub struct OpenAiAgentSpec {
    /// Responses model name.
    pub model: String,
    /// API key. Required: the endpoint accepts no anonymous calls.
    pub api_key: String,
    /// Optional Responses reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Optional Responses reasoning summary.
    pub reasoning_summary: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`LinkedProviderSpec::OpenRouter`].
pub struct OpenRouterAgentSpec {
    /// `OpenRouter` model identifier (e.g. `openai/gpt-5`; `:nitro` and
    /// `:floor` routing suffixes are allowed).
    pub model: String,
    /// API key. Required: the endpoint accepts no anonymous calls.
    pub api_key: String,
    /// Optional non-secret `HTTP-Referer` attribution header.
    pub referer: Option<String>,
    /// Optional non-secret `X-Title` attribution header.
    pub title: Option<String>,
    /// Optional Responses reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Optional Responses reasoning summary.
    pub reasoning_summary: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`LinkedProviderSpec::Anthropic`].
pub struct AnthropicAgentSpec {
    /// Messages API base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Optional API key. HTTPS is required when set; a loopback base URL
    /// may run without one.
    pub api_key: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`LinkedProviderSpec::Gemini`].
pub struct GeminiAgentSpec {
    /// Gemini `generateContent` base URL (Generative Language API).
    pub endpoint: String,
    /// Model name.
    pub model: String,
    /// Optional API key. HTTPS is required when set; a loopback endpoint
    /// may run without one.
    pub api_key: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`LinkedProviderSpec::Ollama`].
pub struct OllamaAgentSpec {
    // No `api_key`: Ollama serves locally and takes no credential.
    /// Native `/api/chat` base URL.
    pub base_url: String,
    /// Model name.
    pub model: String,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

/// Arguments for [`LinkedProviderSpec::Gateway`].
pub struct GatewayAgentSpec {
    /// Provider endpoint URL. HTTPS is required off loopback.
    pub endpoint: String,
    /// Configured model name.
    pub model: String,
    /// Wire protocol: `openai_responses`, `anthropic_messages`, `ollama_chat`,
    /// or `gemini_generate_content`. `openai_chat` is a configuration error.
    pub wire_protocol: String,
    /// Named credential reference. Never a secret literal.
    pub credential_name: String,
    /// Maximum canonical request bytes. Construction fails when omitted.
    pub hard_input_bytes: Option<u64>,
    /// Auth scheme: `none`, `bearer`, or `api_key`. Defaults from `api_key`.
    pub auth_kind: Option<String>,
    /// Optional API key. HTTPS is required when set; a gateway may instead
    /// authenticate through `auth_kind` and `credential_name`.
    pub api_key: Option<String>,
    /// Shared instruction, ports, and child-run policy.
    pub common: LinkedCommon,
}

impl Agent {
    /// Construct a linked-provider agent. This is the single Rust-owned
    /// construction path for every wire protocol; language bindings only map
    /// arguments into a [`LinkedProviderSpec`] variant.
    ///
    /// Never reads environment variables. Endpoint and credential rules are
    /// per variant:
    ///
    /// - [`LinkedProviderSpec::OpenAi`] always targets
    ///   `https://api.openai.com`.
    /// - [`LinkedProviderSpec::OpenRouter`] always targets
    ///   `https://openrouter.ai/api/v1/responses`. It does not attach a
    ///   [`finstack_ai_runtime::ports::model::MediaResolver`]: vision, file,
    ///   and audio input require a host-built provider with
    ///   `with_media_resolver`. Outbound media toolsets are composed separately.
    /// - [`LinkedProviderSpec::Anthropic`] and [`LinkedProviderSpec::Gemini`]
    ///   require HTTPS when `api_key` is set; a loopback endpoint may run
    ///   without one.
    /// - [`LinkedProviderSpec::Ollama`] is keyless and local.
    /// - [`LinkedProviderSpec::Gateway`] dispatches onto the dedicated
    ///   openai, anthropic, ollama, or gemini provider under `native-tokio`
    ///   only; HTTPS is required off loopback and whenever a credential is
    ///   set, and `openai_chat` is a configuration error.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_UNSUPPORTED_PLAN`] on `wasm-host` or when
    /// the selected `provider-*` feature is disabled. Returns
    /// [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when the endpoint, model,
    /// credential pairing, attribution, reasoning, or gateway route is
    /// invalid.
    pub async fn linked(spec: LinkedProviderSpec) -> Result<LinkedAgent, AgentRunError> {
        match spec {
            LinkedProviderSpec::OpenAi(spec) => openai_inner(spec).await,
            LinkedProviderSpec::OpenRouter(spec) => openrouter_inner(spec).await,
            LinkedProviderSpec::Anthropic(spec) => anthropic_inner(spec).await,
            LinkedProviderSpec::Gemini(spec) => gemini_inner(spec).await,
            LinkedProviderSpec::Ollama(spec) => ollama_inner(spec).await,
            LinkedProviderSpec::Gateway(spec) => gateway_inner(spec).await,
        }
    }
}

#[cfg(feature = "provider-openai")]
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
        ))
        .map_err(|error| model_configuration_error(&error))?;
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
    let provider = arc_model(OpenAiProvider::try_new, config, model_config)?;
    let common = spec.common;
    build_linked_provider(
        (
            "python.agent.openai",
            "python.bundle.openai",
            "python.model.openai",
        ),
        provider,
        model_name,
        common,
        settings,
        OPENAI_TIMEOUT,
    )
    .await
}

#[cfg(feature = "provider-openrouter")]
async fn openrouter_inner(spec: OpenRouterAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_provider_openrouter::{
        Authentication, OpenRouterConfig, OpenRouterModelConfig, OpenRouterProvider, SecretString,
    };

    let settings = reasoning_settings(
        spec.reasoning_effort.as_deref(),
        spec.reasoning_summary.as_deref(),
    )?;
    let config = OpenRouterConfig::try_new("https://openrouter.ai")
        .map_err(|error| model_configuration_error(&error))?
        .with_authentication(Authentication::Bearer(
            SecretString::try_new(spec.api_key).map_err(|_| secret_configuration_error())?,
        ))
        .map_err(|error| model_configuration_error(&error))?
        .with_attribution(spec.referer.as_deref(), spec.title.as_deref())
        .map_err(|error| model_configuration_error(&error))?;
    let model_config = OpenRouterModelConfig::try_new(
        &spec.model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_PROVIDER_OVERHEAD_TOKENS,
    )
    .map_err(|error| model_configuration_error(&error))?
    .with_reasoning(true);
    let model_name = model_config.name().clone();
    let provider = arc_model(OpenRouterProvider::try_new, config, model_config)?;
    let common = spec.common;
    build_linked_provider(
        (
            "python.agent.openrouter",
            "python.bundle.openrouter",
            "python.model.openrouter",
        ),
        provider,
        model_name,
        common,
        settings,
        OPENAI_TIMEOUT,
    )
    .await
}

#[cfg(feature = "provider-anthropic")]
async fn anthropic_inner(spec: AnthropicAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_provider_anthropic::{
        AnthropicConfig, AnthropicModelConfig, AnthropicProvider, Authentication, SecretString,
    };

    let mut config = AnthropicConfig::try_new(spec.base_url)
        .map_err(|error| model_configuration_error(&error))?;
    if let Some(api_key) = spec.api_key {
        config = config
            .with_authentication(Authentication::ApiKey(
                SecretString::try_new(api_key).map_err(|_| secret_configuration_error())?,
            ))
            .map_err(|error| model_configuration_error(&error))?;
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
    let provider = arc_model(AnthropicProvider::try_new, config, model_config)?;
    let common = spec.common;
    build_linked_provider(
        (
            "python.agent.anthropic",
            "python.bundle.anthropic",
            "python.model.anthropic",
        ),
        provider,
        model_name,
        common,
        empty_model_settings()?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(feature = "provider-gemini")]
async fn gemini_inner(spec: GeminiAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_provider_gemini::{
        Authentication, GeminiConfig, GeminiModelConfig, GeminiProvider, SecretString,
    };

    let model_name = parse_model_name(&spec.model)?;
    let mut config =
        GeminiConfig::try_new(spec.endpoint).map_err(|error| model_configuration_error(&error))?;
    if let Some(api_key) = spec.api_key {
        config = config
            .with_authentication(Authentication::ApiKey(
                SecretString::try_new(api_key).map_err(|_| secret_configuration_error())?,
            ))
            .map_err(|error| model_configuration_error(&error))?;
    }
    let model_config = GeminiModelConfig::try_new(
        &spec.model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_ANTHROPIC_OUTPUT_TOKENS,
    )
    .map_err(|error| model_configuration_error(&error))?
    .with_provider_overhead_tokens(LINKED_PROVIDER_OVERHEAD_TOKENS);
    let provider = arc_model(GeminiProvider::try_new, config, model_config)?;
    let common = spec.common;
    build_linked_provider(
        (
            "python.agent.gemini",
            "python.bundle.gemini",
            "python.model.gemini",
        ),
        provider,
        model_name,
        common,
        empty_model_settings()?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(feature = "provider-ollama")]
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
    let provider = arc_model(OllamaProvider::try_new, config, model_config)?;
    let common = spec.common;
    build_linked_provider(
        (
            "python.agent.ollama",
            "python.bundle.ollama",
            "python.model.ollama",
        ),
        provider,
        model_name,
        common,
        empty_model_settings()?,
        DEFAULT_TIMEOUT,
    )
    .await
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
async fn gateway_inner(spec: GatewayAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    use finstack_ai_runtime::ports::model::{Authentication, CredentialReference, CredentialStore};

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
        "anthropic_messages" | "gemini_generate_content" => match authentication {
            Authentication::Bearer(secret) => Authentication::ApiKey(secret),
            other => other,
        },
        _ => {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "gateway wire_protocol must be openai_responses, anthropic_messages, ollama_chat, or gemini_generate_content",
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
    build_linked_provider(
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

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
#[allow(
    unused_variables,
    unreachable_patterns,
    clippy::needless_pass_by_value,
    reason = "gateway routes are selected by individual provider features"
)]
fn gateway_provider(
    wire_protocol: &str,
    endpoint: &str,
    model: &str,
    hard_input_bytes: u64,
    store: finstack_ai_runtime::ports::model::CredentialStore,
    reference: finstack_ai_runtime::ports::model::CredentialReference,
) -> Result<(Arc<dyn Model>, ModelName), AgentRunError> {
    match wire_protocol {
        #[cfg(feature = "provider-openai")]
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
                arc_model(OpenAiProvider::try_new, config, model)?,
                model_name,
            ))
        }
        #[cfg(feature = "provider-anthropic")]
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
                arc_model(AnthropicProvider::try_new, config, model)?,
                model_name,
            ))
        }
        #[cfg(feature = "provider-ollama")]
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
                arc_model(OllamaProvider::try_new, config, model)?,
                model_name,
            ))
        }
        #[cfg(feature = "provider-gemini")]
        "gemini_generate_content" => {
            gateway_gemini_provider(endpoint, model, hard_input_bytes, store, reference)
        }
        "openai_responses" | "anthropic_messages" | "ollama_chat" | "gemini_generate_content" => {
            Err(AgentRunError::configuration(
                AGENT_RUN_UNSUPPORTED_PLAN,
                "gateway route requires its individual provider feature",
            ))
        }
        _ => Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "gateway wire_protocol must be openai_responses, anthropic_messages, ollama_chat, or gemini_generate_content",
        )),
    }
}

#[cfg(feature = "provider-gemini")]
fn gateway_gemini_provider(
    endpoint: &str,
    model: &str,
    hard_input_bytes: u64,
    store: finstack_ai_runtime::ports::model::CredentialStore,
    reference: finstack_ai_runtime::ports::model::CredentialReference,
) -> Result<(Arc<dyn Model>, ModelName), AgentRunError> {
    use finstack_ai_provider_gemini::{GeminiConfig, GeminiModelConfig, GeminiProvider};

    let config = GeminiConfig::try_new(endpoint)
        .map_err(|error| model_configuration_error(&error))?
        .with_credentials(store, reference);
    let model_name = parse_model_name(model)?;
    let model = GeminiModelConfig::try_new(
        model,
        hard_input_bytes,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_ANTHROPIC_OUTPUT_TOKENS,
    )
    .map_err(|error| model_configuration_error(&error))?;
    Ok((
        arc_model(GeminiProvider::try_new, config, model)?,
        model_name,
    ))
}

#[cfg(not(feature = "provider-openai"))]
async fn openai_inner(spec: OpenAiAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported_provider(spec, "openai").await
}

#[cfg(not(feature = "provider-openrouter"))]
async fn openrouter_inner(spec: OpenRouterAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported_provider(spec, "openrouter").await
}

#[cfg(not(feature = "provider-anthropic"))]
async fn anthropic_inner(spec: AnthropicAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported_provider(spec, "anthropic").await
}

#[cfg(not(feature = "provider-gemini"))]
async fn gemini_inner(spec: GeminiAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported_provider(spec, "gemini").await
}

#[cfg(not(feature = "provider-ollama"))]
async fn ollama_inner(spec: OllamaAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported_provider(spec, "ollama").await
}

#[cfg(not(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
)))]
async fn gateway_inner(spec: GatewayAgentSpec) -> Result<LinkedAgent, AgentRunError> {
    unsupported_provider(spec, "gateway").await
}

#[cfg(not(all(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
)))]
#[expect(
    clippy::unused_async,
    reason = "unsupported linked features keep the same async signature"
)]
async fn unsupported_provider<T>(
    _spec: T,
    name: &'static str,
) -> Result<LinkedAgent, AgentRunError> {
    let requirement = if name == "gateway" {
        "a gateway route's provider-* feature".to_owned()
    } else {
        format!("the provider-{name} feature")
    };
    Err(AgentRunError::configuration(
        AGENT_RUN_UNSUPPORTED_PLAN,
        format!("Agent::linked({name}) requires {requirement}"),
    ))
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
pub(super) fn component(id: &str) -> Result<ComponentRef, AgentRunError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        Some(super::PREVIEW_ENGINE_VERSION),
    ))
}

/// Construct one provider and erase it to `Arc<dyn Model>`.
///
/// Single finish path for every linked and gateway provider so the
/// construct-wrap-map sequence cannot drift between providers.
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn arc_model<C, M, P>(
    ctor: fn(C, Vec<M>) -> Result<P, finstack_ai_runtime::ports::model::ModelError>,
    config: C,
    model_config: M,
) -> Result<Arc<dyn Model>, AgentRunError>
where
    P: Model + 'static,
{
    Ok(Arc::new(
        ctor(config, vec![model_config]).map_err(|error| model_configuration_error(&error))?,
    ))
}

/// Parse a provider model name into the shared error shape.
#[cfg(feature = "provider-gemini")]
fn parse_model_name(model: &str) -> Result<ModelName, AgentRunError> {
    ModelName::try_new(model).map_err(|error| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            format!("{}: {}", error.code(), error.message()),
        )
    })
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn model_configuration_error(
    error: &finstack_ai_runtime::ports::model::ModelError,
) -> AgentRunError {
    AgentRunError::configuration(
        AGENT_RUN_INVALID_CONFIGURATION,
        format!("{}: {}", error.code(), error.message()),
    )
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn secret_configuration_error() -> AgentRunError {
    AgentRunError::configuration(
        AGENT_RUN_INVALID_CONFIGURATION,
        "provider secret is invalid",
    )
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn gateway_authentication(
    kind: Option<&str>,
    api_key: Option<String>,
) -> Result<finstack_ai_runtime::ports::model::Authentication, AgentRunError> {
    use finstack_ai_runtime::ports::model::{Authentication, SecretString};

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

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn require_https_or_loopback(endpoint: &str) -> Result<(), AgentRunError> {
    let parsed = url::Url::parse(endpoint).map_err(|_| {
        AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "provider endpoint must be a valid http or https URL",
        )
    })?;
    if parsed.username() != "" || parsed.password().is_some() || parsed.host().is_none() {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "provider endpoint must not contain userinfo and must include a host",
        ));
    }
    if parsed.scheme().eq_ignore_ascii_case("https") {
        return Ok(());
    }
    if parsed.scheme().eq_ignore_ascii_case("http")
        && parsed.host().is_some_and(|host| match host {
            url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        })
    {
        return Ok(());
    }
    Err(AgentRunError::configuration(
        AGENT_RUN_INVALID_CONFIGURATION,
        "plaintext HTTP is allowed only for loopback endpoints",
    ))
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn require_https_for_credentials(endpoint: &str, has_secret: bool) -> Result<(), AgentRunError> {
    if !has_secret {
        return Ok(());
    }
    let is_https = url::Url::parse(endpoint)
        .ok()
        .is_some_and(|url| url.scheme().eq_ignore_ascii_case("https"));
    if !is_https {
        return Err(AgentRunError::configuration(
            AGENT_RUN_INVALID_CONFIGURATION,
            "provider credentials require HTTPS",
        ));
    }
    Ok(())
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn empty_model_settings() -> Result<ModelSettings, AgentRunError> {
    Ok(ModelSettings {
        values: RawJson::parse(b"{}").map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
    })
}

#[cfg(any(feature = "provider-openai", feature = "provider-openrouter"))]
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

#[cfg(all(test, feature = "linked-providers"))]
mod tests {
    use super::*;

    pub(super) fn common() -> LinkedCommon {
        LinkedCommon::default()
    }

    #[tokio::test]
    async fn openai_constructs_without_a_network_request() {
        let built = Agent::linked(LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-openai-secret-canary-056".into(),
            reasoning_effort: None,
            reasoning_summary: None,

            common: LinkedCommon {
                instruction: Some("Answer concisely.".into()),
                ..common()
            },
        }))
        .await
        .expect("openai construct");
        assert!(built.agent.capability_catalog().is_empty());
        assert_eq!(built.default_timeout, OPENAI_TIMEOUT);
        let resolved = built.agent.re_resolve().await.expect("re_resolve");
        assert!(resolved.capability_catalog().is_empty());
    }

    #[tokio::test]
    async fn openai_rejects_unknown_reasoning_effort() {
        let error = Agent::linked(LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
            model: "fixture-model".into(),
            api_key: "sk-openai-secret-canary-056".into(),
            reasoning_effort: Some("turbo".into()),
            reasoning_summary: None,

            common: LinkedCommon { ..common() },
        }))
        .await
        .err()
        .expect("unknown effort");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("reasoning_effort"));
    }

    #[tokio::test]
    async fn openai_empty_model_does_not_leak_the_api_key() {
        let canary = "sk-openai-secret-canary-056";
        let error = Agent::linked(LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
            model: String::new(),
            api_key: canary.into(),
            reasoning_effort: None,
            reasoning_summary: None,

            common: LinkedCommon { ..common() },
        }))
        .await
        .err()
        .expect("empty model");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(!error.to_string().contains(canary));
    }

    #[tokio::test]
    async fn openrouter_constructs_without_a_network_request() {
        let built = Agent::linked(LinkedProviderSpec::OpenRouter(OpenRouterAgentSpec {
            model: "openai/gpt-5".into(),
            api_key: "sk-or-secret-canary-101".into(),
            referer: Some("https://example.app".into()),
            title: Some("Example App".into()),
            reasoning_effort: None,
            reasoning_summary: None,

            common: LinkedCommon {
                instruction: Some("Answer concisely.".into()),
                ..common()
            },
        }))
        .await
        .expect("openrouter construct");
        assert!(built.agent.capability_catalog().is_empty());
        assert_eq!(built.default_timeout, OPENAI_TIMEOUT);
    }

    #[tokio::test]
    async fn openrouter_invalid_attribution_does_not_leak_the_api_key() {
        let canary = "sk-or-secret-canary-101";
        let error = Agent::linked(LinkedProviderSpec::OpenRouter(OpenRouterAgentSpec {
            model: "openai/gpt-5".into(),
            api_key: canary.into(),
            referer: Some("bad\nreferer".into()),
            title: None,
            reasoning_effort: None,
            reasoning_summary: None,

            common: common(),
        }))
        .await
        .err()
        .expect("invalid attribution");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(!error.to_string().contains(canary));
    }

    #[tokio::test]
    async fn openrouter_rejects_unknown_reasoning_effort() {
        let error = Agent::linked(LinkedProviderSpec::OpenRouter(OpenRouterAgentSpec {
            model: "openai/gpt-5".into(),
            api_key: "sk-or-secret-canary-101".into(),
            referer: None,
            title: None,
            reasoning_effort: Some("turbo".into()),
            reasoning_summary: None,

            common: common(),
        }))
        .await
        .err()
        .expect("unknown effort");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("reasoning_effort"));
    }

    #[tokio::test]
    async fn anthropic_http_credentials_fail_closed_without_leaking_the_canary() {
        let canary = "sk-ant-secret-canary-055";
        let error = Agent::linked(LinkedProviderSpec::Anthropic(AnthropicAgentSpec {
            base_url: "http://127.0.0.1:9".into(),
            model: "fixture-model".into(),
            api_key: Some(canary.into()),
            common: LinkedCommon { ..common() },
        }))
        .await
        .err()
        .expect("http + key");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(!error.to_string().contains(canary));
    }

    #[tokio::test]
    async fn gemini_constructs_without_a_network_request() {
        let built = Agent::linked(LinkedProviderSpec::Gemini(GeminiAgentSpec {
            endpoint: "http://127.0.0.1:9".into(),
            model: "fixture-model".into(),
            api_key: None,
            common: LinkedCommon {
                instruction: Some("Answer concisely.".into()),
                ..common()
            },
        }))
        .await
        .expect("gemini construct");
        assert!(built.agent.capability_catalog().is_empty());
        assert_eq!(built.default_timeout, DEFAULT_TIMEOUT);
        let component = built
            .agent
            .resolved()
            .run_plan()
            .model()
            .descriptor()
            .component
            .clone();
        assert_eq!(component.id().as_str(), "python.model.gemini");
    }

    #[tokio::test]
    async fn gemini_http_credentials_fail_closed_without_leaking_the_canary() {
        let canary = "AIza-secret-canary-055";
        let error = Agent::linked(LinkedProviderSpec::Gemini(GeminiAgentSpec {
            endpoint: "http://127.0.0.1:9".into(),
            model: "fixture-model".into(),
            api_key: Some(canary.into()),
            common: LinkedCommon { ..common() },
        }))
        .await
        .err()
        .expect("http + key");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(!error.to_string().contains(canary));
    }

    #[tokio::test]
    async fn ollama_constructs_without_a_network_request() {
        let built = Agent::linked(LinkedProviderSpec::Ollama(OllamaAgentSpec {
            base_url: "http://127.0.0.1:11434".into(),
            model: "fixture-model".into(),
            common: LinkedCommon { ..common() },
        }))
        .await
        .expect("ollama construct");
        assert!(built.agent.capability_catalog().is_empty());
    }

    pub(super) fn gateway_spec() -> GatewayAgentSpec {
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
        let built = Agent::linked(LinkedProviderSpec::Gateway(gateway_spec()))
            .await
            .expect("gateway construct");
        assert!(built.agent.capability_catalog().is_empty());
        assert_eq!(built.default_timeout, OPENAI_TIMEOUT);
    }

    #[tokio::test]
    async fn gateway_rejects_missing_hard_input_bytes() {
        let mut spec = gateway_spec();
        spec.hard_input_bytes = None;
        let error = Agent::linked(LinkedProviderSpec::Gateway(spec))
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
        let error = Agent::linked(LinkedProviderSpec::Gateway(spec))
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
        let error = Agent::linked(LinkedProviderSpec::Gateway(spec))
            .await
            .err()
            .expect("http + key");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("HTTPS"));
        assert!(!error.to_string().contains(canary));
    }

    #[test]
    fn gateway_endpoint_policy_accepts_only_loopback_http() {
        for endpoint in [
            "http://localhost:8080/v1",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:8080/v1",
            "https://api.example.test/v1",
            "HTTPS://api.example.test/v1",
        ] {
            require_https_or_loopback(endpoint).expect(endpoint);
        }
    }

    #[test]
    fn gateway_endpoint_policy_rejects_unsafe_or_malformed_urls() {
        for endpoint in [
            "http://8.8.8.8/v1",
            "http://user:pass@localhost/v1",
            "http:///missing-host",
            "ftp://localhost/v1",
            "http://[::2]/v1",
        ] {
            assert!(require_https_or_loopback(endpoint).is_err(), "{endpoint}");
        }
    }

    #[tokio::test]
    async fn gateway_gemini_generate_content_constructs_without_a_network_request() {
        let mut spec = gateway_spec();
        spec.wire_protocol = "gemini_generate_content".into();
        let built = Agent::linked(LinkedProviderSpec::Gateway(spec))
            .await
            .expect("gateway gemini construct");
        assert!(built.agent.capability_catalog().is_empty());
    }

    #[tokio::test]
    async fn gateway_rejects_a_misspelled_gemini_protocol() {
        let mut spec = gateway_spec();
        spec.wire_protocol = "gemini_generatecontent".into();
        let error = Agent::linked(LinkedProviderSpec::Gateway(spec))
            .await
            .err()
            .expect("misspelled gemini protocol stays an error");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("gemini_generate_content"));
    }

    #[tokio::test]
    async fn gateway_rejects_openai_chat() {
        let mut spec = gateway_spec();
        spec.wire_protocol = "openai_chat".into();
        let error = Agent::linked(LinkedProviderSpec::Gateway(spec))
            .await
            .err()
            .expect("openai_chat");
        assert_eq!(error.code(), AGENT_RUN_INVALID_CONFIGURATION);
        assert!(error.to_string().contains("openai_chat"));
    }
}
