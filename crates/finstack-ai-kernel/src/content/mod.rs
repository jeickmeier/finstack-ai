//! Content blocks and blob references (TDD §7.1–§7.2).

mod blob;
mod content_block;
mod error;
mod opaque;
mod text;
mod tool;

#[cfg(test)]
mod tests;

pub(crate) use crate::primitives::BoundedString;

pub(crate) type ContentItems = crate::primitives::BoundedVec<ContentBlock, CONTENT_MAX_ITEMS>;

pub use blob::{BlobRef, MediaRef};
pub use content_block::ContentBlock;
pub use error::ContentError;
pub(crate) use error::validate_content_items;
pub use opaque::{OpaqueBlock, OpaquePayload};
pub use text::{
    CONTENT_MAX_ITEMS, JsonBlock, LABEL_MAX_BYTES, TEXT_MAX_BYTES, TextBlock, hex_nibble,
    label_is_valid,
};
pub use tool::{ToolCallBlock, ToolResultBlock};
