//! Target-neutral, bounded NDJSON line framing.

use super::StreamNormError;

/// Incremental NDJSON splitter. Object interpretation stays in the caller.
pub struct NdjsonParser {
    buffer: Vec<u8>,
    total_bytes: usize,
    max_event_bytes: usize,
    max_stream_bytes: usize,
}

/// NDJSON framing failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdjsonError {
    /// A line or the accumulated stream exceeded its configured ceiling.
    Limit,
    /// A line was not valid UTF-8.
    InvalidUtf8,
}

impl From<NdjsonError> for StreamNormError {
    fn from(error: NdjsonError) -> Self {
        match error {
            NdjsonError::Limit => Self::limit(),
            NdjsonError::InvalidUtf8 => Self::stream("NDJSON line is not UTF-8"),
        }
    }
}

impl NdjsonParser {
    /// Construct a bounded line parser.
    #[must_use]
    pub const fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            total_bytes: 0,
            max_event_bytes,
            max_stream_bytes,
        }
    }

    /// Push bytes and return complete non-empty lines without their terminators.
    ///
    /// # Errors
    ///
    /// Returns [`NdjsonError`] when a line, the stream, or UTF-8 validation fails.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, NdjsonError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or(NdjsonError::Limit)?;
        if self.total_bytes > self.max_stream_bytes {
            return Err(NdjsonError::Limit);
        }
        self.buffer.extend_from_slice(bytes);
        let mut lines = Vec::new();
        let mut cursor = 0;
        while let Some(relative) = self.buffer[cursor..].iter().position(|byte| *byte == b'\n') {
            let end = cursor + relative;
            if end - cursor > self.max_event_bytes {
                return Err(NdjsonError::Limit);
            }
            if let Some(line) = parse_line(&self.buffer[cursor..end])? {
                lines.push(line);
            }
            cursor = end + 1;
        }
        if cursor > 0 {
            self.buffer.drain(..cursor);
        }
        if self.buffer.len() > self.max_event_bytes {
            return Err(NdjsonError::Limit);
        }
        Ok(lines)
    }

    /// Emit a final line that lacked a trailing newline.
    ///
    /// # Errors
    ///
    /// Returns [`NdjsonError`] when the leftover line is oversized or not UTF-8.
    pub fn finish(self) -> Result<Vec<String>, NdjsonError> {
        if self.buffer.len() > self.max_event_bytes {
            return Err(NdjsonError::Limit);
        }
        match parse_line(&self.buffer)? {
            Some(line) => Ok(vec![line]),
            None => Ok(Vec::new()),
        }
    }
}

fn parse_line(bytes: &[u8]) -> Result<Option<String>, NdjsonError> {
    let line = core::str::from_utf8(bytes).map_err(|_| NdjsonError::InvalidUtf8)?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.chars().all(char::is_whitespace) {
        return Ok(None);
    }
    Ok(Some(line.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_ndjson_lines() {
        let mut parser = NdjsonParser::new(256, 1_024);
        assert!(parser.push(b"{\"done\":fals").unwrap().is_empty());
        assert_eq!(
            parser.push(b"e}\n{\"done\":true}\n").unwrap(),
            vec!["{\"done\":false}".to_owned(), "{\"done\":true}".to_owned()]
        );
        assert!(parser.finish().unwrap().is_empty());
    }

    #[test]
    fn finish_emits_a_final_line_without_newline() {
        let mut parser = NdjsonParser::new(256, 1_024);
        assert!(parser.push(b"{\"done\":true}").unwrap().is_empty());
        assert_eq!(parser.finish().unwrap(), vec!["{\"done\":true}".to_owned()]);
    }

    #[test]
    fn enforces_line_and_total_limits() {
        assert_eq!(
            NdjsonParser::new(3, 16).push(b"1234"),
            Err(NdjsonError::Limit)
        );
        assert_eq!(
            NdjsonParser::new(32, 4).push(b"12345"),
            Err(NdjsonError::Limit)
        );
    }
}
