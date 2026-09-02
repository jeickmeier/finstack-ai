//! Target-neutral SSE framing and event-field parsing.

/// One parsed SSE event after `event:` / `data:` field splitting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Optional `event:` name. `OpenAI` Responses omits this; Anthropic requires it.
    pub name: Option<String>,
    /// Concatenated `data:` payload.
    pub data: String,
}

/// SSE field-parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseParseError {
    /// An event or the accumulated stream exceeded its configured ceiling.
    Limit,
    /// A frame was not valid UTF-8.
    InvalidUtf8,
    /// A named-event frame declared more than one `event:` field.
    DuplicateName,
}

/// Incremental SSE event parser used by every first-party wire protocol.
#[derive(Debug)]
pub struct SseEventParser {
    buffer: Vec<u8>,
    total_bytes: usize,
    max_event_bytes: usize,
    max_stream_bytes: usize,
}

impl SseEventParser {
    /// Construct a bounded event parser.
    #[must_use]
    pub const fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            total_bytes: 0,
            max_event_bytes,
            max_stream_bytes,
        }
    }

    /// Push bytes and return complete events.
    ///
    /// # Errors
    ///
    /// Returns [`SseParseError`] when framing, UTF-8, or event-name rules fail.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, SseParseError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or(SseParseError::Limit)?;
        if self.total_bytes > self.max_stream_bytes {
            return Err(SseParseError::Limit);
        }
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut cursor = 0;
        while let Some((boundary, separator_len)) = event_boundary(&self.buffer[cursor..]) {
            if boundary > self.max_event_bytes {
                return Err(SseParseError::Limit);
            }
            frames.push(cursor..cursor + boundary);
            cursor += boundary + separator_len;
        }
        if self.buffer.len() - cursor > self.max_event_bytes {
            return Err(SseParseError::Limit);
        }
        let mut events = Vec::new();
        for frame in frames {
            if let Some(event) = parse_frame(&self.buffer[frame])? {
                events.push(event);
            }
        }
        self.buffer.drain(..cursor);
        Ok(events)
    }

    /// Whether any unfinished non-whitespace bytes remain.
    #[must_use]
    pub fn finish_clean(&self) -> bool {
        self.buffer.iter().all(u8::is_ascii_whitespace)
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

fn parse_frame(bytes: &[u8]) -> Result<Option<SseEvent>, SseParseError> {
    let frame = core::str::from_utf8(bytes).map_err(|_| SseParseError::InvalidUtf8)?;
    let mut name = None;
    let mut data = String::new();
    for line in frame.lines() {
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("event:") {
            if name.is_some() {
                return Err(SseParseError::DuplicateName);
            }
            name = Some(value.strip_prefix(' ').unwrap_or(value).to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if name.is_none() && data.is_empty() {
        return Ok(None);
    }
    Ok(Some(SseEvent { name, data }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_named_and_unnamed_events() {
        let mut parser = SseEventParser::new(256, 1_024);
        assert!(parser.push(b"data: {\"id\":").unwrap().is_empty());
        assert_eq!(
            parser.push(b"\"one\"}\r\n\r\ndata: [DONE]\n\n").unwrap(),
            vec![
                SseEvent {
                    name: None,
                    data: "{\"id\":\"one\"}".to_owned(),
                },
                SseEvent {
                    name: None,
                    data: "[DONE]".to_owned(),
                },
            ]
        );
        assert!(parser.finish_clean());
    }

    #[test]
    fn enforces_event_and_total_limits() {
        assert_eq!(
            SseEventParser::new(3, 16).push(b"data: value"),
            Err(SseParseError::Limit)
        );
        assert_eq!(
            SseEventParser::new(32, 4).push(b"12345"),
            Err(SseParseError::Limit)
        );
    }
}
