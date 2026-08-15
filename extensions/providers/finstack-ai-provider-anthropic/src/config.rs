//! Secret-safe Anthropic Messages provider and model configuration.

use core::fmt;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::{
    InputCapabilities, ModelCapabilities, ModelContextProfile, ModelError, ModelName,
    StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource,
};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::ANTHROPIC_MESSAGES_VERSION;
use crate::error::config_error;

const DEFAULT_MESSAGES_PATH: &str = "/v1/messages";
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);
const DEFAULT_MAX_EVENT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_STREAM_BYTES: usize = 16 * 1_048_576;
const SECRET_MAX_BYTES: usize = 16 * 1_024;

/// Opaque configured secret whose formatting is always redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Arc<str>);

impl SecretString {
    /// Construct a non-empty bounded secret.
    ///
    /// # Errors
    ///
    /// Returns `anthropic_config_invalid` for an empty, oversized, or NUL-bearing value.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, ModelError> {
        let value = value.as_ref();
        if value.is_empty() || value.len() > SECRET_MAX_BYTES || value.as_bytes().contains(&0) {
            return Err(config_error("provider secret is invalid"));
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
    /// Anthropic `x-api-key` credential.
    ApiKey(SecretString),
}

impl fmt::Debug for Authentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::ApiKey(_) => formatter.write_str("ApiKey([REDACTED])"),
        }
    }
}

/// One configured header whose value is always treated as secret.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretHeader {
    name: Arc<str>,
    value: SecretString,
}

impl SecretHeader {
    /// Construct a secret custom header.
    ///
    /// # Errors
    ///
    /// Rejects invalid or provider-owned header names and invalid values.
    pub fn try_new(name: impl AsRef<str>, value: SecretString) -> Result<Self, ModelError> {
        let name = name.as_ref();
        let parsed = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| config_error("custom header name is invalid"))?;
        if matches!(
            parsed.as_str(),
            "authorization" | "x-api-key" | "anthropic-version" | "content-type"
        ) {
            return Err(config_error("custom header name is provider-owned"));
        }
        Ok(Self {
            name: Arc::from(parsed.as_str()),
            value,
        })
    }
}

impl fmt::Debug for SecretHeader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretHeader")
            .field("name", &self.name)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// Strict Anthropic Messages transport configuration.
#[derive(Clone)]
pub struct AnthropicConfig {
    base_url: Arc<str>,
    messages_path: Arc<str>,
    anthropic_version: Arc<str>,
    authentication: Authentication,
    headers: Arc<[SecretHeader]>,
    request_timeout: Duration,
    max_event_bytes: usize,
    max_stream_bytes: usize,
}

impl fmt::Debug for AnthropicConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicConfig")
            .field("base_url", &self.base_url)
            .field("messages_path", &self.messages_path)
            .field("anthropic_version", &self.anthropic_version)
            .field("authentication", &self.authentication)
            .field("headers", &self.headers)
            .field("request_timeout", &self.request_timeout)
            .field("max_event_bytes", &self.max_event_bytes)
            .field("max_stream_bytes", &self.max_stream_bytes)
            .finish()
    }
}

impl AnthropicConfig {
    /// Construct keyless configuration for one Messages endpoint.
    ///
    /// # Errors
    ///
    /// Rejects a URL with credentials, query, fragment, or a non-HTTP scheme.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_anthropic::AnthropicConfig;
    ///
    /// let config = AnthropicConfig::try_new("http://127.0.0.1:9").expect("config");
    /// assert!(format!("{config:?}").contains("127.0.0.1"));
    /// ```
    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self, ModelError> {
        let base_url = base_url.as_ref();
        validate_base_url(base_url)?;
        Ok(Self {
            base_url: Arc::from(base_url),
            messages_path: Arc::from(DEFAULT_MESSAGES_PATH),
            anthropic_version: Arc::from(ANTHROPIC_MESSAGES_VERSION),
            authentication: Authentication::None,
            headers: Arc::from([]),
            request_timeout: DEFAULT_TIMEOUT,
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
        })
    }

    /// Set the Messages path.
    ///
    /// # Errors
    ///
    /// Rejects non-absolute paths, query/fragment text, NUL, and oversized values.
    pub fn with_messages_path(mut self, path: impl AsRef<str>) -> Result<Self, ModelError> {
        let path = path.as_ref();
        if !path.starts_with('/') || path.len() > 2_048 || path.contains(['?', '#', '\0']) {
            return Err(config_error("messages path is invalid"));
        }
        self.messages_path = Arc::from(path);
        Ok(self)
    }

    /// Override the `anthropic-version` header.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, or NUL-bearing values.
    pub fn with_anthropic_version(mut self, version: impl AsRef<str>) -> Result<Self, ModelError> {
        let version = version.as_ref();
        if version.is_empty() || version.len() > 64 || version.as_bytes().contains(&0) {
            return Err(config_error("anthropic-version is invalid"));
        }
        self.anthropic_version = Arc::from(version);
        Ok(self)
    }

    /// Set explicit authentication.
    #[must_use]
    pub fn with_authentication(mut self, authentication: Authentication) -> Self {
        self.authentication = authentication;
        self
    }

    /// Set secret custom headers.
    #[must_use]
    pub fn with_headers(mut self, headers: Vec<SecretHeader>) -> Self {
        self.headers = headers.into();
        self
    }

    /// Set the whole-request timeout.
    ///
    /// # Errors
    ///
    /// Rejects zero and durations above one hour.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Result<Self, ModelError> {
        if timeout.is_zero() || timeout > Duration::from_hours(1) {
            return Err(config_error("provider request timeout is invalid"));
        }
        self.request_timeout = timeout;
        Ok(self)
    }

    /// Set raw SSE event and total response byte limits.
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
            return Err(config_error("provider stream limits are invalid"));
        }
        self.max_event_bytes = max_event_bytes;
        self.max_stream_bytes = max_stream_bytes;
        Ok(self)
    }

    pub(crate) fn endpoint_url(&self) -> Result<Url, ModelError> {
        let mut base =
            Url::parse(&self.base_url).map_err(|_| config_error("provider base URL is invalid"))?;
        base.set_path(&self.messages_path);
        Ok(base)
    }

    pub(crate) fn header_map(&self) -> Result<HeaderMap, ModelError> {
        let url =
            Url::parse(&self.base_url).map_err(|_| config_error("provider base URL is invalid"))?;
        if url.scheme() != "https"
            && (!matches!(self.authentication, Authentication::None) || !self.headers.is_empty())
        {
            return Err(config_error(
                "provider credentials and secret headers require HTTPS",
            ));
        }
        let mut headers = HeaderMap::new();
        let version = HeaderValue::from_str(&self.anthropic_version)
            .map_err(|_| config_error("anthropic-version is not a valid header value"))?;
        headers.insert(HeaderName::from_static("anthropic-version"), version);
        match &self.authentication {
            Authentication::None => {}
            Authentication::ApiKey(value) => {
                let mut header = HeaderValue::from_str(value.expose())
                    .map_err(|_| config_error("API key is not a valid header value"))?;
                header.set_sensitive(true);
                headers.insert(HeaderName::from_static("x-api-key"), header);
            }
        }
        for custom in self.headers.iter() {
            let name = HeaderName::from_bytes(custom.name.as_bytes())
                .map_err(|_| config_error("custom header name is invalid"))?;
            let mut value = HeaderValue::from_str(custom.value.expose())
                .map_err(|_| config_error("custom header value is invalid"))?;
            value.set_sensitive(true);
            if headers.insert(name, value).is_some() {
                return Err(config_error("provider header is duplicated"));
            }
        }
        Ok(headers)
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
}

fn validate_base_url(value: &str) -> Result<(), ModelError> {
    let url = Url::parse(value).map_err(|_| config_error("provider base URL is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(config_error(
            "provider base URL contains forbidden components",
        ));
    }
    Ok(())
}

/// Provider facts for one configured Anthropic model name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicModelConfig {
    /// Provider model name.
    pub name: ModelName,
    /// Maximum canonical request bytes.
    pub hard_input_bytes: u64,
    /// Total provider context window.
    pub context_window_tokens: u64,
    /// Maximum generated tokens.
    pub max_output_tokens: u64,
    /// Reserved output margin.
    pub reserved_output_tokens: u64,
    /// Conservative framing overhead.
    pub provider_overhead_tokens: u64,
    /// Native parallel tool-call support.
    pub parallel_tool_calls: bool,
    /// Whether Anthropic thinking is configured for this model.
    pub thinking: bool,
    /// Thinking token budget when thinking is enabled.
    pub thinking_budget_tokens: u64,
    /// Whether cache breakpoints may attach to the last stable system block.
    pub cache_breakpoints: bool,
}

impl AnthropicModelConfig {
    /// Construct conservative model metadata.
    ///
    /// # Errors
    ///
    /// Rejects zero/overflowing ceilings or safety margins outside the window.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_anthropic::AnthropicModelConfig;
    ///
    /// let model = AnthropicModelConfig::try_new(
    ///     "claude-test",
    ///     1_000_000,
    ///     128_000,
    ///     4_096,
    ///     4_096,
    ///     256,
    /// )
    /// .expect("model");
    /// assert!(!model.thinking);
    /// assert!(!model.cache_breakpoints);
    /// ```
    pub fn try_new(
        name: impl AsRef<str>,
        hard_input_bytes: u64,
        context_window_tokens: u64,
        max_output_tokens: u64,
        reserved_output_tokens: u64,
        provider_overhead_tokens: u64,
    ) -> Result<Self, ModelError> {
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
            return Err(config_error("provider model context profile is invalid"));
        }
        Ok(Self {
            name,
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            reserved_output_tokens,
            provider_overhead_tokens,
            parallel_tool_calls: true,
            thinking: false,
            thinking_budget_tokens: 1_024,
            cache_breakpoints: false,
        })
    }

    /// Set parallel tool-call capability.
    #[must_use]
    pub const fn with_parallel_tool_calls(mut self, enabled: bool) -> Self {
        self.parallel_tool_calls = enabled;
        self
    }

    /// Enable Anthropic thinking with an explicit token budget.
    ///
    /// # Errors
    ///
    /// Rejects a zero budget or a budget that is not strictly below `max_output_tokens`.
    pub fn with_thinking(mut self, enabled: bool, budget_tokens: u64) -> Result<Self, ModelError> {
        if enabled && (budget_tokens == 0 || budget_tokens >= self.max_output_tokens) {
            return Err(config_error("provider thinking budget is invalid"));
        }
        self.thinking = enabled;
        if enabled {
            self.thinking_budget_tokens = budget_tokens;
        }
        Ok(self)
    }

    /// Enable cache breakpoints on the last stable system prefix block.
    #[must_use]
    pub const fn with_cache_breakpoints(mut self, enabled: bool) -> Self {
        self.cache_breakpoints = enabled;
        self
    }

    pub(crate) fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: ModelContextProfile {
                provider: Arc::from("anthropic"),
                model: self.name.clone(),
                hard_input_bytes: self.hard_input_bytes,
                context_window_tokens: self.context_window_tokens,
                max_output_tokens: self.max_output_tokens,
                reserved_output_tokens: self.reserved_output_tokens,
                provider_overhead_tokens: self.provider_overhead_tokens,
                estimator: estimator_ref(),
            },
            native_tool_calls: true,
            parallel_tool_calls: self.parallel_tool_calls,
            structured_output: StructuredOutputCapability::Prompted,
            reasoning: self.thinking,
            prompt_cache: self.cache_breakpoints,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities: BTreeSet::from([
                Arc::from("anthropic.messages"),
                Arc::from("anthropic.sse"),
            ]),
        }
    }

    pub(crate) fn apply_capabilities(&mut self, update: &ModelCapabilities) {
        self.thinking = update.reasoning;
        self.cache_breakpoints = update.prompt_cache;
        self.hard_input_bytes = update.context_profile.hard_input_bytes;
        self.context_window_tokens = update.context_profile.context_window_tokens;
        self.max_output_tokens = update.context_profile.max_output_tokens;
        self.reserved_output_tokens = update.context_profile.reserved_output_tokens;
        self.provider_overhead_tokens = update.context_profile.provider_overhead_tokens;
        self.parallel_tool_calls = update.parallel_tool_calls;
    }
}

pub(crate) fn estimator_ref() -> TokenEstimatorRef {
    TokenEstimatorRef {
        id: Arc::from("anthropic.utf8-byte-upper-bound"),
        version: Arc::from("1"),
        source: TokenEstimatorSource::ConservativeUpperBound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANARY: &str = "sk-ant-secret-canary-055";

    #[test]
    fn secret_values_are_redacted_from_all_debug_surfaces() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let header = SecretHeader::try_new("x-private-token", secret.clone()).expect("header");
        let config = AnthropicConfig::try_new("https://api.anthropic.test")
            .expect("config")
            .with_authentication(Authentication::ApiKey(secret.clone()))
            .with_headers(vec![header.clone()]);

        for rendered in [
            format!("{secret:?}"),
            format!("{:?}", Authentication::ApiKey(secret)),
            format!("{header:?}"),
            format!("{config:?}"),
        ] {
            assert!(!rendered.contains(CANARY));
            assert!(rendered.contains("REDACTED"));
        }
    }

    #[test]
    fn base_url_rejects_credential_and_redirect_shaping_components() {
        for url in [
            "ftp://example.test",
            "https://user:pass@example.test",
            "https://example.test?token=nope",
            "https://example.test/#fragment",
        ] {
            assert_eq!(
                AnthropicConfig::try_new(url)
                    .expect_err("unsafe URL")
                    .code(),
                crate::error::CONFIG_INVALID
            );
        }
    }

    #[test]
    fn plaintext_http_is_keyless_only_and_owned_headers_are_rejected() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let config = AnthropicConfig::try_new("http://127.0.0.1:8080")
            .expect("local endpoint")
            .with_authentication(Authentication::ApiKey(secret.clone()));
        assert_eq!(
            config.header_map().expect_err("HTTP credential").code(),
            crate::error::CONFIG_INVALID
        );
        assert!(SecretHeader::try_new("x-api-key", secret).is_err());
    }
}
