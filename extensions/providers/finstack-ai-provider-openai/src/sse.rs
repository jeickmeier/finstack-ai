//! Incremental, bounded official `OpenAI` Responses Server-Sent Events framing.

use finstack_ai_runtime::{ModelError, SseFrameParser};

use crate::error::{stream_error, stream_limit_error};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub(crate) data: String,
}

pub(crate) struct SseParser {
    frames: SseFrameParser,
}

impl SseParser {
    pub(crate) fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            frames: SseFrameParser::new(max_event_bytes, max_stream_bytes),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, ModelError> {
        let frames = self.frames.push(bytes).map_err(|_| stream_limit_error())?;
        let mut events = Vec::new();
        for frame in frames {
            if let Some(event) = parse_frame(&frame)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub(crate) fn finish(self) -> Result<(), ModelError> {
        if self.frames.finish_clean() {
            Ok(())
        } else {
            Err(stream_error("OpenAI SSE stream ended mid-event"))
        }
    }
}

fn parse_frame(bytes: &[u8]) -> Result<Option<SseEvent>, ModelError> {
    let frame =
        core::str::from_utf8(bytes).map_err(|_| stream_error("OpenAI SSE event is not UTF-8"))?;
    let mut data = String::new();
    for line in frame.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if data.is_empty() || data == "[DONE]" {
        // `[DONE]` is not a Responses success signal.
        Ok(None)
    } else {
        Ok(Some(SseEvent { data }))
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
            vec![SseEvent {
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
