//! Secret-safe Ollama provider and model configuration.

use core::fmt;
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::ports::model::{
    Authentication, CredentialReference, CredentialStore, InputCapabilities, MediaResolver,
    ModelCapabilities, ModelContextProfile, ModelError, ModelName, StructuredOutputCapability,
    TokenEstimatorRef, TokenEstimatorSource,
};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderValue};

use crate::error::config_error;

const DEFAULT_CHAT_PATH: &str = "/api/chat";
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);
const DEFAULT_MAX_EVENT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_STREAM_BYTES: usize = 16 * 1_048_576;
const DEFAULT_CREDENTIAL_NAME: &str = "default";

/// Strict Ollama `/api/chat` transport configuration.
#[derive(Clone)]
pub struct OllamaConfig {
    base_url: Arc<str>,
    chat_path: Arc<str>,
    credentials: CredentialStore,
    credential: Option<CredentialReference>,
    request_timeout: Duration,
    max_event_bytes: usize,
    max_stream_bytes: usize,
    media_resolver: Option<Arc<dyn MediaResolver>>,
}

impl fmt::Debug for OllamaConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OllamaConfig")
            .field("base_url", &self.base_url)
            .field("chat_path", &self.chat_path)
            .field("credentials", &self.credentials)
            .field("credential", &self.credential)
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

impl OllamaConfig {
    /// Construct keyless configuration for one `/api/chat` endpoint.
    ///
    /// # Errors
    ///
    /// Rejects a URL with credentials, query, fragment, or a non-HTTP scheme.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_ollama::OllamaConfig;
    ///
    /// let config = OllamaConfig::try_new("http://127.0.0.1:11434").expect("config");
    /// assert!(format!("{config:?}").contains("127.0.0.1"));
    /// ```
    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self, ModelError> {
        let base_url = base_url.as_ref();
        validate_base_url(base_url)?;
        Ok(Self {
            base_url: Arc::from(base_url),
            chat_path: Arc::from(DEFAULT_CHAT_PATH),
            credentials: CredentialStore::empty(),
            credential: None,
            request_timeout: DEFAULT_TIMEOUT,
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
            media_resolver: None,
        })
    }

    /// Attach a host-supplied media resolver enabling image input.
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

    /// Set raw NDJSON line and total response byte limits.
    ///
    /// # Errors
    ///
    /// Rejects zero limits or a total below one line.
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
        base.set_path(&self.chat_path);
        Ok(base)
    }

    pub(crate) fn header_map(&self) -> Result<HeaderMap, ModelError> {
        let url =
            Url::parse(&self.base_url).map_err(|_| config_error("provider base URL is invalid"))?;
        let authentication = self.resolved_authentication()?;
        if url.scheme() != "https" && !matches!(authentication, Authentication::None) {
            return Err(config_error("provider credentials require HTTPS"));
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
                return Err(config_error("ollama authentication must be bearer"));
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

/// Provider facts for one configured Ollama model name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaModelConfig {
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
    /// Whether Ollama thinking is configured for this model.
    pub reasoning: bool,
    /// Whether this model accepts base64 image input.
    pub input_images: bool,
}

impl OllamaModelConfig {
    /// Construct conservative model metadata.
    ///
    /// # Errors
    ///
    /// Rejects zero/overflowing ceilings or safety margins outside the window.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_ollama::OllamaModelConfig;
    ///
    /// let model = OllamaModelConfig::try_new(
    ///     "gemma3",
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
            reasoning: false,
            input_images: false,
        })
    }

    /// Advertise reasoning content for this model only.
    #[must_use]
    pub const fn with_reasoning(mut self, enabled: bool) -> Self {
        self.reasoning = enabled;
        self
    }

    /// Advertise base64 image input for this model only.
    #[must_use]
    pub const fn with_input_images(mut self, enabled: bool) -> Self {
        self.input_images = enabled;
        self
    }

    pub(crate) fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: self.input_images,
                audio: false,
                files: false,
            },
            context_profile: ModelContextProfile {
                provider: Arc::from("ollama"),
                model: self.name.clone(),
                hard_input_bytes: self.hard_input_bytes,
                context_window_tokens: self.context_window_tokens,
                max_output_tokens: self.max_output_tokens,
                reserved_output_tokens: self.reserved_output_tokens,
                provider_overhead_tokens: self.provider_overhead_tokens,
                estimator: estimator_ref(),
            },
            native_tool_calls: true,
            parallel_tool_calls: true,
            structured_output: StructuredOutputCapability::Prompted,
            reasoning: self.reasoning,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities: BTreeSet::from([Arc::from("ollama.api_chat")]),
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
        self.input_images = update.input.images;
        self.hard_input_bytes = update.context_profile.hard_input_bytes;
        self.context_window_tokens = update.context_profile.context_window_tokens;
        self.max_output_tokens = update.context_profile.max_output_tokens;
        self.reserved_output_tokens = update.context_profile.reserved_output_tokens;
        self.provider_overhead_tokens = update.context_profile.provider_overhead_tokens;
        Ok(())
    }
}

pub(crate) fn estimator_ref() -> TokenEstimatorRef {
    TokenEstimatorRef {
        id: Arc::from("ollama.utf8-byte-upper-bound"),
        version: Arc::from("1"),
        source: TokenEstimatorSource::ConservativeUpperBound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_runtime::ports::model::SecretString;

    const CANARY: &str = "ollama-secret-canary-070";

    #[test]
    fn secret_values_are_redacted_from_all_debug_surfaces() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let config = OllamaConfig::try_new("https://ollama.example.test")
            .expect("config")
            .with_authentication(Authentication::Bearer(secret.clone()));

        for rendered in [
            format!("{secret:?}"),
            format!("{:?}", Authentication::Bearer(secret.clone())),
            format!("{config:?}"),
        ] {
            assert!(!rendered.contains(CANARY));
        }
        assert!(format!("{secret:?}").contains("REDACTED"));
    }

    #[derive(Debug)]
    struct FixtureResolver;

    impl MediaResolver for FixtureResolver {
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
                Ok(finstack_ai_runtime::ports::model::ResolvedMedia::Url(
                    Arc::from("https://example.test/a.png"),
                ))
            })
        }
    }

    #[test]
    fn debug_with_a_resolver_attached_still_redacts_and_hides_resolver_internals() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let config = OllamaConfig::try_new("https://ollama.example.test")
            .expect("config")
            .with_authentication(Authentication::Bearer(secret))
            .expect("authentication")
            .with_media_resolver(Arc::new(FixtureResolver));

        let rendered = format!("{config:?}");
        assert!(!rendered.contains(CANARY));
        assert!(rendered.contains("[resolver]"));
        assert!(config.media_resolver().is_some());
    }

    #[test]
    fn capability_round_trip_flips_input_images() {
        let model =
            OllamaModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model")
                .with_input_images(true);
        assert!(model.capabilities().input.images);

        let mut refreshed =
            OllamaModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model");
        assert!(!refreshed.capabilities().input.images);
        refreshed
            .apply_capabilities(&model.capabilities())
            .expect("apply");
        assert!(refreshed.capabilities().input.images);
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
                OllamaConfig::try_new(url).expect_err("unsafe URL").code(),
                crate::error::CONFIG_INVALID
            );
        }
    }

    #[test]
    fn keyless_http_loopback_is_allowed_and_credentials_require_https() {
        let keyless = OllamaConfig::try_new("http://127.0.0.1:11434").expect("local endpoint");
        assert!(keyless.header_map().is_ok());

        let secret = SecretString::try_new(CANARY).expect("secret");
        let with_key = keyless
            .with_authentication(Authentication::Bearer(secret))
            .expect("authentication");
        assert_eq!(
            with_key.header_map().expect_err("HTTP credential").code(),
            crate::error::CONFIG_INVALID
        );
    }

    #[test]
    fn plaintext_http_rejects_non_loopback_hosts() {
        for url in [
            "http://example.test",
            "http://192.0.2.1",
            "http://localhost",
        ] {
            assert_eq!(
                OllamaConfig::try_new(url)
                    .expect_err("remote plaintext")
                    .code(),
                crate::error::CONFIG_INVALID
            );
        }
        assert!(OllamaConfig::try_new("http://[::1]:11434").is_ok());
        assert!(OllamaConfig::try_new("https://example.test").is_ok());
    }
}
