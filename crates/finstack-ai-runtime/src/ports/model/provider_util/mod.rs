//! Shared provider helpers for secret validation and stream normalization.

mod anthropic_messages;
mod ndjson;
mod ollama_chat;
mod openai_chat;
mod openai_responses;
mod sse;

pub use anthropic_messages::AnthropicMessagesAssembly;
pub use ndjson::{NdjsonError, NdjsonParser};
pub use ollama_chat::{OllamaChatAssembly, OllamaReplayEntry};
pub use openai_chat::OpenAiChatAssembly;
pub use openai_responses::OpenAiResponsesAssembly;
pub use sse::{
    SECRET_MAX_BYTES, SseEvent, SseEventParser, SseFrameError, SseFrameParser, SseParseError,
    secret_is_valid,
};

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
