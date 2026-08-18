//! Message roles and immutable message values (TDD §7.3–§7.4).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{
    BoundedString, ContentBlock, ContentError, ContentItems, LABEL_MAX_BYTES, TEXT_MAX_BYTES,
    validate_content_items,
};
use crate::primitives::Metadata;
use crate::primitives::Timestamp;
use crate::primitives::{MessageId, ToolCallId};

/// Largest model context length that round-trips exactly through portable JSON.
pub const MODEL_CONTEXT_LENGTH_MAX: u64 = 9_007_199_254_740_991;

/// Message role in the canonical conversation model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// System instructions.
    System,
    /// Developer / instruction role.
    Developer,
    /// End-user input.
    User,
    /// Model assistant output.
    Assistant,
    /// Tool-result role.
    Tool,
}

impl MessageRole {
    /// Stable role name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Developer => "developer",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// Selected thinking / reasoning effort for a model invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    /// Lowest thinking effort.
    Low,
    /// Balanced thinking effort.
    Medium,
    /// Highest thinking effort.
    High,
}

/// Provider/model identity and optional selected configuration.
///
/// `thinking_level`, `context_length`, and `fast` are independently optional: a
/// reference may omit all three, or carry any combination (thinking only, fast
/// only, or both, with or without context length).
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::{ModelRef, ThinkingLevel};
///
/// let base = ModelRef::try_new("openai", "gpt-example").expect("model");
/// assert_eq!(base.thinking_level(), None);
///
/// let configured = ModelRef::try_new_with_options(
///     "openai",
///     "gpt-example",
///     Some(ThinkingLevel::Medium),
///     Some(128_000),
///     Some(true),
/// )
/// .expect("configured model");
/// assert_eq!(configured.context_length(), Some(128_000));
/// assert_eq!(configured.fast(), Some(true));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ModelRef {
    provider: Arc<str>,
    model: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<ThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast: Option<bool>,
}

impl ModelRef {
    /// Construct a model reference without optional selected configuration.
    ///
    /// # Arguments
    ///
    /// * `provider` - Provider label (non-empty, ≤ [`LABEL_MAX_BYTES`], no NUL).
    /// * `model` - Model label under the same rules as `provider`.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError::InvalidLabel`] when labels are empty, oversized, or
    /// contain NUL.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::ModelRef;
    ///
    /// let model = ModelRef::try_new("openai", "gpt-example").expect("model");
    /// assert_eq!(model.provider(), "openai");
    /// assert_eq!(model.thinking_level(), None);
    /// ```
    pub fn try_new(
        provider: impl AsRef<str>,
        model: impl AsRef<str>,
    ) -> Result<Self, MessageError> {
        Self::try_new_with_options(provider, model, None, None, None)
    }

    /// Construct a model reference with optional selected configuration.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError::InvalidLabel`] when labels are empty, oversized, or
    /// contain NUL. Returns [`MessageError::InvalidContextLength`] when a context
    /// length is zero or exceeds [`MODEL_CONTEXT_LENGTH_MAX`].
    ///
    /// # Arguments
    ///
    /// * `provider` - Provider label (non-empty, ≤ [`LABEL_MAX_BYTES`], no NUL).
    /// * `model` - Model label under the same rules as `provider`.
    /// * `thinking_level` - Optional selected thinking effort; `None` leaves it unset.
    /// * `context_length` - Optional token context window in `1..=MODEL_CONTEXT_LENGTH_MAX`.
    /// * `fast` - Optional fast-mode selection; `None` leaves it unset.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{ModelRef, ThinkingLevel};
    ///
    /// let model = ModelRef::try_new_with_options(
    ///     "openai",
    ///     "gpt-example",
    ///     Some(ThinkingLevel::Medium),
    ///     Some(128_000),
    ///     Some(true),
    /// )
    /// .expect("model");
    /// assert_eq!(model.context_length(), Some(128_000));
    /// ```
    pub fn try_new_with_options(
        provider: impl AsRef<str>,
        model: impl AsRef<str>,
        thinking_level: Option<ThinkingLevel>,
        context_length: Option<u64>,
        fast: Option<bool>,
    ) -> Result<Self, MessageError> {
        if let Some(value) = context_length
            && !(1..=MODEL_CONTEXT_LENGTH_MAX).contains(&value)
        {
            return Err(MessageError::InvalidContextLength {
                value,
                max: MODEL_CONTEXT_LENGTH_MAX,
            });
        }
        Ok(Self {
            provider: validated_label(provider.as_ref(), "provider")?,
            model: validated_label(model.as_ref(), "model")?,
            thinking_level,
            context_length,
            fast,
        })
    }

    /// Provider name.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Optional thinking level.
    #[must_use]
    pub const fn thinking_level(&self) -> Option<ThinkingLevel> {
        self.thinking_level
    }

    /// Optional context length in tokens.
    #[must_use]
    pub const fn context_length(&self) -> Option<u64> {
        self.context_length
    }

    /// Optional fast-mode selection.
    #[must_use]
    pub const fn fast(&self) -> Option<bool> {
        self.fast
    }
}

impl<'de> Deserialize<'de> for ModelRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            provider: BoundedString<LABEL_MAX_BYTES>,
            model: BoundedString<LABEL_MAX_BYTES>,
            #[serde(default)]
            thinking_level: Option<ThinkingLevel>,
            #[serde(default)]
            context_length: Option<u64>,
            #[serde(default)]
            fast: Option<bool>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new_with_options(
            wire.provider.into_inner(),
            wire.model.into_inner(),
            wire.thinking_level,
            wire.context_length,
            wire.fast,
        )
        .map_err(de::Error::custom)
    }
}

/// Opaque provider correlation identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Default)]
#[allow(clippy::struct_field_names)] // TDD §5.2 names are `*_id`.
pub struct ProviderIds {
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_id: Option<Arc<str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continuation_id: Option<Arc<str>>,
}

impl ProviderIds {
    /// Empty provider identifiers.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            request_id: None,
            response_id: None,
            continuation_id: None,
        }
    }

    /// Construct provider identifiers with v1 string ceilings.
    ///
    /// # Arguments
    ///
    /// * `request_id` - Optional provider request id; `None` omits the field.
    /// * `response_id` - Optional provider response id; `None` omits the field.
    /// * `continuation_id` - Optional provider continuation id; `None` omits the field.
    ///
    /// Each present string must be non-empty and within the v1 text ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError`] when any present string is empty or oversized.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::ProviderIds;
    ///
    /// let ids = ProviderIds::try_new(Some("req-1"), None::<&str>, None::<&str>).expect("ids");
    /// assert_eq!(ids.request_id(), Some("req-1"));
    /// assert_eq!(ids.response_id(), None);
    /// ```
    pub fn try_new(
        request_id: Option<impl AsRef<str>>,
        response_id: Option<impl AsRef<str>>,
        continuation_id: Option<impl AsRef<str>>,
    ) -> Result<Self, MessageError> {
        Ok(Self {
            request_id: optional_provider_string(request_id, "request_id")?,
            response_id: optional_provider_string(response_id, "response_id")?,
            continuation_id: optional_provider_string(continuation_id, "continuation_id")?,
        })
    }

    /// Provider request identifier.
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }

    /// Provider response identifier.
    #[must_use]
    pub fn response_id(&self) -> Option<&str> {
        self.response_id.as_deref()
    }

    /// Provider continuation identifier.
    #[must_use]
    pub fn continuation_id(&self) -> Option<&str> {
        self.continuation_id.as_deref()
    }
}

impl<'de> Deserialize<'de> for ProviderIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[allow(clippy::struct_field_names)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            request_id: Option<BoundedString<TEXT_MAX_BYTES>>,
            #[serde(default)]
            response_id: Option<BoundedString<TEXT_MAX_BYTES>>,
            #[serde(default)]
            continuation_id: Option<BoundedString<TEXT_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.request_id.map(BoundedString::into_inner),
            wire.response_id.map(BoundedString::into_inner),
            wire.continuation_id.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}

/// Immutable provider-neutral message value.
///
/// # Examples
///
/// ```
/// use finstack_ai_kernel::{
///     ContentBlock, Message, MessageId, MessageRole, Metadata, ProviderIds, TextBlock,
///     Timestamp,
/// };
///
/// let message = Message::try_new(
///     MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
///     MessageRole::User,
///     vec![ContentBlock::Text(TextBlock::try_new("hello").expect("text"))],
///     Timestamp::from_unix_ms(0).expect("ts"),
///     None,
///     ProviderIds::empty(),
///     Metadata::empty(),
/// )
/// .expect("message");
/// assert_eq!(message.role(), MessageRole::User);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Message {
    id: MessageId,
    role: MessageRole,
    content: Arc<[ContentBlock]>,
    created_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<ModelRef>,
    provider_ids: ProviderIds,
    metadata: Metadata,
}

impl Message {
    /// Construct and validate a message.
    ///
    /// # Arguments
    ///
    /// * `id` - Stable message identity.
    /// * `role` - Canonical conversation role; it must match the block kinds in
    ///   `content`.
    /// * `content` - Ordered content blocks. Empty is allowed except for
    ///   [`MessageRole::Tool`], which requires at least one tool-result block.
    /// * `created_at` - Environment-supplied creation timestamp.
    /// * `model` - Optional model that produced an assistant message; `None` for
    ///   other roles or when the producer is unknown.
    /// * `provider_ids` - Opaque provider correlation identifiers.
    /// * `metadata` - Non-authoritative message metadata.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError`] for role/block violations, oversized content, or
    /// invalid tool associations (when role is [`MessageRole::Tool`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     ContentBlock, Message, MessageId, MessageRole, Metadata, ProviderIds, TextBlock,
    ///     Timestamp,
    /// };
    ///
    /// let message = Message::try_new(
    ///     MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
    ///     MessageRole::User,
    ///     vec![ContentBlock::Text(TextBlock::try_new("hello").expect("text"))],
    ///     Timestamp::from_unix_ms(0).expect("ts"),
    ///     None,
    ///     ProviderIds::empty(),
    ///     Metadata::empty(),
    /// )
    /// .expect("message");
    /// assert_eq!(message.role(), MessageRole::User);
    /// ```
    pub fn try_new(
        id: MessageId,
        role: MessageRole,
        content: Vec<ContentBlock>,
        created_at: Timestamp,
        model: Option<ModelRef>,
        provider_ids: ProviderIds,
        metadata: Metadata,
    ) -> Result<Self, MessageError> {
        validate_content_items(&content)?;
        validate_role_blocks(role, &content)?;
        let message = Self {
            id,
            role,
            content: Arc::from(content),
            created_at,
            model,
            provider_ids,
            metadata,
        };
        message.validate_tool_associations(None)?;
        Ok(message)
    }

    /// Message identity.
    #[must_use]
    pub const fn id(&self) -> &MessageId {
        &self.id
    }

    /// Message role.
    #[must_use]
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// Content blocks.
    #[must_use]
    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    /// Environment-supplied creation timestamp.
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// Optional model reference.
    #[must_use]
    pub const fn model(&self) -> Option<&ModelRef> {
        self.model.as_ref()
    }

    /// Provider correlation identifiers.
    #[must_use]
    pub const fn provider_ids(&self) -> &ProviderIds {
        &self.provider_ids
    }

    /// Non-authoritative metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Validate pure structural tool-result associations.
    ///
    /// When `known_calls` is `Some`, every tool-result `tool_call_id` must appear in
    /// that set. Run-state pairing remains outside this DTO layer.
    ///
    /// # Errors
    ///
    /// Returns [`MessageError`] for missing results on tool messages, duplicates,
    /// or unknown IDs relative to `known_calls`.
    pub fn validate_tool_associations(
        &self,
        known_calls: Option<&[ToolCallId]>,
    ) -> Result<(), MessageError> {
        if self.role != MessageRole::Tool {
            return Ok(());
        }
        let mut seen = BTreeSet::new();
        let mut count = 0_usize;
        for block in self.content.iter() {
            let ContentBlock::ToolResult(result) = block else {
                continue;
            };
            count += 1;
            let id = result.tool_call_id().to_canonical_string();
            if !seen.insert(id) {
                return Err(MessageError::DuplicateToolAssociation {
                    tool_call_id: result.tool_call_id().to_canonical_string(),
                });
            }
            if let Some(known) = known_calls
                && !known.iter().any(|item| item == result.tool_call_id())
            {
                return Err(MessageError::UnknownToolAssociation {
                    tool_call_id: result.tool_call_id().to_canonical_string(),
                });
            }
        }
        if count == 0 {
            return Err(MessageError::MissingToolResult);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: MessageId,
            role: MessageRole,
            content: ContentItems,
            created_at: Timestamp,
            #[serde(default)]
            model: Option<ModelRef>,
            #[serde(default)]
            provider_ids: ProviderIds,
            #[serde(default)]
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.id,
            wire.role,
            wire.content.into_inner(),
            wire.created_at,
            wire.model,
            wire.provider_ids,
            wire.metadata,
        )
        .map_err(de::Error::custom)
    }
}

/// Message validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MessageError {
    /// Content construction failure.
    #[error(transparent)]
    Content(#[from] ContentError),
    /// Role/block combination is invalid.
    #[error("role {role} does not allow content kind {kind}")]
    RoleBlockMismatch {
        /// Role name.
        role: &'static str,
        /// Block kind name.
        kind: &'static str,
    },
    /// Tool message lacked any tool-result block.
    #[error("tool message requires at least one tool_result block")]
    MissingToolResult,
    /// Duplicate tool-result association within one message.
    #[error("duplicate tool_call_id association: {tool_call_id}")]
    DuplicateToolAssociation {
        /// Conflicting tool-call id.
        tool_call_id: String,
    },
    /// Tool-result referenced a tool call outside the supplied known set.
    #[error("unknown tool_call_id association: {tool_call_id}")]
    UnknownToolAssociation {
        /// Rejected tool-call id.
        tool_call_id: String,
    },
    /// Label was empty, oversized, or contained NUL.
    #[error("invalid {field}: empty, oversized, or contains NUL")]
    InvalidLabel {
        /// Field name.
        field: &'static str,
    },
    /// Provider string exceeded the text ceiling.
    #[error("{field} length {len} exceeds max {max}")]
    ProviderIdTooLarge {
        /// Field name.
        field: &'static str,
        /// Observed length.
        len: usize,
        /// Ceiling.
        max: usize,
    },
    /// `context_length` was outside the portable positive token-budget range.
    #[error("context_length {value} must be between 1 and {max}")]
    InvalidContextLength {
        /// Rejected context length.
        value: u64,
        /// Maximum portable JSON integer.
        max: u64,
    },
}

impl MessageError {
    /// Stable error code for fixtures and descriptors.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Content(ContentError::InvalidLabel { .. }) | Self::InvalidLabel { .. } => {
                "invalid_label"
            }
            Self::Content(ContentError::TextTooLarge { .. }) => "text_too_large",
            Self::Content(ContentError::BytesTooLarge { .. }) => "bytes_too_large",
            Self::Content(ContentError::TooManyItems { .. }) => "too_many_items",
            Self::Content(ContentError::NestedToolBlock) => "nested_tool_block",
            Self::Content(ContentError::InvalidHex) => "invalid_hex",
            Self::RoleBlockMismatch { .. } => "role_block_mismatch",
            Self::MissingToolResult => "missing_tool_result",
            Self::DuplicateToolAssociation { .. } => "duplicate_tool_association",
            Self::UnknownToolAssociation { .. } => "unknown_tool_association",
            Self::ProviderIdTooLarge { .. } => "provider_id_too_large",
            Self::InvalidContextLength { .. } => "invalid_context_length",
        }
    }
}

fn validate_role_blocks(role: MessageRole, content: &[ContentBlock]) -> Result<(), MessageError> {
    for block in content {
        let allowed = matches!(
            (role, block),
            (
                MessageRole::System | MessageRole::Developer,
                ContentBlock::Text(_) | ContentBlock::Json(_) | ContentBlock::Opaque(_),
            ) | (
                MessageRole::User,
                ContentBlock::Text(_)
                    | ContentBlock::Json(_)
                    | ContentBlock::Image(_)
                    | ContentBlock::Audio(_)
                    | ContentBlock::File(_)
                    | ContentBlock::Opaque(_),
            ) | (
                MessageRole::Assistant,
                ContentBlock::Text(_)
                    | ContentBlock::Json(_)
                    | ContentBlock::Image(_)
                    | ContentBlock::Audio(_)
                    | ContentBlock::File(_)
                    | ContentBlock::ToolCall(_)
                    | ContentBlock::Opaque(_),
            ) | (MessageRole::Tool, ContentBlock::ToolResult(_))
        );
        if !allowed {
            return Err(MessageError::RoleBlockMismatch {
                role: role.as_str(),
                kind: block.kind_name(),
            });
        }
    }
    Ok(())
}

fn validated_label(value: &str, field: &'static str) -> Result<Arc<str>, MessageError> {
    crate::primitives::label::validated_label(value, field, |field| MessageError::InvalidLabel {
        field,
    })
}

fn optional_provider_string(
    value: Option<impl AsRef<str>>,
    field: &'static str,
) -> Result<Option<Arc<str>>, MessageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.as_ref();
    if value.is_empty() {
        return Err(MessageError::InvalidLabel { field });
    }
    if value.len() > TEXT_MAX_BYTES {
        return Err(MessageError::ProviderIdTooLarge {
            field,
            len: value.len(),
            max: TEXT_MAX_BYTES,
        });
    }
    Ok(Some(Arc::<str>::from(value)))
}
