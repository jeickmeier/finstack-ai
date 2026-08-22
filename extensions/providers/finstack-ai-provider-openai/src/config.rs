//! Secret-safe official `OpenAI` Responses provider and model configuration.

use core::fmt;
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::ports::model::{
    Authentication, CredentialReference, CredentialStore, InputCapabilities, MediaResolver,
    ModelCapabilities, ModelContextProfile, ModelError, ModelName, SecretString,
    StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource,
};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::error::config_error;

const DEFAULT_RESPONSES_PATH: &str = "/v1/responses";
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);
const DEFAULT_MAX_EVENT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_STREAM_BYTES: usize = 16 * 1_048_576;

const DEFAULT_CREDENTIAL_NAME: &str = "default";

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
            "authorization" | "content-type" | "x-client-request-id"
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

/// Strict `OpenAI` Responses (`/v1/responses`) transport configuration.
#[derive(Clone)]
pub struct OpenAiConfig {
    base_url: Arc<str>,
    responses_path: Arc<str>,
    credentials: CredentialStore,
    credential: Option<CredentialReference>,
    headers: Arc<[SecretHeader]>,
    request_timeout: Duration,
    max_event_bytes: usize,
    max_stream_bytes: usize,
    media_resolver: Option<Arc<dyn MediaResolver>>,
}

impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiConfig")
            .field("base_url", &self.base_url)
            .field("responses_path", &self.responses_path)
            .field("credentials", &self.credentials)
            .field("credential", &self.credential)
            .field("headers", &self.headers)
            .field("request_timeout", &self.request_timeout)
            .field("max_event_bytes", &self.max_event_bytes)
            .field("max_stream_bytes", &self.max_stream_bytes)
            .field(
                "media_resolver",
                &self.media_resolver.as_ref().map(|_| "[resolver]"),
            )
            .finish()
    }
}

impl OpenAiConfig {
    /// Construct keyless configuration for one Responses endpoint.
    ///
    /// The wire path is fixed at `/v1/responses`.
    ///
    /// # Errors
    ///
    /// Rejects a URL with credentials, query, fragment, or a non-HTTP scheme.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_openai::OpenAiConfig;
    ///
    /// let config = OpenAiConfig::try_new("http://127.0.0.1:9").expect("config");
    /// assert!(format!("{config:?}").contains("127.0.0.1"));
    /// ```
    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self, ModelError> {
        let base_url = base_url.as_ref();
        validate_base_url(base_url)?;
        Ok(Self {
            base_url: Arc::from(base_url),
            responses_path: Arc::from(DEFAULT_RESPONSES_PATH),
            credentials: CredentialStore::empty(),
            credential: None,
            headers: Arc::from([]),
            request_timeout: DEFAULT_TIMEOUT,
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
            media_resolver: None,
        })
    }

    /// Attach a host-supplied media resolver enabling image/audio/file input.
    #[must_use]
    pub fn with_media_resolver(mut self, resolver: Arc<dyn MediaResolver>) -> Self {
        self.media_resolver = Some(resolver);
        self
    }

    pub(crate) fn media_resolver(&self) -> Option<Arc<dyn MediaResolver>> {
        self.media_resolver.clone()
    }

    /// Set the default credential from an [`Authentication`] value.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the credential cannot be stored.
    /// It is reported rather than swallowed: silently returning an
    /// unauthenticated config surfaces later as a confusing request failure.
    pub fn with_authentication(self, authentication: Authentication) -> Result<Self, ModelError> {
        let mut store = CredentialStore::empty();
        store
            .insert(DEFAULT_CREDENTIAL_NAME, authentication)
            .map_err(|_| config_error("default credential name is invalid"))?;
        let reference = CredentialReference::try_new(DEFAULT_CREDENTIAL_NAME)
            .map_err(|_| config_error("default credential name is invalid"))?;
        Ok(self.with_credential_store(store, reference))
    }

    /// Bind an explicit host-supplied credential store and reference.
    #[must_use]
    pub fn with_credential_store(
        mut self,
        store: CredentialStore,
        reference: CredentialReference,
    ) -> Self {
        self.credentials = store;
        self.credential = Some(reference);
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

    fn resolved_authentication(&self) -> Result<Authentication, ModelError> {
        let Some(reference) = &self.credential else {
            return Ok(Authentication::None);
        };
        self.credentials
            .resolve(reference)
            .cloned()
            .ok_or_else(|| config_error("named credential is missing"))
    }

    pub(crate) fn endpoint_url(&self) -> Result<Url, ModelError> {
        let mut base =
            Url::parse(&self.base_url).map_err(|_| config_error("provider base URL is invalid"))?;
        base.set_path(&self.responses_path);
        Ok(base)
    }

    pub(crate) fn header_map(&self) -> Result<HeaderMap, ModelError> {
        let url =
            Url::parse(&self.base_url).map_err(|_| config_error("provider base URL is invalid"))?;
        let authentication = self.resolved_authentication()?;
        if url.scheme() != "https"
            && (!matches!(authentication, Authentication::None) || !self.headers.is_empty())
        {
            return Err(config_error(
                "provider credentials and secret headers require HTTPS",
            ));
        }
        let mut headers = HeaderMap::new();
        match authentication {
            Authentication::None => {}
            Authentication::Bearer(value) => {
                let mut header = HeaderValue::from_str(&format!("Bearer {}", value.expose()))
                    .map_err(|_| config_error("bearer credential is not a valid header value"))?;
                header.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, header);
            }
            Authentication::ApiKey(_) => {
                return Err(config_error("openai authentication must be bearer"));
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
    if url.scheme() == "http"
        && !url
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|address| address.is_loopback())
    {
        return Err(config_error(
            "plaintext provider base URL must use a loopback IP",
        ));
    }
    Ok(())
}

fn validate_context_profile(
    hard_input_bytes: u64,
    context_window_tokens: u64,
    max_output_tokens: u64,
    reserved_output_tokens: u64,
    provider_overhead_tokens: u64,
) -> Result<(), ModelError> {
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
    Ok(())
}

/// Provider facts for one configured official `OpenAI` model name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent, host-toggled capability advertisement"
)]
pub struct OpenAiModelConfig {
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
    /// Whether the configured model advertises reasoning content.
    pub reasoning: bool,
    /// Whether the configured model accepts image input.
    pub input_images: bool,
    /// Whether the configured model accepts audio input.
    pub input_audio: bool,
    /// Whether the configured model accepts file input.
    pub input_files: bool,
}

impl OpenAiModelConfig {
    /// Construct conservative model metadata.
    ///
    /// # Errors
    ///
    /// Rejects zero/overflowing ceilings or safety margins outside the window.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_openai::OpenAiModelConfig;
    ///
    /// let model = OpenAiModelConfig::try_new(
    ///     "gpt-test",
    ///     1_000_000,
    ///     128_000,
    ///     4_096,
    ///     4_096,
    ///     256,
    /// )
    /// .expect("model");
    /// assert!(!model.reasoning);
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
        validate_context_profile(
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            reserved_output_tokens,
            provider_overhead_tokens,
        )?;
        Ok(Self {
            name,
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            reserved_output_tokens,
            provider_overhead_tokens,
            parallel_tool_calls: true,
            reasoning: false,
            input_images: false,
            input_audio: false,
            input_files: false,
        })
    }

    /// Set parallel tool-call capability.
    #[must_use]
    pub const fn with_parallel_tool_calls(mut self, enabled: bool) -> Self {
        self.parallel_tool_calls = enabled;
        self
    }

    /// Advertise reasoning content for this model only.
    #[must_use]
    pub const fn with_reasoning(mut self, enabled: bool) -> Self {
        self.reasoning = enabled;
        self
    }

    /// Advertise image input support for this model only.
    #[must_use]
    pub const fn with_input_images(mut self, enabled: bool) -> Self {
        self.input_images = enabled;
        self
    }

    /// Advertise audio input support for this model only.
    #[must_use]
    pub const fn with_input_audio(mut self, enabled: bool) -> Self {
        self.input_audio = enabled;
        self
    }

    /// Advertise file input support for this model only.
    #[must_use]
    pub const fn with_input_files(mut self, enabled: bool) -> Self {
        self.input_files = enabled;
        self
    }

    pub(crate) fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: self.input_images,
                audio: self.input_audio,
                files: self.input_files,
            },
            context_profile: ModelContextProfile {
                provider: Arc::from("openai"),
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
            structured_output: StructuredOutputCapability::Native,
            reasoning: self.reasoning,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities: BTreeSet::from([Arc::from("openai.responses")]),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), ModelError> {
        validate_context_profile(
            self.hard_input_bytes,
            self.context_window_tokens,
            self.max_output_tokens,
            self.reserved_output_tokens,
            self.provider_overhead_tokens,
        )
    }

    pub(crate) fn apply_capabilities(
        &mut self,
        update: &ModelCapabilities,
    ) -> Result<(), ModelError> {
        validate_context_profile(
            update.context_profile.hard_input_bytes,
            update.context_profile.context_window_tokens,
            update.context_profile.max_output_tokens,
            update.context_profile.reserved_output_tokens,
            update.context_profile.provider_overhead_tokens,
        )?;
        self.reasoning = update.reasoning;
        self.hard_input_bytes = update.context_profile.hard_input_bytes;
        self.context_window_tokens = update.context_profile.context_window_tokens;
        self.max_output_tokens = update.context_profile.max_output_tokens;
        self.reserved_output_tokens = update.context_profile.reserved_output_tokens;
        self.provider_overhead_tokens = update.context_profile.provider_overhead_tokens;
        self.parallel_tool_calls = update.parallel_tool_calls;
        self.input_images = update.input.images;
        self.input_audio = update.input.audio;
        self.input_files = update.input.files;
        Ok(())
    }
}

pub(crate) fn estimator_ref() -> TokenEstimatorRef {
    TokenEstimatorRef {
        id: Arc::from("openai.utf8-byte-upper-bound"),
        version: Arc::from("1"),
        source: TokenEstimatorSource::ConservativeUpperBound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANARY: &str = "sk-secret-canary-040";

    #[test]
    fn secret_values_are_redacted_from_all_debug_surfaces() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let header = SecretHeader::try_new("x-private-token", secret.clone()).expect("header");
        let config = OpenAiConfig::try_new("https://api.openai.test")
            .expect("config")
            .with_authentication(Authentication::Bearer(secret.clone()))
            .expect("authentication")
            .with_headers(vec![header.clone()])
            .with_media_resolver(std::sync::Arc::new(CanaryResolver));

        for rendered in [
            format!("{secret:?}"),
            format!("{:?}", Authentication::Bearer(secret)),
            format!("{header:?}"),
            format!("{config:?}"),
        ] {
            assert!(!rendered.contains(CANARY));
        }
        assert!(format!("{header:?}").contains("REDACTED"));
        assert!(format!("{config:?}").contains("media_resolver"));
        assert!(!format!("{config:?}").contains("CanaryResolver"));
    }

    #[derive(Debug)]
    struct CanaryResolver;

    impl MediaResolver for CanaryResolver {
        fn resolve(
            &self,
            _blob: &finstack_ai_kernel::BlobRef,
        ) -> finstack_ai_runtime::ports::PortFuture<
            Result<
                finstack_ai_runtime::ports::model::ResolvedMedia,
                finstack_ai_runtime::ports::model::MediaResolveError,
            >,
        > {
            Box::pin(async {
                Err(finstack_ai_runtime::ports::model::MediaResolveError {
                    kind: finstack_ai_runtime::ports::model::MediaResolveKind::NotFound,
                    message: "canary resolver never resolves",
                })
            })
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
                OpenAiConfig::try_new(url).expect_err("unsafe URL").code(),
                crate::error::CONFIG_INVALID
            );
        }
    }

    #[test]
    fn plaintext_http_requires_a_literal_loopback_ip() {
        for url in [
            "http://example.test",
            "http://192.0.2.1",
            "http://localhost",
        ] {
            assert_eq!(
                OpenAiConfig::try_new(url)
                    .expect_err("remote plaintext")
                    .code(),
                crate::error::CONFIG_INVALID
            );
        }
        for url in ["http://127.0.0.1:8080", "http://[::1]:8080"] {
            assert!(OpenAiConfig::try_new(url).is_ok());
        }
        assert!(OpenAiConfig::try_new("https://example.test").is_ok());
    }

    #[test]
    fn plaintext_http_is_keyless_only_and_owned_headers_are_rejected() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let config = OpenAiConfig::try_new("http://127.0.0.1:8080")
            .expect("local endpoint")
            .with_authentication(Authentication::Bearer(secret.clone()))
            .expect("authentication");
        assert_eq!(
            config.header_map().expect_err("HTTP credential").code(),
            crate::error::CONFIG_INVALID
        );
        assert!(SecretHeader::try_new("authorization", secret).is_err());
    }

    #[test]
    fn unresolved_named_credential_fails_closed() {
        let store = CredentialStore::empty();
        let reference = CredentialReference::try_new("missing").expect("reference");
        let config = OpenAiConfig::try_new("https://api.openai.test")
            .expect("config")
            .with_credential_store(store, reference);
        assert_eq!(
            config.header_map().expect_err("missing").code(),
            crate::error::CONFIG_INVALID
        );
    }

    #[test]
    fn try_new_keeps_the_stable_openai_config_code() {
        assert_eq!(
            OpenAiConfig::try_new("ftp://example.test")
                .expect_err("scheme")
                .code(),
            crate::error::CONFIG_INVALID
        );
    }
}
