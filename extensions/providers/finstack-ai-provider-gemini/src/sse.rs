//! Incremental, bounded Gemini Server-Sent Events framing.
//!
//! Unlike the Anthropic Messages wire protocol, Gemini `streamGenerateContent`
//! frames carry no `event:` field: every frame is an unnamed `data:` line.
//! An explicit event name other than the empty name or `message` is treated
//! as a protocol violation.

use finstack_ai_kernel::ErrorCategory;
use finstack_ai_runtime::{ModelError, SseEvent, SseEventParser, SseParseError};

use crate::error::{GEMINI_STREAM_INVALID, GEMINI_STREAM_LIMIT_EXCEEDED, error};

#[allow(
    dead_code,
    reason = "consumed once streaming request support lands in a later task"
)]
fn stream_error(message: &'static str) -> ModelError {
    error(GEMINI_STREAM_INVALID, ErrorCategory::Model, false, message)
}

#[allow(
    dead_code,
    reason = "consumed once streaming request support lands in a later task"
)]
fn stream_limit_error() -> ModelError {
    error(
        GEMINI_STREAM_LIMIT_EXCEEDED,
        ErrorCategory::Limit,
        false,
        "Gemini response exceeded a configured stream limit",
    )
}

#[allow(
    dead_code,
    reason = "constructed once streaming request support lands in a later task"
)]
pub(crate) struct GeminiSse {
    inner: SseEventParser,
}

#[allow(
    dead_code,
    reason = "called once streaming request support lands in a later task"
)]
impl GeminiSse {
    pub(crate) fn new(max_event_bytes: usize) -> Self {
        Self {
            inner: SseEventParser::new(max_event_bytes, max_event_bytes),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ModelError> {
        let events = self.inner.push(bytes).map_err(map_parse)?;
        let mut payloads = Vec::new();
        for SseEvent { name, data } in events {
            if let Some(name) = name.as_deref()
                && !name.is_empty()
                && name != "message"
            {
                return Err(stream_error("Gemini SSE event declared an unexpected name"));
            }
            if data.is_empty() {
                continue;
            }
            payloads.push(data);
        }
        Ok(payloads)
    }

    pub(crate) fn finish(self) -> Result<(), ModelError> {
        if self.inner.finish_clean() {
            Ok(())
        } else {
            Err(stream_error("Gemini SSE stream ended mid-event"))
        }
    }
}

#[allow(
    dead_code,
    reason = "called once streaming request support lands in a later task"
)]
fn map_parse(error: SseParseError) -> ModelError {
    match error {
        SseParseError::Limit => stream_limit_error(),
        SseParseError::InvalidUtf8 => stream_error("Gemini SSE event is not UTF-8"),
        SseParseError::DuplicateName => stream_error("Gemini SSE event declared multiple names"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_unnamed_events() {
        let mut parser = GeminiSse::new(256);
        assert!(parser.push(b"data: {\"candidates\":").unwrap().is_empty());
        assert_eq!(
            parser
                .push(b"[1]}\r\n\r\ndata: {\"candidates\":[2]}\n\n")
                .unwrap(),
            vec![
                "{\"candidates\":[1]}".to_owned(),
                "{\"candidates\":[2]}".to_owned(),
            ]
        );
        parser.finish().unwrap();
    }

    #[test]
    fn enforces_event_and_total_limits() {
        assert_eq!(
            GeminiSse::new(3).push(b"data: 12345"),
            Err(stream_limit_error())
        );
    }

    #[test]
    fn unnamed_data_frame_is_accepted() {
        let mut parser = GeminiSse::new(128);
        assert_eq!(
            parser.push(b"data: {\"id\":\"one\"}\n\n").unwrap(),
            vec!["{\"id\":\"one\"}".to_owned()]
        );
    }

    #[test]
    fn named_event_other_than_message_is_rejected() {
        let mut parser = GeminiSse::new(128);
        assert_eq!(
            parser.push(b"event: ping\ndata: {}\n\n"),
            Err(stream_error("Gemini SSE event declared an unexpected name"))
        );
    }

    #[test]
    fn named_message_event_is_accepted() {
        let mut parser = GeminiSse::new(128);
        assert_eq!(
            parser
                .push(b"event: message\ndata: {\"id\":\"one\"}\n\n")
                .unwrap(),
            vec!["{\"id\":\"one\"}".to_owned()]
        );
    }

    #[test]
    fn empty_data_frames_are_no_ops() {
        let mut parser = GeminiSse::new(128);
        assert!(parser.push(b": comment\n\n").unwrap().is_empty());
    }
}
