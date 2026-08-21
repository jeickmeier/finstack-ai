//! Incremental, bounded Anthropic Server-Sent Events framing.

use finstack_ai_provider_wire::{SseEvent, SseEventParser, SseParseError};
use finstack_ai_runtime::ModelError;

use crate::error::{stream_error, stream_limit_error};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NamedSseEvent {
    pub(crate) name: String,
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

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<NamedSseEvent>, ModelError> {
        let events = self.inner.push(bytes).map_err(map_parse)?;
        let mut named = Vec::new();
        for SseEvent { name, data } in events {
            let Some(name) = name.filter(|value| !value.is_empty()) else {
                return Err(stream_error("Anthropic SSE event omitted its name"));
            };
            named.push(NamedSseEvent { name, data });
        }
        Ok(named)
    }

    pub(crate) fn finish(self) -> Result<(), ModelError> {
        if self.inner.finish_clean() {
            Ok(())
        } else {
            Err(stream_error("Anthropic SSE stream ended mid-event"))
        }
    }
}

fn map_parse(error: SseParseError) -> ModelError {
    match error {
        SseParseError::Limit => stream_limit_error(),
        SseParseError::InvalidUtf8 => stream_error("Anthropic SSE event is not UTF-8"),
        SseParseError::DuplicateName => stream_error("Anthropic SSE event declared multiple names"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_named_events() {
        let mut parser = SseParser::new(256, 1_024);
        assert!(parser.push(b"event: message_start\n").unwrap().is_empty());
        assert_eq!(
            parser
                .push(b"data: {\"id\":\"one\"}\r\n\r\nevent: ping\ndata: {\"type\":\"ping\"}\n\n")
                .unwrap(),
            vec![
                NamedSseEvent {
                    name: "message_start".to_owned(),
                    data: "{\"id\":\"one\"}".to_owned(),
                },
                NamedSseEvent {
                    name: "ping".to_owned(),
                    data: "{\"type\":\"ping\"}".to_owned(),
                },
            ]
        );
        parser.finish().unwrap();
    }

    #[test]
    fn enforces_event_and_total_limits() {
        assert_eq!(
            SseParser::new(3, 16).push(b"event: x"),
            Err(stream_limit_error())
        );
        assert_eq!(
            SseParser::new(32, 4).push(b"12345"),
            Err(stream_limit_error())
        );
    }

    #[test]
    fn unnamed_data_frames_fail_closed() {
        let mut parser = SseParser::new(128, 1_024);
        assert_eq!(
            parser.push(b"data: {\"id\":\"one\"}\n\n"),
            Err(stream_error("Anthropic SSE event omitted its name"))
        );
    }
}
