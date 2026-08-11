//! Versioned compatibility facts for known OpenAI-compatible endpoint families.

/// Current endpoint-quirks table version.
pub const ENDPOINT_QUIRKS_VERSION: u16 = 1;

/// Known endpoint family used to select conservative wire behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// `OpenAI`'s Chat Completions API.
    OpenAi,
    /// Azure-style deployment-scoped `OpenAI` endpoint.
    AzureOpenAi,
    /// vLLM's OpenAI-compatible server.
    Vllm,
    /// Ollama's OpenAI-compatible endpoint.
    Ollama,
    /// LM Studio's OpenAI-compatible endpoint.
    LmStudio,
    /// An application-configured OpenAI-compatible gateway.
    Gateway,
}

/// Authentication convention documented by an endpoint family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationConvention {
    /// OpenAI-style `Authorization: Bearer` authentication.
    Bearer,
    /// Azure-style `api-key` or bearer authentication.
    ApiKeyOrBearer,
    /// Authentication is endpoint-configured and may be absent locally.
    Configurable,
}

/// Conservative versioned compatibility flags for one endpoint family.
///
/// These flags prevent the provider from claiming every compatible endpoint is
/// identical. Callers can inspect the exact table version captured by a
/// configured provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointQuirks {
    /// Quirks table version.
    pub version: u16,
    /// Endpoint family.
    pub kind: EndpointKind,
    /// Conventional authentication form.
    pub authentication: AuthenticationConvention,
    /// Whether the family accepts the Chat Completions `developer` role.
    pub developer_role: bool,
    /// Whether the provider should request a final usage-only stream chunk.
    pub stream_usage: bool,
    /// Whether native JSON-schema response formatting is enabled.
    pub native_structured_output: bool,
}

impl EndpointQuirks {
    /// Return the checked-in conservative row for `kind`.
    #[must_use]
    pub const fn for_kind(kind: EndpointKind) -> Self {
        match kind {
            EndpointKind::OpenAi => Self {
                version: ENDPOINT_QUIRKS_VERSION,
                kind,
                authentication: AuthenticationConvention::Bearer,
                developer_role: true,
                stream_usage: true,
                native_structured_output: true,
            },
            EndpointKind::AzureOpenAi => Self {
                version: ENDPOINT_QUIRKS_VERSION,
                kind,
                authentication: AuthenticationConvention::ApiKeyOrBearer,
                developer_role: true,
                stream_usage: true,
                native_structured_output: true,
            },
            EndpointKind::Vllm => Self {
                version: ENDPOINT_QUIRKS_VERSION,
                kind,
                authentication: AuthenticationConvention::Configurable,
                developer_role: false,
                stream_usage: true,
                native_structured_output: true,
            },
            EndpointKind::Ollama | EndpointKind::LmStudio | EndpointKind::Gateway => Self {
                version: ENDPOINT_QUIRKS_VERSION,
                kind,
                authentication: AuthenticationConvention::Configurable,
                developer_role: false,
                stream_usage: false,
                native_structured_output: false,
            },
        }
    }
}
