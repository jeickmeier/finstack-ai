//! Protocol seams the shared streaming driver is written against.
//!
//! A [`StreamProtocol`] turns raw body bytes into wire frames (SSE `data:`
//! payloads, named SSE events, NDJSON lines); a [`StreamAssembler`] turns
//! frames into normalized [`ModelStreamItem`]s. Both report failures as
//! [`StreamNormError`] so one leaf-neutral mapping (see
//! [`stream_norm_error`](crate::stream_norm_error)) selects the stable adapter
//! code, category, and retryability.

use finstack_ai_runtime::ports::model::ModelStreamItem;

use crate::StreamNormError;
use crate::anthropic_messages::AnthropicMessagesAssembly;
use crate::gemini_generate_content::GeminiGenerateContentAssembly;
use crate::ndjson::NdjsonParser;
use crate::ollama_chat::OllamaChatAssembly;
use crate::openai_responses::OpenAiResponsesAssembly;
use crate::sse::{NamedSseEvent, SseParseError};

/// Incremental, bounded framing of one streaming response body.
pub trait StreamProtocol {
    /// One complete wire frame.
    type Frame;

    /// Push body bytes and return every frame they completed.
    ///
    /// # Errors
    ///
    /// Returns a [`StreamNormError`] when a frame or the accumulated stream
    /// exceeds its ceiling, or when the bytes violate the framing rules.
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Self::Frame>, StreamNormError>;

    /// Observe end of body and return any frame that lacked its terminator.
    ///
    /// # Errors
    ///
    /// Returns a [`StreamNormError`] when the body ended inside a frame that
    /// the protocol cannot complete.
    fn finish(self) -> Result<Vec<Self::Frame>, StreamNormError>;
}

/// Protocol-specific assembly of wire frames into normalized stream items.
pub trait StreamAssembler {
    /// Frame type consumed by this assembler.
    type Frame;

    /// Consume one frame and emit the items it completes.
    ///
    /// Emitting a [`ModelStreamItem::Completed`] item ends the stream: the
    /// driver stops reading the body after forwarding it.
    ///
    /// # Errors
    ///
    /// Returns a [`StreamNormError`] when the frame violates the protocol.
    fn consume(&mut self, frame: &Self::Frame) -> Result<Vec<ModelStreamItem>, StreamNormError>;

    /// Whether the protocol's terminal frame has been consumed even though
    /// no [`ModelStreamItem::Completed`] item was emitted yet.
    ///
    /// Protocols that complete through [`StreamAssembler::finish`] (Ollama's
    /// `done: true`) override this; the driver stops reading once it is true.
    fn is_done(&self) -> bool {
        false
    }

    /// Produce the terminal item once reading stopped without a
    /// [`ModelStreamItem::Completed`] item having been emitted.
    ///
    /// `Ok(Some(item))` forwards one final item; `Ok(None)` ends the stream
    /// silently.
    ///
    /// # Errors
    ///
    /// Returns a [`StreamNormError`] when the body ended before the
    /// protocol's terminal frame or the accumulated response is invalid.
    fn finish(self) -> Result<Option<ModelStreamItem>, StreamNormError>;
}

impl From<SseParseError> for StreamNormError {
    fn from(error: SseParseError) -> Self {
        match error {
            SseParseError::Limit => Self::limit(),
            SseParseError::InvalidUtf8 => Self::stream("SSE event is not UTF-8"),
            SseParseError::DuplicateName => Self::stream("SSE event declared multiple names"),
        }
    }
}

impl StreamProtocol for NdjsonParser {
    type Frame = String;

    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, StreamNormError> {
        Self::push(self, bytes).map_err(StreamNormError::from)
    }

    fn finish(self) -> Result<Vec<String>, StreamNormError> {
        Self::finish(self).map_err(StreamNormError::from)
    }
}

impl StreamAssembler for OpenAiResponsesAssembly {
    type Frame = String;

    fn consume(&mut self, frame: &String) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        OpenAiResponsesAssembly::consume(self, frame)
    }

    fn finish(self) -> Result<Option<ModelStreamItem>, StreamNormError> {
        Err(StreamNormError::stream(
            "SSE stream ended before response.completed",
        ))
    }
}

impl StreamAssembler for AnthropicMessagesAssembly {
    type Frame = NamedSseEvent;

    fn consume(&mut self, frame: &NamedSseEvent) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        AnthropicMessagesAssembly::consume(self, &frame.name, &frame.data)
    }

    fn finish(self) -> Result<Option<ModelStreamItem>, StreamNormError> {
        Err(StreamNormError::stream(
            "SSE stream ended before message_stop",
        ))
    }
}

impl StreamAssembler for GeminiGenerateContentAssembly {
    type Frame = String;

    fn consume(&mut self, frame: &String) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        GeminiGenerateContentAssembly::consume(self, frame)
    }

    fn finish(mut self) -> Result<Option<ModelStreamItem>, StreamNormError> {
        GeminiGenerateContentAssembly::finish(&mut self)?;
        Err(StreamNormError::stream(
            "SSE stream ended before a terminal chunk",
        ))
    }
}

impl StreamAssembler for OllamaChatAssembly {
    type Frame = String;

    fn consume(&mut self, frame: &String) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        OllamaChatAssembly::consume(self, frame)
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn finish(self) -> Result<Option<ModelStreamItem>, StreamNormError> {
        OllamaChatAssembly::finish(self).map(|response| Some(ModelStreamItem::Completed(response)))
    }
}
