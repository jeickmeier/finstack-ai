//! Target-neutral SSE framing and event-field parsing.
//!
//! [`SseEventParser`] splits frames into `event:` / `data:` fields;
//! [`DataSseParser`] and [`NamedSseParser`] apply the per-protocol frame
//! rules on top and implement [`StreamProtocol`] for the shared driver.

use crate::StreamNormError;
use crate::stream::StreamProtocol;

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

/// Bounded SSE parser yielding `data:` payloads.
///
/// By default `event:` names are ignored, empty frames are dropped, and the
/// `[DONE]` sentinel is swallowed, which is the `OpenAI` Responses and
/// `OpenRouter` framing. [`DataSseParser::rejecting_named_events`] switches
/// to the Gemini rule: a name other than the empty name or `message` is a
/// protocol violation.
///
/// ```
/// use finstack_ai_provider_wire::{DataSseParser, StreamProtocol};
///
/// let mut parser = DataSseParser::new(128, 1_024);
/// assert!(parser.push(b"data: {\"id\":").unwrap().is_empty());
/// assert_eq!(
///     parser.push(b"\"one\"}\n\ndata: [DONE]\n\n").unwrap(),
///     vec!["{\"id\":\"one\"}".to_owned()]
/// );
/// assert!(parser.finish().unwrap().is_empty());
/// ```
#[derive(Debug)]
pub struct DataSseParser {
    inner: SseEventParser,
    reject_named: bool,
}

impl DataSseParser {
    /// Construct a parser bounded by a per-event ceiling and a stream budget.
    #[must_use]
    pub const fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            inner: SseEventParser::new(max_event_bytes, max_stream_bytes),
            reject_named: false,
        }
    }

    /// Reject any `event:` name other than the empty name or `message`.
    #[must_use]
    pub const fn rejecting_named_events(mut self) -> Self {
        self.reject_named = true;
        self
    }
}

impl StreamProtocol for DataSseParser {
    type Frame = String;

    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, StreamNormError> {
        let events = self.inner.push(bytes)?;
        let mut payloads = Vec::new();
        for SseEvent { name, data } in events {
            if self.reject_named
                && name
                    .as_deref()
                    .is_some_and(|name| !name.is_empty() && name != "message")
            {
                return Err(StreamNormError::stream(
                    "SSE event declared an unexpected name",
                ));
            }
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            payloads.push(data);
        }
        Ok(payloads)
    }

    fn finish(self) -> Result<Vec<String>, StreamNormError> {
        finish_clean(&self.inner)
    }
}

/// One SSE event whose `event:` name is mandatory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedSseEvent {
    /// Non-empty `event:` name.
    pub name: String,
    /// Concatenated `data:` payload.
    pub data: String,
}

/// Bounded SSE parser for protocols that name every event (Anthropic Messages).
///
/// A frame without a non-empty `event:` field is a protocol violation.
#[derive(Debug)]
pub struct NamedSseParser {
    inner: SseEventParser,
}

impl NamedSseParser {
    /// Construct a parser bounded by a per-event ceiling and a stream budget.
    #[must_use]
    pub const fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            inner: SseEventParser::new(max_event_bytes, max_stream_bytes),
        }
    }
}

impl StreamProtocol for NamedSseParser {
    type Frame = NamedSseEvent;

    fn push(&mut self, bytes: &[u8]) -> Result<Vec<NamedSseEvent>, StreamNormError> {
        let events = self.inner.push(bytes)?;
        let mut named = Vec::with_capacity(events.len());
        for SseEvent { name, data } in events {
            let Some(name) = name.filter(|value| !value.is_empty()) else {
                return Err(StreamNormError::stream("SSE event omitted its name"));
            };
            named.push(NamedSseEvent { name, data });
        }
        Ok(named)
    }

    fn finish(self) -> Result<Vec<NamedSseEvent>, StreamNormError> {
        finish_clean(&self.inner)
    }
}

fn finish_clean<T>(parser: &SseEventParser) -> Result<Vec<T>, StreamNormError> {
    if parser.finish_clean() {
        Ok(Vec::new())
    } else {
        Err(StreamNormError::stream("SSE stream ended mid-event"))
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

    fn limit() -> StreamNormError {
        StreamNormError::limit()
    }

    #[test]
    fn data_parser_ignores_done_and_names_by_default() {
        let mut parser = DataSseParser::new(128, 1_024);
        assert!(parser.push(b"data: {\"id\":").unwrap().is_empty());
        assert_eq!(
            parser
                .push(b"\"one\"}\r\n\r\nevent: ping\ndata: {}\n\ndata: [DONE]\n\n")
                .unwrap(),
            vec!["{\"id\":\"one\"}".to_owned(), "{}".to_owned()]
        );
        assert!(parser.push(b": comment\n\n").unwrap().is_empty());
        assert!(parser.finish().unwrap().is_empty());
    }

    #[test]
    fn data_parser_enforces_limits_and_clean_eof() {
        assert_eq!(DataSseParser::new(3, 16).push(b"data: value"), Err(limit()));
        assert_eq!(DataSseParser::new(32, 4).push(b"12345"), Err(limit()));
        let mut parser = DataSseParser::new(128, 1_024);
        assert!(parser.push(b"data: partial").unwrap().is_empty());
        assert_eq!(
            parser.finish(),
            Err(StreamNormError::stream("SSE stream ended mid-event"))
        );
    }

    #[test]
    fn data_parser_rejecting_names_accepts_only_message() {
        let mut parser = DataSseParser::new(128, 4_096).rejecting_named_events();
        assert_eq!(
            parser
                .push(b"event: message\ndata: {\"id\":\"one\"}\n\ndata: {\"id\":\"two\"}\n\n")
                .unwrap(),
            vec!["{\"id\":\"one\"}".to_owned(), "{\"id\":\"two\"}".to_owned()]
        );
        assert_eq!(
            parser.push(b"event: ping\ndata: {}\n\n"),
            Err(StreamNormError::stream(
                "SSE event declared an unexpected name"
            ))
        );
    }

    #[test]
    fn data_parser_stream_budget_is_cumulative() {
        let frame = |index: usize| {
            format!("data: {{\"i\":{index},\"p\":\"{}\"}}\n\n", "x".repeat(800)).into_bytes()
        };
        let mut parser = DataSseParser::new(64 * 1024, 1_048_576);
        let parsed: usize = (0..100)
            .map(|index| parser.push(&frame(index)).unwrap().len())
            .sum();
        assert_eq!(parsed, 100);

        let mut parser = DataSseParser::new(64 * 1024, 4_096);
        let result = (0..10)
            .map(|index| parser.push(&frame(index)))
            .find(Result::is_err);
        assert_eq!(result, Some(Err(limit())));
    }

    #[test]
    fn named_parser_requires_names() {
        let mut parser = NamedSseParser::new(256, 1_024);
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
        assert!(parser.finish().unwrap().is_empty());

        let mut parser = NamedSseParser::new(128, 1_024);
        assert_eq!(
            parser.push(b"data: {\"id\":\"one\"}\n\n"),
            Err(StreamNormError::stream("SSE event omitted its name"))
        );
        assert_eq!(NamedSseParser::new(3, 16).push(b"event: x"), Err(limit()));
    }
}
