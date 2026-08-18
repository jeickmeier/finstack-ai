//! Secret-safe gateway route, model, and credential configuration.

use core::fmt;
use core::net::IpAddr;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::{
    ErrorCategory, InputCapabilities, MODEL_PROFILE_INVALID, MODEL_REQUEST_INVALID, Metadata,
    ModelCapabilities, ModelContextProfile, ModelError, ModelName, StructuredOutputCapability,
    TokenEstimatorRef, secret_is_valid,
};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderValue};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);
const DEFAULT_MAX_EVENT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_STREAM_BYTES: usize = 16 * 1_048_576;
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const PROVIDER_NAME: &str = "gateway";

/// Named wire protocol selected by route configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireProtocol {
    /// Official `OpenAI` Responses SSE (`response.*` events).
    OpenaiResponses,
    /// `OpenAI` Chat Completions SSE, including the `[DONE]` sentinel.
    OpenaiChat,
    /// Anthropic Messages named SSE events.
    AnthropicMessages,
    /// Native Ollama `/api/chat` NDJSON.
    OllamaChat,
}

/// Opaque configured secret whose formatting is always redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Arc<str>);

impl SecretString {
    /// Construct a non-empty bounded secret.
    ///
    /// # Errors
    ///
    /// Returns `model_request_invalid` for an empty, oversized, or NUL-bearing value.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, ModelError> {
        let value = value.as_ref();
        if !secret_is_valid(value) {
            return Err(request_error("provider secret is invalid"));
        }
        Ok(Self(Arc::from(value)))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

/// Explicit provider authentication configuration.
#[derive(Clone, PartialEq, Eq)]
pub enum Authentication {
    /// No credential, suitable for keyless local loopback.
    None,
    /// Bearer credential (`Authorization: Bearer …`).
    Bearer(SecretString),
    /// Anthropic-style API key (`x-api-key`).
    ApiKey(SecretString),
}

impl fmt::Debug for Authentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Bearer(_) => formatter.write_str("Bearer([REDACTED])"),
            Self::ApiKey(_) => formatter.write_str("ApiKey([REDACTED])"),
        }
    }
}

impl Authentication {
    pub(crate) const fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// Named credential reference stored on a route. Never a secret literal.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialReference {
    name: Arc<str>,
}

impl CredentialReference {
    /// Construct one non-empty credential reference name.
    ///
    /// # Errors
    ///
    /// Returns `model_request_invalid` for an empty or NUL-bearing name.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_gateway::CredentialReference;
    ///
    /// let reference = CredentialReference::try_new("prod").expect("reference");
    /// assert_eq!(reference.as_str(), "prod");
    /// ```
    pub fn try_new(name: impl AsRef<str>) -> Result<Self, ModelError> {
        let name = name.as_ref();
        if name.is_empty() || name.as_bytes().contains(&0) {
            return Err(request_error("credential reference is invalid"));
        }
        Ok(Self {
            name: Arc::from(name),
        })
    }

    /// Configured reference name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for CredentialReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialReference")
            .field("name", &self.name)
            .finish()
    }
}

/// Explicit map of credential references to redacted authentication values.
///
/// The store is supplied by the host. It never reads environment variables.
#[derive(Clone, Default)]
pub struct CredentialStore {
    entries: BTreeMap<Arc<str>, Authentication>,
}

impl fmt::Debug for CredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialStore")
            .field("names", &self.entries.keys().cloned().collect::<Vec<_>>())
            .finish()
    }
}

impl CredentialStore {
    /// Construct an empty store.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Insert one named credential.
    ///
    /// # Errors
    ///
    /// Returns `model_request_invalid` for an empty or NUL-bearing name.
    pub fn insert(
        &mut self,
        name: impl AsRef<str>,
        authentication: Authentication,
    ) -> Result<(), ModelError> {
        let reference = CredentialReference::try_new(name)?;
        self.entries
            .insert(Arc::from(reference.as_str()), authentication);
        Ok(())
    }

    pub(crate) fn resolve(&self, reference: &CredentialReference) -> Option<&Authentication> {
        self.entries.get(reference.name.as_ref())
    }
}

/// Capability flags that must be supplied per configured model.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "capability flags are independent advertised provider features"
)]
pub struct GatewayCapabilityFlags {
    /// Accepted input classes.
    pub input: InputCapabilities,
    /// Native tool-call support.
    pub native_tool_calls: bool,
    /// Parallel tool-call support.
    pub parallel_tool_calls: bool,
    /// Structured-output support.
    pub structured_output: StructuredOutputCapability,
    /// Reasoning stream support.
    pub reasoning: bool,
    /// Prompt-cache support.
    pub prompt_cache: bool,
    /// Resumable stream support.
    pub resumable_stream: bool,
    /// Provider idempotency support.
    pub idempotent_requests: bool,
}

/// Config-shaped model description. Every field is required at construction.
#[derive(Debug, Clone, Default)]
pub struct GatewayModelConfig {
    /// Provider model name.
    pub name: Option<String>,
    /// Maximum canonical request bytes.
    pub hard_input_bytes: Option<u64>,
    /// Total provider context window. Never inferred.
    pub context_window_tokens: Option<u64>,
    /// Provider maximum output tokens.
    pub max_output_tokens: Option<u64>,
    /// Reserved output margin.
    pub reserved_output_tokens: Option<u64>,
    /// Conservative framing overhead.
    pub provider_overhead_tokens: Option<u64>,
    /// Bound estimator identity.
    pub estimator: Option<TokenEstimatorRef>,
    /// Advertised capability flags.
    pub capabilities: Option<GatewayCapabilityFlags>,
}

/// Validated facts for one configured gateway model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayModelSpec {
    name: ModelName,
    hard_input_bytes: u64,
    context_window_tokens: u64,
    max_output_tokens: u64,
    reserved_output_tokens: u64,
    provider_overhead_tokens: u64,
    estimator: TokenEstimatorRef,
    capabilities: GatewayCapabilityFlags,
}

impl GatewayModelSpec {
    /// Construct one model from a fully populated config object.
    ///
    /// # Errors
    ///
    /// Returns `model_profile_invalid` when any required field is missing or
    /// the numeric profile cannot form a locked context profile.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_gateway::{GatewayCapabilityFlags, GatewayModelConfig, GatewayModelSpec};
    /// use finstack_ai_runtime::{
    ///     InputCapabilities, StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource,
    /// };
    /// use std::sync::Arc;
    ///
    /// let spec = GatewayModelSpec::try_from_config(GatewayModelConfig {
    ///     name: Some("fixture-model".to_owned()),
    ///     hard_input_bytes: Some(1_000_000),
    ///     context_window_tokens: Some(8_192),
    ///     max_output_tokens: Some(1_024),
    ///     reserved_output_tokens: Some(1_024),
    ///     provider_overhead_tokens: Some(64),
    ///     estimator: Some(TokenEstimatorRef {
    ///         id: Arc::from("gateway.utf8-byte-upper-bound"),
    ///         version: Arc::from("1"),
    ///         source: TokenEstimatorSource::ConservativeUpperBound,
    ///     }),
    ///     capabilities: Some(GatewayCapabilityFlags {
    ///         input: InputCapabilities {
    ///             text: true,
    ///             json: true,
    ///             images: false,
    ///             audio: false,
    ///             files: false,
    ///         },
    ///         native_tool_calls: true,
    ///         parallel_tool_calls: false,
    ///         structured_output: StructuredOutputCapability::Unsupported,
    ///         reasoning: false,
    ///         prompt_cache: false,
    ///         resumable_stream: false,
    ///         idempotent_requests: false,
    ///     }),
    /// })
    /// .expect("spec");
    /// assert_eq!(spec.name().as_str(), "fixture-model");
    /// ```
    pub fn try_from_config(config: GatewayModelConfig) -> Result<Self, ModelError> {
        let name = config
            .name
            .ok_or_else(|| profile_error("gateway model name is required"))?;
        let hard_input_bytes = config
            .hard_input_bytes
            .ok_or_else(|| profile_error("gateway model hard_input_bytes is required"))?;
        let context_window_tokens = config
            .context_window_tokens
            .ok_or_else(|| profile_error("gateway model context_window_tokens is required"))?;
        let max_output_tokens = config
            .max_output_tokens
            .ok_or_else(|| profile_error("gateway model max_output_tokens is required"))?;
        let reserved_output_tokens = config
            .reserved_output_tokens
            .ok_or_else(|| profile_error("gateway model reserved_output_tokens is required"))?;
        let provider_overhead_tokens = config
            .provider_overhead_tokens
            .ok_or_else(|| profile_error("gateway model provider_overhead_tokens is required"))?;
        let estimator = config
            .estimator
            .ok_or_else(|| profile_error("gateway model estimator is required"))?;
        let capabilities = config
            .capabilities
            .ok_or_else(|| profile_error("gateway model capabilities are required"))?;
        let name = ModelName::try_new(name)?;
        if hard_input_bytes == 0
            || context_window_tokens == 0
            || max_output_tokens == 0
            || reserved_output_tokens == 0
            || max_output_tokens > context_window_tokens
            || reserved_output_tokens
                .checked_add(provider_overhead_tokens)
                .is_none_or(|total| total > context_window_tokens)
        {
            return Err(profile_error("gateway model context profile is invalid"));
        }
        if estimator.id.is_empty() || estimator.version.is_empty() {
            return Err(profile_error("gateway model estimator is invalid"));
        }
        Ok(Self {
            name,
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            reserved_output_tokens,
            provider_overhead_tokens,
            estimator,
            capabilities,
        })
    }

    /// Configured model name.
    #[must_use]
    pub const fn name(&self) -> &ModelName {
        &self.name
    }

    pub(crate) const fn max_output_tokens(&self) -> u64 {
        self.max_output_tokens
    }

    pub(crate) fn estimator(&self) -> TokenEstimatorRef {
        self.estimator.clone()
    }

    pub(crate) fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            input: self.capabilities.input,
            context_profile: ModelContextProfile {
                provider: Arc::from(PROVIDER_NAME),
                model: self.name.clone(),
                hard_input_bytes: self.hard_input_bytes,
                context_window_tokens: self.context_window_tokens,
                max_output_tokens: self.max_output_tokens,
                reserved_output_tokens: self.reserved_output_tokens,
                provider_overhead_tokens: self.provider_overhead_tokens,
                estimator: self.estimator.clone(),
            },
            native_tool_calls: self.capabilities.native_tool_calls,
            parallel_tool_calls: self.capabilities.parallel_tool_calls,
            structured_output: self.capabilities.structured_output,
            reasoning: self.capabilities.reasoning,
            prompt_cache: self.capabilities.prompt_cache,
            resumable_stream: self.capabilities.resumable_stream,
            idempotent_requests: self.capabilities.idempotent_requests,
            native_capabilities: BTreeSet::new(),
        }
    }
}

/// One configured gateway route. `auth` is a credential reference, never a literal.
#[derive(Clone)]
pub struct GatewayRouteConfig {
    wire_protocol: WireProtocol,
    endpoint: Arc<str>,
    auth: CredentialReference,
    request_timeout: Duration,
    max_event_bytes: usize,
    max_stream_bytes: usize,
    anthropic_version: Arc<str>,
}

impl fmt::Debug for GatewayRouteConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayRouteConfig")
            .field("wire_protocol", &self.wire_protocol)
            .field("endpoint", &self.endpoint)
            .field("auth", &self.auth)
            .field("request_timeout", &self.request_timeout)
            .field("max_event_bytes", &self.max_event_bytes)
            .field("max_stream_bytes", &self.max_stream_bytes)
            .field("anthropic_version", &self.anthropic_version)
            .finish()
    }
}

impl GatewayRouteConfig {
    /// Construct one route. HTTPS is required for non-loopback endpoints.
    ///
    /// # Errors
    ///
    /// Returns `model_request_invalid` for a URL with credentials, query,
    /// fragment, a non-HTTP scheme, or plaintext HTTP off loopback.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_gateway::{CredentialReference, GatewayRouteConfig, WireProtocol};
    ///
    /// let route = GatewayRouteConfig::try_new(
    ///     WireProtocol::OpenaiChat,
    ///     "http://127.0.0.1:9/v1/chat/completions",
    ///     CredentialReference::try_new("local").expect("reference"),
    /// )
    /// .expect("route");
    /// assert!(format!("{route:?}").contains("127.0.0.1"));
    /// ```
    pub fn try_new(
        wire_protocol: WireProtocol,
        endpoint: impl AsRef<str>,
        auth: CredentialReference,
    ) -> Result<Self, ModelError> {
        let endpoint = endpoint.as_ref();
        validate_endpoint(endpoint)?;
        Ok(Self {
            wire_protocol,
            endpoint: Arc::from(endpoint),
            auth,
            request_timeout: DEFAULT_TIMEOUT,
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
            anthropic_version: Arc::from(DEFAULT_ANTHROPIC_VERSION),
        })
    }

    /// Set the whole-request timeout.
    ///
    /// # Errors
    ///
    /// Rejects zero and durations above one hour.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Result<Self, ModelError> {
        if timeout.is_zero() || timeout > Duration::from_hours(1) {
            return Err(request_error("provider request timeout is invalid"));
        }
        self.request_timeout = timeout;
        Ok(self)
    }

    /// Set raw stream event and total response byte limits.
    ///
    /// # Errors
    ///
    /// Rejects zero limits or a total below one event.
    pub fn with_stream_limits(
        mut self,
        max_event_bytes: usize,
        max_stream_bytes: usize,
    ) -> Result<Self, ModelError> {
        if max_event_bytes == 0 || max_stream_bytes < max_event_bytes {
            return Err(request_error("provider stream limits are invalid"));
        }
        self.max_event_bytes = max_event_bytes;
        self.max_stream_bytes = max_stream_bytes;
        Ok(self)
    }

    pub(crate) const fn wire_protocol(&self) -> WireProtocol {
        self.wire_protocol
    }

    pub(crate) fn endpoint(&self) -> Result<Url, ModelError> {
        Url::parse(&self.endpoint).map_err(|_| request_error("provider endpoint is invalid"))
    }

    pub(crate) fn auth(&self) -> &CredentialReference {
        &self.auth
    }

    pub(crate) const fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub(crate) const fn max_event_bytes(&self) -> usize {
        self.max_event_bytes
    }

    pub(crate) const fn max_stream_bytes(&self) -> usize {
        self.max_stream_bytes
    }

    pub(crate) fn header_map(
        &self,
        authentication: &Authentication,
    ) -> Result<HeaderMap, ModelError> {
        let url = self.endpoint()?;
        if url.scheme() != "https" && !authentication.is_none() {
            return Err(request_error("provider credentials require HTTPS"));
        }
        let mut headers = HeaderMap::new();
        match authentication {
            Authentication::None => {}
            Authentication::Bearer(value) => {
                let mut header = HeaderValue::from_str(&format!("Bearer {}", value.expose()))
                    .map_err(|_| request_error("bearer credential is not a valid header value"))?;
                header.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, header);
            }
            Authentication::ApiKey(value) => {
                let mut header = HeaderValue::from_str(value.expose())
                    .map_err(|_| request_error("API key is not a valid header value"))?;
                header.set_sensitive(true);
                headers.insert("x-api-key", header);
            }
        }
        if self.wire_protocol == WireProtocol::AnthropicMessages {
            let version = HeaderValue::from_str(&self.anthropic_version)
                .map_err(|_| request_error("anthropic-version is not a valid header value"))?;
            headers.insert("anthropic-version", version);
        }
        Ok(headers)
    }
}

pub(crate) fn provider_name() -> Arc<str> {
    Arc::from(PROVIDER_NAME)
}

pub(crate) fn profile_error(message: &'static str) -> ModelError {
    ModelError::try_new(
        MODEL_PROFILE_INVALID,
        ErrorCategory::Validation,
        false,
        message,
        Metadata::empty(),
    )
    .expect("reserved profile error is valid")
}

pub(crate) fn request_error(message: &'static str) -> ModelError {
    ModelError::try_new(
        MODEL_REQUEST_INVALID,
        ErrorCategory::Validation,
        false,
        message,
        Metadata::empty(),
    )
    .expect("reserved request error is valid")
}

fn validate_endpoint(value: &str) -> Result<(), ModelError> {
    let url = Url::parse(value).map_err(|_| request_error("provider endpoint is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(request_error(
            "provider endpoint contains forbidden components",
        ));
    }
    if url.scheme() == "http" && !is_loopback(&url) {
        return Err(request_error(
            "plaintext HTTP is allowed only for loopback endpoints",
        ));
    }
    Ok(())
}

fn is_loopback(url: &Url) -> bool {
    match url.host_str() {
        Some(host) if host.eq_ignore_ascii_case("localhost") => true,
        Some(host) => host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_runtime::TokenEstimatorSource;

    const CANARY: &str = "sk-secret-canary-gateway";

    fn estimator() -> TokenEstimatorRef {
        TokenEstimatorRef {
            id: Arc::from("gateway.utf8-byte-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        }
    }

    fn flags() -> GatewayCapabilityFlags {
        GatewayCapabilityFlags {
            input: InputCapabilities {
                text: true,
                json: true,
                images: false,
                audio: false,
                files: false,
            },
            native_tool_calls: true,
            parallel_tool_calls: false,
            structured_output: StructuredOutputCapability::Unsupported,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
        }
    }

    fn complete_model_config() -> GatewayModelConfig {
        GatewayModelConfig {
            name: Some("fixture-model".to_owned()),
            hard_input_bytes: Some(1_000_000),
            context_window_tokens: Some(8_192),
            max_output_tokens: Some(1_024),
            reserved_output_tokens: Some(1_024),
            provider_overhead_tokens: Some(64),
            estimator: Some(estimator()),
            capabilities: Some(flags()),
        }
    }

    #[test]
    fn gateway_rejects_model_missing_context_window() {
        let mut config = complete_model_config();
        config.context_window_tokens = None;
        let error = GatewayModelSpec::try_from_config(config).expect_err("missing window");
        assert_eq!(error.code(), MODEL_PROFILE_INVALID);
    }

    #[test]
    fn gateway_rejects_plaintext_non_loopback_endpoint() {
        let error = GatewayRouteConfig::try_new(
            WireProtocol::OpenaiChat,
            "http://example.test/v1/chat/completions",
            CredentialReference::try_new("prod").expect("reference"),
        )
        .expect_err("plaintext non-loopback");
        assert_eq!(error.code(), MODEL_REQUEST_INVALID);
    }

    #[test]
    fn secret_values_are_redacted_from_all_debug_surfaces() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let authentication = Authentication::Bearer(secret.clone());
        let mut store = CredentialStore::empty();
        store.insert("prod", authentication.clone()).expect("store");
        let route = GatewayRouteConfig::try_new(
            WireProtocol::OpenaiChat,
            "https://api.example.test/v1/chat/completions",
            CredentialReference::try_new("prod").expect("reference"),
        )
        .expect("route");

        for rendered in [
            format!("{secret:?}"),
            format!("{authentication:?}"),
            format!("{store:?}"),
            format!("{route:?}"),
        ] {
            assert!(!rendered.contains(CANARY), "secret leaked in {rendered}");
        }
        assert!(format!("{route:?}").contains("prod"));
        assert!(!format!("{route:?}").contains("Bearer"));
    }

    #[test]
    fn loopback_http_is_allowed_and_userinfo_is_rejected() {
        GatewayRouteConfig::try_new(
            WireProtocol::OllamaChat,
            "http://127.0.0.1:11434/api/chat",
            CredentialReference::try_new("local").expect("reference"),
        )
        .expect("loopback");
        assert_eq!(
            GatewayRouteConfig::try_new(
                WireProtocol::OpenaiChat,
                "https://user:pass@api.example.test/v1/chat/completions",
                CredentialReference::try_new("prod").expect("reference"),
            )
            .expect_err("userinfo")
            .code(),
            MODEL_REQUEST_INVALID
        );
    }
}
