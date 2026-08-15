//! Incremental, bounded Anthropic Server-Sent Events framing.

use finstack_ai_runtime::ModelError;

use crate::error::{stream_error, stream_limit_error};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub(crate) name: String,
    pub(crate) data: String,
}

pub(crate) struct SseParser {
    buffer: Vec<u8>,
    total_bytes: usize,
    max_event_bytes: usize,
    max_stream_bytes: usize,
}

impl SseParser {
    pub(crate) fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            total_bytes: 0,
            max_event_bytes,
            max_stream_bytes,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, ModelError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or_else(stream_limit_error)?;
        if self.total_bytes > self.max_stream_bytes {
            return Err(stream_limit_error());
        }
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some((boundary, separator_len)) = event_boundary(&self.buffer) {
            if boundary > self.max_event_bytes {
                return Err(stream_limit_error());
            }
            let remainder = self.buffer.split_off(boundary + separator_len);
            let frame = core::mem::replace(&mut self.buffer, remainder);
            if let Some(event) = parse_frame(&frame[..boundary])? {
                events.push(event);
            }
        }
        if self.buffer.len() > self.max_event_bytes {
            return Err(stream_limit_error());
        }
        Ok(events)
    }

    pub(crate) fn finish(self) -> Result<(), ModelError> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err(stream_error("Anthropic SSE stream ended mid-event"))
        }
    }
}

fn event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| (position, 2));
    let crlf = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4));
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 < right.0 { left } else { right }),
        (left, right) => left.or(right),
    }
}

fn parse_frame(bytes: &[u8]) -> Result<Option<SseEvent>, ModelError> {
    let frame = core::str::from_utf8(bytes)
        .map_err(|_| stream_error("Anthropic SSE event is not UTF-8"))?;
    let mut name = String::new();
    let mut data = String::new();
    for line in frame.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            if !name.is_empty() {
                return Err(stream_error("Anthropic SSE event declared multiple names"));
            }
            name.push_str(value.strip_prefix(' ').unwrap_or(value));
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if name.is_empty() && data.is_empty() {
        return Ok(None);
    }
    if name.is_empty() {
        return Err(stream_error("Anthropic SSE event omitted its name"));
    }
    Ok(Some(SseEvent { name, data }))
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
                SseEvent {
                    name: "message_start".to_owned(),
                    data: "{\"id\":\"one\"}".to_owned(),
                },
                SseEvent {
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
