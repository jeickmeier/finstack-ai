//! Incremental, bounded official `OpenAI` Responses Server-Sent Events framing.

use finstack_ai_provider_wire::{SseEvent, SseEventParser, SseParseError};
use finstack_ai_runtime::ports::model::ModelError;

use crate::error::{stream_error, stream_limit_error};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SseEventData {
    pub(crate) data: String,
}

pub(crate) struct SseParser {
    inner: SseEventParser,
}

impl SseParser {
    pub(crate) fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            inner: SseEventParser::new(max_event_bytes, max_stream_bytes),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEventData>, ModelError> {
        let events = self.inner.push(bytes).map_err(map_parse)?;
        Ok(events
            .into_iter()
            .filter(|event| !event.data.is_empty() && event.data != "[DONE]")
            .map(|SseEvent { data, .. }| SseEventData { data })
            .collect())
    }

    pub(crate) fn finish(self) -> Result<(), ModelError> {
        if self.inner.finish_clean() {
            Ok(())
        } else {
            Err(stream_error("OpenAI SSE stream ended mid-event"))
        }
    }
}

fn map_parse(error: SseParseError) -> ModelError {
    match error {
        SseParseError::Limit => stream_limit_error(),
        SseParseError::InvalidUtf8 => stream_error("OpenAI SSE event is not UTF-8"),
        SseParseError::DuplicateName => stream_error("OpenAI SSE event declared multiple names"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_json_and_ignores_done() {
        let mut parser = SseParser::new(128, 1_024);
        assert!(parser.push(b"data: {\"id\":").unwrap().is_empty());
        assert_eq!(
            parser.push(b"\"one\"}\r\n\r\ndata: [DONE]\n\n").unwrap(),
            vec![SseEventData {
                data: "{\"id\":\"one\"}".to_owned(),
            }]
        );
        parser.finish().unwrap();
    }

    #[test]
    fn enforces_event_and_total_limits() {
        assert_eq!(
            SseParser::new(3, 16).push(b"data: value"),
            Err(stream_limit_error())
        );
        assert_eq!(
            SseParser::new(32, 4).push(b"12345"),
            Err(stream_limit_error())
        );
    }
}
