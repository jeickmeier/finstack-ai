//! Shared provider helpers for secret validation and stream normalization.

mod anthropic_messages;
mod credentials;
mod gemini_generate_content;
mod media;
mod ndjson;
mod ollama_chat;
mod openai_responses;
mod secret;
mod sse;

pub use anthropic_messages::AnthropicMessagesAssembly;
pub use credentials::{Authentication, CredentialReference, CredentialRejected, CredentialStore};
pub use gemini_generate_content::{
    GEMINI_CACHED_TOKENS_KEY, GEMINI_CODE_RESULT_MEDIA_TYPE, GEMINI_CONTINUATION_PROVIDER,
    GEMINI_EXECUTABLE_CODE_MEDIA_TYPE, GEMINI_GROUNDING_MEDIA_TYPE, GEMINI_THOUGHTS_TOKENS_KEY,
    GeminiGenerateContentAssembly,
};
pub use media::{
    MediaResolveError, MediaResolveKind, MediaResolver, ResolveDraftMediaError, ResolvedMedia,
    resolve_draft_media,
};
pub use ndjson::{NdjsonError, NdjsonParser};
pub use ollama_chat::{OllamaChatAssembly, OllamaReplayEntry};
pub use openai_responses::OpenAiResponsesAssembly;
pub use secret::{SECRET_MAX_BYTES, SecretRejected, SecretString, secret_is_valid};
pub use sse::{SseEvent, SseEventParser, SseParseError};

/// Budget tiers for the portable `thinking_level` setting.
///
/// Every leaf that honors `thinking_level` maps `low`/`medium`/`high` through
/// this one table so the setting means the same token budget on every
/// provider; returns `None` for values outside the allowlist.
#[must_use]
pub fn thinking_level_budget(level: &str) -> Option<u64> {
    match level {
        "low" => Some(1_024),
        "medium" => Some(4_096),
        "high" => Some(8_192),
        _ => None,
    }
}

/// Kind of a shared stream-normalization failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamNormKind {
    /// An event or the accumulated stream exceeded its configured ceiling.
    Limit,
    /// The wire event could not be framed or decoded.
    Stream,
    /// The decoded payload violated the protocol contract.
    Response,
    /// The provider reported an incomplete generation.
    Incomplete,
}

/// Protocol-neutral stream normalization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamNormError {
    /// Failure class used by leaf crates to select a stable adapter code.
    pub kind: StreamNormKind,
    /// Safe, non-secret diagnostic.
    pub message: &'static str,
}

impl StreamNormError {
    pub(crate) const fn limit() -> Self {
        Self {
            kind: StreamNormKind::Limit,
            message: "provider response exceeded a configured stream limit",
        }
    }

    pub(crate) const fn stream(message: &'static str) -> Self {
        Self {
            kind: StreamNormKind::Stream,
            message,
        }
    }

    pub(crate) const fn response(message: &'static str) -> Self {
        Self {
            kind: StreamNormKind::Response,
            message,
        }
    }

    pub(crate) const fn incomplete(message: &'static str) -> Self {
        Self {
            kind: StreamNormKind::Incomplete,
            message,
        }
    }
}
