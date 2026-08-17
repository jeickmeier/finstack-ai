//! Immutable conversation tree and message values (TDD §7.3–§7.4, §24.1).

mod message;
mod tree;

#[cfg(test)]
mod tests;

pub use message::{
    MODEL_CONTEXT_LENGTH_MAX, Message, MessageError, MessageRole, ModelRef, ProviderIds,
    ThinkingLevel,
};
pub use tree::{
    ConversationEntry, ConversationError, EntryBody, LaneProjection, OperationSummary,
    SessionProjection, apply_conversation_entry, extract_history, walk_conversation,
};
