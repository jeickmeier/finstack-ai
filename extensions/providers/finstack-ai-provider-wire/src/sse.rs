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
    /// Scan watermark: no complete event boundary lies entirely within
    /// `buffer[..scanned]`. Each push resumes from `scanned - 3` so a separator
    /// split across pushes is still found, and the watermark is reset to the
    /// retained tail length (zero once the buffer drains) after every
    /// successful push; error paths leave it untouched.
    scanned: usize,
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
            scanned: 0,
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
        let mut from = self.scanned.saturating_sub(3);
        while let Some((boundary, separator_len)) = event_boundary(&self.buffer, cursor, from) {
            if boundary - cursor > self.max_event_bytes {
                return Err(SseParseError::Limit);
            }
            frames.push(cursor..boundary);
            cursor = boundary + separator_len;
            from = cursor;
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
        self.scanned = self.buffer.len();
        Ok(events)
    }

    /// Whether any unfinished non-whitespace bytes remain.
    #[must_use]
    pub fn finish_clean(&self) -> bool {
        self.buffer.iter().all(u8::is_ascii_whitespace)
    }
}

/// Locate the earliest `\n\n` or `\r\n\r\n` separator starting at or after
/// `cursor`, visiting only `\n` bytes from `from` onwards.
///
/// Returns the absolute separator start and its length. A CRLF separator is
/// recognised at its first `\n`, so a caller resuming at most three bytes
/// before the previously scanned end never misses a separator split across
/// pushes. Mixed `\n\r\n` sequences are intentionally not separators.
fn event_boundary(bytes: &[u8], cursor: usize, from: usize) -> Option<(usize, usize)> {
    let mut index = from.max(cursor);
    while let Some(offset) = bytes.get(index..)?.iter().position(|&byte| byte == b'\n') {
        let lf = index + offset;
        if let Some(start) = lf.checked_sub(1).filter(|start| *start >= cursor)
            && bytes.get(start..start + 4) == Some(b"\r\n\r\n".as_slice())
        {
            return Some((start, 4));
        }
        if bytes.get(lf + 1) == Some(&b'\n') {
            return Some((lf, 2));
        }
        index = lf + 1;
    }
    None
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

    fn event(name: Option<&str>, data: &str) -> SseEvent {
        SseEvent {
            name: name.map(str::to_owned),
            data: data.to_owned(),
        }
    }

    #[test]
    fn boundary_split_across_pushes() {
        let cases: [(&[u8], &[u8]); 3] = [
            (b"data: x\n", b"\n"),
            (b"data: x\r\n\r", b"\n"),
            (b"data: x\r\n", b"\r\n"),
        ];
        for (first, second) in cases {
            let mut parser = SseEventParser::new(256, 1_024);
            assert!(parser.push(first).unwrap().is_empty());
            assert_eq!(parser.push(second).unwrap(), vec![event(None, "x")]);
            assert!(parser.finish_clean());
            assert_eq!(parser.scanned, 0);
        }
    }

    /// Fixture of 200 events mixing LF and CRLF separators, named and unnamed
    /// events, multi-line `data:`, comment lines, a trailing `\r` kept inside a
    /// frame (`\r\n\n`), and a terminal `[DONE]`.
    fn mixed_fixture() -> Vec<u8> {
        let mut fixture = Vec::new();
        for index in 0..200 {
            match index % 5 {
                0 => fixture.extend_from_slice(format!("data: {{\"n\":{index}}}\n\n").as_bytes()),
                1 => fixture.extend_from_slice(
                    format!("event: delta\r\ndata: {{\"n\":{index}}}\r\n\r\n").as_bytes(),
                ),
                2 => fixture.extend_from_slice(
                    format!(": keep-alive\ndata: first {index}\ndata: second {index}\n\n")
                        .as_bytes(),
                ),
                3 => fixture.extend_from_slice(format!("data: cr {index}\r\n\n").as_bytes()),
                _ if index == 199 => fixture.extend_from_slice(b"data: [DONE]\n\n"),
                _ => fixture
                    .extend_from_slice(format!("event: ping\r\ndata: {index}\r\n\r\n").as_bytes()),
            }
        }
        fixture
    }

    #[test]
    fn many_small_pushes_match_single_push() {
        let fixture = mixed_fixture();
        let single = SseEventParser::new(usize::MAX, usize::MAX)
            .push(&fixture)
            .unwrap();
        assert_eq!(single.len(), 200);
        assert_eq!(single[1], event(Some("delta"), "{\"n\":1}"));
        assert_eq!(single[2], event(None, "first 2\nsecond 2"));
        assert_eq!(single[3], event(None, "cr 3\r"));
        assert_eq!(single[199], event(None, "[DONE]"));

        let mut parser = SseEventParser::new(usize::MAX, usize::MAX);
        let mut many = Vec::new();
        for byte in &fixture {
            many.extend(parser.push(core::slice::from_ref(byte)).unwrap());
        }
        assert_eq!(many, single);
        assert!(parser.finish_clean());
    }

    #[test]
    fn watermark_resets_after_drain() {
        let mut parser = SseEventParser::new(256, 1_024);
        assert_eq!(parser.push(b"data: x\n\n").unwrap(), vec![event(None, "x")]);
        assert_eq!(parser.scanned, 0);
        assert!(parser.push(b"data: y").unwrap().is_empty());
        assert_eq!(parser.scanned, 7);
        assert_eq!(parser.push(b"\n\ndata: z").unwrap(), vec![event(None, "y")]);
        assert_eq!(parser.scanned, 7);
        assert_eq!(parser.push(b"\n\n").unwrap(), vec![event(None, "z")]);
        assert_eq!(parser.scanned, 0);
        assert!(parser.finish_clean());
    }

    #[test]
    fn mixed_line_endings_and_limits_are_preserved() {
        let mut parser = SseEventParser::new(256, 1_024);
        assert!(parser.push(b"data: a\n\r\n").unwrap().is_empty());
        assert!(!parser.finish_clean());
        assert_eq!(parser.push(b"\n").unwrap(), vec![event(None, "a")]);

        let mut parser = SseEventParser::new(4, 1_024);
        assert!(parser.push(b"da").unwrap().is_empty());
        assert!(parser.push(b"ta").unwrap().is_empty());
        assert_eq!(parser.push(b":"), Err(SseParseError::Limit));
    }
}
