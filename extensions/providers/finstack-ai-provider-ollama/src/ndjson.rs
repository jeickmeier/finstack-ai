//! Incremental, bounded Ollama NDJSON line framing.

use finstack_ai_runtime::{ModelError, NdjsonError, NdjsonParser as SharedNdjsonParser};

use crate::error::{stream_error, stream_limit_error};

pub(crate) struct NdjsonParser {
    inner: SharedNdjsonParser,
}

impl NdjsonParser {
    pub(crate) const fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            inner: SharedNdjsonParser::new(max_event_bytes, max_stream_bytes),
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ModelError> {
        self.inner.push(bytes).map_err(map_error)
    }

    pub(crate) fn finish(self) -> Result<Vec<String>, ModelError> {
        self.inner.finish().map_err(map_error)
    }
}

fn map_error(error: NdjsonError) -> ModelError {
    match error {
        NdjsonError::Limit => stream_limit_error(),
        NdjsonError::InvalidUtf8 => stream_error("Ollama NDJSON line is not UTF-8"),
    }
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
            Err(stream_limit_error())
        );
        assert_eq!(
            NdjsonParser::new(32, 4).push(b"12345"),
            Err(stream_limit_error())
        );
    }
}
