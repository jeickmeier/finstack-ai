//! Immutable conversation tree and message values (TDD §7.3–§7.4, §24.1).
//!
//! [`Message`] is the provider-neutral content value. [`ConversationEntry`] and
//! [`SessionProjection`] model the session-level conversation tree; they are
//! not part of [`crate::KernelState`]. [`SessionProjection::walk`] and
//! [`SessionProjection::history`] project lanes and histories.

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
    SessionProjection,
};
