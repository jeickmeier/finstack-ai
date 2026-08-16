//! Shared provider helpers for secret validation and SSE framing.

/// Maximum accepted provider secret length.
pub const SECRET_MAX_BYTES: usize = 16 * 1_024;

/// Whether a configured secret is non-empty, bounded, and NUL-free.
#[must_use]
pub fn secret_is_valid(value: &str) -> bool {
    !value.is_empty() && value.len() <= SECRET_MAX_BYTES && !value.as_bytes().contains(&0)
}

/// Incremental SSE frame splitter. Event interpretation stays in the provider.
#[derive(Debug)]
pub struct SseFrameParser {
    buffer: Vec<u8>,
    total_bytes: usize,
    max_event_bytes: usize,
    max_stream_bytes: usize,
}

impl SseFrameParser {
    /// Construct a bounded frame parser.
    #[must_use]
    pub const fn new(max_event_bytes: usize, max_stream_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            total_bytes: 0,
            max_event_bytes,
            max_stream_bytes,
        }
    }

    /// Push bytes and return complete frames without their separators.
    ///
    /// # Errors
    ///
    /// Returns [`SseFrameError::Limit`] when an event or the stream exceeds its ceiling.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, SseFrameError> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or(SseFrameError::Limit)?;
        if self.total_bytes > self.max_stream_bytes {
            return Err(SseFrameError::Limit);
        }
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut cursor = 0;
        while let Some((boundary, separator_len)) = event_boundary(&self.buffer[cursor..]) {
            if boundary > self.max_event_bytes {
                return Err(SseFrameError::Limit);
            }
            frames.push(self.buffer[cursor..cursor + boundary].to_vec());
            cursor += boundary + separator_len;
        }
        if cursor > 0 {
            self.buffer.drain(..cursor);
        }
        if self.buffer.len() > self.max_event_bytes {
            return Err(SseFrameError::Limit);
        }
        Ok(frames)
    }

    /// Whether any unfinished non-whitespace bytes remain.
    #[must_use]
    pub fn finish_clean(&self) -> bool {
        self.buffer.iter().all(u8::is_ascii_whitespace)
    }
}

/// SSE framing failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseFrameError {
    /// An event or the accumulated stream exceeded its configured ceiling.
    Limit,
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
