//! Content construction and validation errors.

use thiserror::Error;

use super::content_block::ContentBlock;
use super::text::CONTENT_MAX_ITEMS;

/// Content construction / validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContentError {
    /// Label was empty, oversized, or contained NUL.
    #[error("invalid {field}: empty, oversized, or contains NUL")]
    InvalidLabel {
        /// Field name.
        field: &'static str,
    },
    /// Text exceeded the v1 ceiling.
    #[error("text length {len} exceeds max {max}")]
    TextTooLarge {
        /// Observed UTF-8 byte length.
        len: usize,
        /// Ceiling.
        max: usize,
    },
    /// Opaque bytes exceeded the v1 ceiling.
    #[error("byte length {len} exceeds max {max}")]
    BytesTooLarge {
        /// Observed byte length.
        len: usize,
        /// Ceiling.
        max: usize,
    },
    /// Content array exceeded the v1 item ceiling.
    #[error("content item count {len} exceeds max {max}")]
    TooManyItems {
        /// Observed item count.
        len: usize,
        /// Ceiling.
        max: usize,
    },
    /// Nested tool-call/tool-result inside a tool-result payload.
    #[error("tool_result content cannot nest tool_call or tool_result blocks")]
    NestedToolBlock,
    /// Hex payload was malformed.
    #[error("invalid hex payload")]
    InvalidHex,
}

pub(crate) fn validate_content_items(content: &[ContentBlock]) -> Result<(), ContentError> {
    if content.len() > CONTENT_MAX_ITEMS {
        return Err(ContentError::TooManyItems {
            len: content.len(),
            max: CONTENT_MAX_ITEMS,
        });
    }
    Ok(())
}
