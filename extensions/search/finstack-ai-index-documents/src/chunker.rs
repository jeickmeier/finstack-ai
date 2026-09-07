use std::sync::Arc;

use finstack_ai_kernel::Digest;
use finstack_ai_search_core::{SearchError, configuration_digest};
use serde::{Deserialize, Serialize};

/// Versioned deterministic Markdown chunking. Character counts are Unicode
/// scalar values, never UTF-8 byte offsets. Overlap never exceeds 512 characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkerConfig {
    /// Algorithm version. This implementation accepts version 1 only.
    pub version: u32,
    /// Target chunk length in Unicode characters (64..=16,384), default 4,096.
    pub target_chars: usize,
    /// Maximum preceding overlap (0..=512 and at most half the target), default 512.
    pub overlap_chars: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            version: 1,
            target_chars: 4096,
            overlap_chars: 512,
        }
    }
}

impl ChunkerConfig {
    /// Validate the algorithm version and finite target/overlap bounds.
    ///
    /// # Errors
    /// Rejects unsupported versions or invalid chunk sizes.
    pub fn validate(&self) -> Result<(), SearchError> {
        if self.version != 1
            || !(64..=16_384).contains(&self.target_chars)
            || self.overlap_chars > 512
            || self.overlap_chars > self.target_chars / 2
        {
            return Err(SearchError::invalid("document_chunker"));
        }
        Ok(())
    }

    /// Versioned fingerprint included in every artifact-chunk reference.
    ///
    /// # Errors
    /// Rejects invalid configurations or encoding failures.
    pub fn digest(&self) -> Result<Digest, SearchError> {
        self.validate()?;
        configuration_digest("document-chunker", self)
    }
}

/// Reconstructable chunk text and citation offsets in parsed Markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentChunk {
    /// Zero-based ordinal under the exact chunker configuration.
    pub ordinal: u32,
    /// Inclusive Unicode character offset in the full parsed Markdown.
    pub start_char: u32,
    /// Exclusive Unicode character offset in the full parsed Markdown.
    pub end_char: u32,
    /// Most recent Markdown heading, when present (bounded to 256 characters).
    pub heading: Option<Arc<str>>,
    /// Exact substring at these offsets, including any retained overlap.
    pub text: Arc<str>,
    /// Versioned fingerprint of exact chunk text.
    pub content_digest: Digest,
}

/// Split bounded Markdown at headings and paragraph boundaries where possible.
/// Long paragraphs use Unicode-safe cuts. Overlap stays inside a heading section.
///
/// # Errors
/// Rejects invalid configurations, text over 4 MiB, and excessive chunk counts.
pub fn chunk_document(
    text: &str,
    config: ChunkerConfig,
    max_chunks: usize,
) -> Result<Vec<DocumentChunk>, SearchError> {
    config.validate()?;
    if text.len() > 4 * 1024 * 1024 || max_chunks == 0 || max_chunks > 100_000 {
        return Err(SearchError::invalid("document_chunk_input"));
    }
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let chars: Vec<char> = text.chars().collect();
    let mut headings = Vec::new();
    let mut paragraphs = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('#')
            && trimmed.chars().take_while(|c| *c == '#').count() <= 6
            && trimmed.trim_start_matches('#').starts_with(' ')
        {
            headings.push((offset, trimmed.chars().take(256).collect::<String>()));
        }
        offset += line.chars().count();
        if trimmed.is_empty() {
            paragraphs.push(offset);
        }
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if chunks.len() == max_chunks {
            return Err(SearchError::SearchCapacityExceeded {
                resource: "document_chunks".into(),
            });
        }
        let hard_end = (start + config.target_chars).min(chars.len());
        let heading_cut = headings
            .iter()
            .map(|(position, _)| *position)
            .find(|position| *position > start && *position <= hard_end);
        let end = heading_cut.unwrap_or_else(|| {
            if hard_end == chars.len() {
                hard_end
            } else {
                paragraphs
                    .iter()
                    .copied()
                    .rfind(|position| {
                        *position > start + config.target_chars / 2 && *position <= hard_end
                    })
                    .unwrap_or(hard_end)
            }
        });
        let slice = chars
            .get(start..end)
            .ok_or_else(|| SearchError::invalid("document_chunk_offsets"))?;
        let content: String = slice.iter().collect();
        let digest = configuration_digest("document-chunk-text", &content)?;
        chunks.push(DocumentChunk {
            ordinal: u32::try_from(chunks.len())
                .map_err(|_| SearchError::invalid("document_chunk_ordinal"))?,
            start_char: u32::try_from(start)
                .map_err(|_| SearchError::invalid("document_chunk_offsets"))?,
            end_char: u32::try_from(end)
                .map_err(|_| SearchError::invalid("document_chunk_offsets"))?,
            heading: headings
                .iter()
                .rev()
                .find(|(position, _)| *position <= start)
                .map(|(_, heading)| Arc::from(heading.as_str())),
            text: content.into(),
            content_digest: digest,
        });
        if end == chars.len() {
            break;
        }
        start = if heading_cut.is_some() {
            end
        } else {
            end.saturating_sub(config.overlap_chars).max(start + 1)
        };
    }
    Ok(chunks)
}
