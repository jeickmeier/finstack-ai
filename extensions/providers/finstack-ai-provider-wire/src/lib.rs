//! Target-neutral wire framing and response assembly for first-party providers.
//!
//! This support crate is an implementation detail of the provider leaves. It
//! depends on the provider-neutral runtime model contract, while the runtime
//! remains independent of vendor protocols.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
#![doc(test(attr(allow(clippy::expect_used))))]

mod anthropic_messages;
mod gemini_generate_content;
mod ndjson;
mod ollama_chat;
mod openai_responses;
mod sse;

pub use anthropic_messages::AnthropicMessagesAssembly;
pub use gemini_generate_content::{
    GEMINI_CACHED_TOKENS_KEY, GEMINI_CODE_RESULT_MEDIA_TYPE, GEMINI_CONTINUATION_PROVIDER,
    GEMINI_EXECUTABLE_CODE_MEDIA_TYPE, GEMINI_GROUNDING_MEDIA_TYPE, GEMINI_THOUGHTS_TOKENS_KEY,
    GeminiGenerateContentAssembly,
};
pub use ndjson::{NdjsonError, NdjsonParser};
pub use ollama_chat::{OllamaChatAssembly, OllamaReplayEntry};
pub use openai_responses::OpenAiResponsesAssembly;
pub use sse::{SseEvent, SseEventParser, SseParseError};

/// Kind of a provider wire-normalization failure.
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

/// Safe provider wire-normalization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamNormError {
    /// Failure class used by leaves to select a stable adapter code.
    pub kind: StreamNormKind,
    /// Safe, non-secret diagnostic.
    pub message: &'static str,
}

impl StreamNormError {
    const fn limit() -> Self {
        Self {
            kind: StreamNormKind::Limit,
            message: "provider response exceeded a configured stream limit",
        }
    }

    const fn stream(message: &'static str) -> Self {
        Self {
            kind: StreamNormKind::Stream,
            message,
        }
    }

    const fn response(message: &'static str) -> Self {
        Self {
            kind: StreamNormKind::Response,
            message,
        }
    }

    const fn incomplete(message: &'static str) -> Self {
        Self {
            kind: StreamNormKind::Incomplete,
            message,
        }
    }
}
