//! Secret-safe Gemini `generateContent` provider and model configuration.

use core::fmt;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::{
    Authentication, CredentialReference, CredentialStore, InputCapabilities, MediaResolver,
    ModelCapabilities, ModelContextProfile, ModelError, ModelName, SecretString,
    StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource,
};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::error::{GEMINI_CONFIG_INVALID, error};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);
const DEFAULT_MAX_EVENT_BYTES: usize = 1_048_576;
const DEFAULT_MAX_STREAM_BYTES: usize = 16 * 1_048_576;
const DEFAULT_CREDENTIAL_NAME: &str = "default";
#[allow(dead_code, reason = "read once request-building support lands in a later task")]
const GENERATIVE_LANGUAGE_API_VERSION: &str = "v1beta";
#[allow(dead_code, reason = "read once request-building support lands in a later task")]
const VERTEX_API_VERSION: &str = "v1";
const MAX_LABEL_BYTES: usize = 255;

fn config_error(message: &'static str) -> ModelError {
    error(GEMINI_CONFIG_INVALID, ErrorCategory::Configuration, false, message)
}

/// Resolved Gemini transport target.
#[derive(Clone, PartialEq, Eq)]
pub enum GeminiEndpoint {
    /// Google AI Studio ("Generative Language") API, default base
    /// `https://generativelanguage.googleapis.com`.
    GenerativeLanguage,
    /// Vertex AI API, default base `https://{location}-aiplatform.googleapis.com`
    /// (`"global"` resolves to `aiplatform.googleapis.com`).
    Vertex {
        /// Google Cloud project id.
        project: Arc<str>,
        /// Vertex AI region, or `"global"`.
        location: Arc<str>,
    },
}

impl fmt::Debug for GeminiEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GenerativeLanguage => formatter.write_str("GenerativeLanguage"),
            Self::Vertex { project, location } => formatter
                .debug_struct("Vertex")
                .field("project", project)
                .field("location", location)
                .finish(),
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
            "authorization" | "x-goog-api-key" | "content-type"
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

/// Strict Gemini `generateContent` transport configuration.
#[derive(Clone)]
pub struct GeminiConfig {
    base_url: Arc<str>,
    endpoint: GeminiEndpoint,
    credentials: CredentialStore,
    credential: Option<CredentialReference>,
    headers: Arc<[SecretHeader]>,
    #[allow(dead_code, reason = "read once request-building support lands in a later task")]
    request_timeout: Duration,
    #[allow(dead_code, reason = "read once request-building support lands in a later task")]
    max_event_bytes: usize,
    #[allow(dead_code, reason = "read once request-building support lands in a later task")]
    max_stream_bytes: usize,
    #[allow(dead_code, reason = "read once request-building support lands in a later task")]
    media_resolver: Option<Arc<dyn MediaResolver>>,
}

impl fmt::Debug for GeminiConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeminiConfig")
            .field("base_url", &self.base_url)
            .field("endpoint", &self.endpoint)
            .field("credentials", &self.credentials)
            .field("credential", &self.credential)
            .field("headers", &self.headers)
            .field("request_timeout", &self.request_timeout)
            .field("max_event_bytes", &self.max_event_bytes)
            .field("max_stream_bytes", &self.max_stream_bytes)
            .field(
                "media_resolver",
                &self
                    .media_resolver
                    .as_ref()
                    .map_or("None", |_| "[resolver]"),
            )
            .finish()
    }
}

impl GeminiConfig {
    /// Construct keyless configuration for the Generative Language API.
    ///
    /// # Errors
    ///
    /// Rejects a URL with credentials, query, fragment, or a non-HTTP scheme.
    pub fn try_new(base_url: impl AsRef<str>) -> Result<Self, ModelError> {
        let base_url = base_url.as_ref();
        validate_base_url(base_url)?;
        Ok(Self {
            base_url: Arc::from(base_url),
            endpoint: GeminiEndpoint::GenerativeLanguage,
            credentials: CredentialStore::empty(),
            credential: None,
            headers: Arc::from([]),
            request_timeout: DEFAULT_TIMEOUT,
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
            media_resolver: None,
        })
    }

    /// Construct keyless configuration for the Vertex AI API.
    ///
    /// # Errors
    ///
    /// Rejects an invalid base URL, or an empty/oversized/NUL-bearing
    /// `project` or `location`.
    pub fn try_new_vertex(
        base_url: impl AsRef<str>,
        project: impl AsRef<str>,
        location: impl AsRef<str>,
    ) -> Result<Self, ModelError> {
        let base_url = base_url.as_ref();
        validate_base_url(base_url)?;
        let project = validate_label(project.as_ref())?;
        let location = validate_label(location.as_ref())?;
        Ok(Self {
            base_url: Arc::from(base_url),
            endpoint: GeminiEndpoint::Vertex {
                project: Arc::from(project),
                location: Arc::from(location),
            },
            credentials: CredentialStore::empty(),
            credential: None,
            headers: Arc::from([]),
            request_timeout: DEFAULT_TIMEOUT,
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
            media_resolver: None,
        })
    }

    /// Bind an explicit host-supplied credential store and reference.
    #[must_use]
    pub fn with_credentials(
        mut self,
        store: CredentialStore,
        reference: CredentialReference,
    ) -> Self {
        self.credentials = store;
        self.credential = Some(reference);
        self
    }

    /// Insert one named credential entry and select it.
    ///
    /// Returns `self` unchanged when the reserved `default` name is rejected.
    #[must_use]
    pub fn with_authentication(self, authentication: Authentication) -> Self {
        let mut store = CredentialStore::empty();
        if store
            .insert(DEFAULT_CREDENTIAL_NAME, authentication)
            .is_err()
        {
            return self;
        }
        let Ok(reference) = CredentialReference::try_new(DEFAULT_CREDENTIAL_NAME) else {
            return self;
        };
        self.with_credentials(store, reference)
    }

    /// Append one secret custom header.
    ///
    /// # Errors
    ///
    /// Rejects a header name that duplicates an already-configured header.
    pub fn with_secret_header(mut self, header: SecretHeader) -> Result<Self, ModelError> {
        if self.headers.iter().any(|existing| existing.name == header.name) {
            return Err(config_error("provider header is duplicated"));
        }
        let mut headers = self.headers.to_vec();
        headers.push(header);
        self.headers = headers.into();
        Ok(self)
    }

    /// Set the whole-request timeout.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set raw SSE event and total response byte limits.
    #[must_use]
    pub fn with_stream_limits(mut self, max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        self.max_event_bytes = max_event_bytes;
        self.max_stream_bytes = max_stream_bytes;
        self
    }

    /// Attach a host-supplied media resolver enabling image/audio/file input.
    #[must_use]
    pub fn with_media_resolver(mut self, resolver: Arc<dyn MediaResolver>) -> Self {
        self.media_resolver = Some(resolver);
        self
    }

    #[allow(dead_code, reason = "called once request-building support lands in a later task")]
    fn resolved_authentication(&self) -> Result<Authentication, ModelError> {
        let Some(reference) = &self.credential else {
            return Ok(Authentication::None);
        };
        self.credentials
            .resolve(reference)
            .cloned()
            .ok_or_else(|| config_error("named credential is missing"))
    }

    #[allow(dead_code, reason = "called once request-building support lands in a later task")]
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
            Authentication::ApiKey(value) => {
                let mut header = HeaderValue::from_str(value.expose())
                    .map_err(|_| config_error("API key is not a valid header value"))?;
                header.set_sensitive(true);
                headers.insert(HeaderName::from_static("x-goog-api-key"), header);
            }
            Authentication::Bearer(value) => {
                let mut header = HeaderValue::from_str(&format!("Bearer {}", value.expose()))
                    .map_err(|_| config_error("bearer credential is not a valid header value"))?;
                header.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, header);
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

    #[allow(dead_code, reason = "called once request-building support lands in a later task")]
    pub(crate) fn model_url(&self, model: &ModelName) -> Result<Url, ModelError> {
        let name = model.as_str();
        if name.contains('/') || name.contains(':') {
            return Err(config_error(
                "provider model name contains forbidden characters",
            ));
        }
        let mut url =
            Url::parse(&self.base_url).map_err(|_| config_error("provider base URL is invalid"))?;
        let path = match &self.endpoint {
            GeminiEndpoint::GenerativeLanguage => {
                format!("/{GENERATIVE_LANGUAGE_API_VERSION}/models/{name}:streamGenerateContent")
            }
            GeminiEndpoint::Vertex { project, location } => format!(
                "/{VERTEX_API_VERSION}/projects/{project}/locations/{location}/publishers/google/models/{name}:streamGenerateContent"
            ),
        };
        url.set_path(&path);
        url.set_query(Some("alt=sse"));
        Ok(url)
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

fn validate_label(value: &str) -> Result<&str, ModelError> {
    if value.is_empty() || value.len() > MAX_LABEL_BYTES || value.as_bytes().contains(&0) {
        return Err(config_error("vertex project/location is invalid"));
    }
    Ok(value)
}

/// Provider facts for one configured Gemini model name.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag toggles an independent, orthogonal capability advertised to the host"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiModelConfig {
    name: ModelName,
    hard_input_bytes: u64,
    context_window_tokens: u64,
    max_output_tokens: u64,
    reserved_output_tokens: u64,
    provider_overhead_tokens: u64,
    parallel_tool_calls: bool,
    thinking: bool,
    #[allow(dead_code, reason = "read once request-building support lands in a later task")]
    thinking_budget_tokens: u64,
    input_images: bool,
    input_audio: bool,
    input_files: bool,
    google_search: bool,
    code_execution: bool,
    cached_content: bool,
}

impl GeminiModelConfig {
    /// Construct conservative model metadata.
    ///
    /// # Errors
    ///
    /// Rejects zero/overflowing ceilings.
    pub fn try_new(
        name: impl AsRef<str>,
        hard_input_bytes: u64,
        context_window_tokens: u64,
        max_output_tokens: u64,
    ) -> Result<Self, ModelError> {
        let name = ModelName::try_new(name)?;
        validate_context_profile(
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            max_output_tokens,
            0,
        )?;
        Ok(Self {
            name,
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            reserved_output_tokens: max_output_tokens,
            provider_overhead_tokens: 0,
            parallel_tool_calls: true,
            thinking: false,
            thinking_budget_tokens: 0,
            input_images: false,
            input_audio: false,
            input_files: false,
            google_search: false,
            code_execution: false,
            cached_content: false,
        })
    }

    /// Set the reserved output safety margin.
    #[must_use]
    pub fn with_reserved_output_tokens(mut self, value: u64) -> Self {
        self.reserved_output_tokens = value;
        self
    }

    /// Set the conservative provider framing overhead.
    #[must_use]
    pub fn with_provider_overhead_tokens(mut self, value: u64) -> Self {
        self.provider_overhead_tokens = value;
        self
    }

    /// Set parallel tool-call capability.
    #[must_use]
    pub fn with_parallel_tool_calls(mut self, value: bool) -> Self {
        self.parallel_tool_calls = value;
        self
    }

    /// Enable Gemini thinking with a default token budget.
    #[must_use]
    pub fn with_thinking(mut self, enabled: bool, default_budget_tokens: u64) -> Self {
        self.thinking = enabled;
        self.thinking_budget_tokens = default_budget_tokens;
        self
    }

    /// Set whether image input is accepted for this model.
    #[must_use]
    pub fn with_input_images(mut self, value: bool) -> Self {
        self.input_images = value;
        self
    }

    /// Set whether audio input is accepted for this model.
    #[must_use]
    pub fn with_input_audio(mut self, value: bool) -> Self {
        self.input_audio = value;
        self
    }

    /// Set whether file input (including `video/*`) is accepted for this model.
    #[must_use]
    pub fn with_input_files(mut self, value: bool) -> Self {
        self.input_files = value;
        self
    }

    /// Set whether the Google Search grounding tool is available.
    #[must_use]
    pub fn with_google_search(mut self, value: bool) -> Self {
        self.google_search = value;
        self
    }

    /// Set whether the code execution tool is available.
    #[must_use]
    pub fn with_code_execution(mut self, value: bool) -> Self {
        self.code_execution = value;
        self
    }

    /// Set whether cached content (`prompt_cache`) is available.
    #[must_use]
    pub fn with_cached_content(mut self, value: bool) -> Self {
        self.cached_content = value;
        self
    }

    /// Derive host-facing provider capabilities for this model.
    #[must_use]
    pub fn capabilities(&self, provider: &str) -> ModelCapabilities {
        let mut native_capabilities = BTreeSet::from([
            Arc::from("gemini.generate-content"),
            Arc::from("gemini.sse"),
        ]);
        if self.google_search {
            native_capabilities.insert(Arc::from("gemini.google-search"));
        }
        if self.code_execution {
            native_capabilities.insert(Arc::from("gemini.code-execution"));
        }
        ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: self.input_images,
                audio: self.input_audio,
                files: self.input_files,
            },
            context_profile: ModelContextProfile {
                provider: Arc::from(provider),
                model: self.name.clone(),
                hard_input_bytes: self.hard_input_bytes,
                context_window_tokens: self.context_window_tokens,
                max_output_tokens: self.max_output_tokens,
                reserved_output_tokens: self.reserved_output_tokens,
                provider_overhead_tokens: self.provider_overhead_tokens,
                estimator: Self::estimator_ref(),
            },
            native_tool_calls: true,
            parallel_tool_calls: self.parallel_tool_calls,
            structured_output: StructuredOutputCapability::Native,
            reasoning: self.thinking,
            prompt_cache: self.cached_content,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities,
        }
    }

    /// Overlay advertised capabilities from a host-supplied snapshot.
    ///
    /// # Errors
    ///
    /// Rejects zero/overflowing ceilings.
    pub fn apply_capabilities(&mut self, caps: &ModelCapabilities) -> Result<(), ModelError> {
        validate_context_profile(
            caps.context_profile.hard_input_bytes,
            caps.context_profile.context_window_tokens,
            caps.context_profile.max_output_tokens,
            caps.context_profile.reserved_output_tokens,
            caps.context_profile.provider_overhead_tokens,
        )?;
        self.hard_input_bytes = caps.context_profile.hard_input_bytes;
        self.context_window_tokens = caps.context_profile.context_window_tokens;
        self.max_output_tokens = caps.context_profile.max_output_tokens;
        self.reserved_output_tokens = caps.context_profile.reserved_output_tokens;
        self.provider_overhead_tokens = caps.context_profile.provider_overhead_tokens;
        self.parallel_tool_calls = caps.parallel_tool_calls;
        self.thinking = caps.reasoning;
        self.cached_content = caps.prompt_cache;
        self.input_images = caps.input.images;
        self.input_audio = caps.input.audio;
        self.input_files = caps.input.files;
        self.google_search = caps
            .native_capabilities
            .iter()
            .any(|capability| capability.as_ref() == "gemini.google-search");
        self.code_execution = caps
            .native_capabilities
            .iter()
            .any(|capability| capability.as_ref() == "gemini.code-execution");
        Ok(())
    }

    pub(crate) fn estimator_ref() -> TokenEstimatorRef {
        TokenEstimatorRef {
            id: Arc::from("gemini.utf8-byte-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const CANARY: &str = "AIza-secret-canary-051";

    #[test]
    fn debug_never_leaks_credentials() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let header = SecretHeader::try_new("x-private-token", secret.clone()).expect("header");
        let config = GeminiConfig::try_new("https://generativelanguage.googleapis.com")
            .expect("config")
            .with_authentication(Authentication::ApiKey(secret.clone()))
            .with_secret_header(header.clone())
            .expect("header");

        for rendered in [
            format!("{secret:?}"),
            format!("{:?}", Authentication::ApiKey(secret)),
            format!("{header:?}"),
            format!("{config:?}"),
        ] {
            assert!(!rendered.contains(CANARY));
        }
        assert!(format!("{header:?}").contains("REDACTED"));
    }

    #[test]
    fn https_required_with_credentials() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        let config = GeminiConfig::try_new("http://127.0.0.1:8080")
            .expect("config")
            .with_authentication(Authentication::ApiKey(secret));
        assert_eq!(
            config.header_map().expect_err("http credential").code(),
            crate::error::GEMINI_CONFIG_INVALID
        );
    }

    #[test]
    fn generative_language_url_shape() {
        let config =
            GeminiConfig::try_new("https://generativelanguage.googleapis.com").expect("config");
        let model = ModelName::try_new("gemini-2.5-pro").expect("model");
        assert_eq!(
            config.model_url(&model).expect("url").as_str(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn vertex_url_shape() {
        let config = GeminiConfig::try_new_vertex(
            "https://us-central1-aiplatform.googleapis.com/",
            "proj-1",
            "us-central1",
        )
        .expect("config");
        let model = ModelName::try_new("gemini-2.5-pro").expect("model");
        assert_eq!(
            config.model_url(&model).expect("url").as_str(),
            "https://us-central1-aiplatform.googleapis.com/v1/projects/proj-1/locations/us-central1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn model_name_with_slash_is_rejected() {
        let config =
            GeminiConfig::try_new("https://generativelanguage.googleapis.com").expect("config");
        let model = ModelName::try_new("models/x").expect("model");
        assert_eq!(
            config.model_url(&model).expect_err("slash").code(),
            crate::error::GEMINI_CONFIG_INVALID
        );
    }

    #[test]
    fn secret_header_rejects_provider_owned_names() {
        let secret = SecretString::try_new(CANARY).expect("secret");
        for name in ["x-goog-api-key", "authorization", "content-type"] {
            assert!(SecretHeader::try_new(name, secret.clone()).is_err());
        }
    }

    #[test]
    fn capabilities_reflect_flags() {
        let model = GeminiModelConfig::try_new("gemini-2.5-pro", 1_000_000, 128_000, 8_192)
            .expect("model")
            .with_input_images(true)
            .with_input_audio(true)
            .with_input_files(true)
            .with_google_search(true)
            .with_code_execution(true);
        let caps = model.capabilities("gemini");
        assert!(caps.input.images);
        assert!(caps.input.audio);
        assert!(caps.input.files);
        assert!(
            caps.native_capabilities
                .contains(&Arc::from("gemini.google-search"))
        );
        assert!(
            caps.native_capabilities
                .contains(&Arc::from("gemini.code-execution"))
        );
        assert!(
            caps.native_capabilities
                .contains(&Arc::from("gemini.generate-content"))
        );
        assert!(caps.native_capabilities.contains(&Arc::from("gemini.sse")));
    }

    #[test]
    fn apply_capabilities_round_trips() {
        let mut model = GeminiModelConfig::try_new("gemini-2.5-pro", 1_000_000, 128_000, 8_192)
            .expect("model")
            .with_google_search(true)
            .with_thinking(true, 1_024)
            .with_cached_content(true);
        let before = model.capabilities("gemini");
        model.apply_capabilities(&before).expect("apply");
        let after = model.capabilities("gemini");
        assert_eq!(before, after);
    }
}
