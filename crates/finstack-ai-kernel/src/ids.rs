//! Typed UUID identifiers and namespaced string keys.
//!
//! Allocated runtime entity IDs use [`Id`] over lowercase UUID strings (ADR-029).
//! Human-selected configuration/package names use [`Key`].

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Opaque tag distinguishing typed UUID identifiers at compile time.
pub trait IdTag: Send + Sync + 'static {
    /// Stable family name used in diagnostics.
    const NAME: &'static str;
}

macro_rules! define_id_tag {
    ($tag:ident, $name:literal) => {
        #[doc = concat!("Tag for [`", stringify!($tag), "`]-family identifiers.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $tag {}
        impl IdTag for $tag {
            const NAME: &'static str = $name;
        }
    };
}

define_id_tag!(SessionTag, "session");
define_id_tag!(LaneTag, "lane");
define_id_tag!(RunTag, "run");
define_id_tag!(TurnTag, "turn");
define_id_tag!(MessageTag, "message");
define_id_tag!(EntryTag, "entry");
define_id_tag!(ModelRequestTag, "model-request");
define_id_tag!(ToolBatchTag, "tool-batch");
define_id_tag!(EffectTag, "effect");
define_id_tag!(ToolCallTag, "tool-call");
define_id_tag!(InteractionTag, "interaction");
define_id_tag!(BudgetScopeTag, "budget-scope");
define_id_tag!(BudgetReservationTag, "budget-reservation");
define_id_tag!(CancellationRequestTag, "cancellation-request");
define_id_tag!(EventTag, "event");
define_id_tag!(RecordTag, "record");
define_id_tag!(AppendBatchTag, "append-batch");
define_id_tag!(ArtifactTag, "artifact");

/// Typed UUID newtype used for allocated runtime entity identifiers.
///
/// Distinct tag types are not interchangeable, so accidental type confusion fails
/// at compile time.
///
/// ```compile_fail
/// use finstack_ai_kernel::{RunId, SessionId};
/// fn takes_run(_: RunId) {}
/// fn demo(session: SessionId) {
///     takes_run(session);
/// }
/// ```
#[repr(transparent)]
pub struct Id<T: IdTag> {
    value: [u8; 16],
    _marker: PhantomData<fn() -> T>,
}

impl<T: IdTag> Id<T> {
    /// Construct an identifier from raw UUID bytes.
    #[must_use]
    pub const fn from_bytes(value: [u8; 16]) -> Self {
        Self {
            value,
            _marker: PhantomData,
        }
    }

    /// Borrow the underlying UUID bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.value
    }

    /// Copy the underlying UUID bytes.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; 16] {
        self.value
    }

    /// Serialize as the canonical lowercase hyphenated UUID string.
    #[must_use]
    pub fn to_canonical_string(&self) -> String {
        format_uuid_hyphenated(&self.value)
    }

    /// Parse a lowercase or uppercase hyphenated UUID string.
    ///
    /// # Errors
    ///
    /// Returns [`IdParseError`] when the string is not a valid UUID.
    pub fn parse(input: &str) -> Result<Self, IdParseError> {
        let Some(value) = parse_uuid_hyphenated(input) else {
            return Err(IdParseError {
                family: T::NAME,
                input: input.to_owned(),
            });
        };
        Ok(Self::from_bytes(value))
    }
}

impl<T: IdTag> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: IdTag> Copy for Id<T> {}

impl<T: IdTag> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: IdTag> Eq for Id<T> {}

impl<T: IdTag> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: IdTag> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T: IdTag> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T: IdTag> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(T::NAME)
            .field(&self.to_canonical_string())
            .finish()
    }
}

impl<T: IdTag> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_canonical_string())
    }
}

impl<T: IdTag> FromStr for Id<T> {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl<T: IdTag> Serialize for Id<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_canonical_string())
    }
}

impl<'de, T: IdTag> Deserialize<'de> for Id<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Failure parsing a typed UUID identifier.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {family} id: {input}")]
pub struct IdParseError {
    /// Identifier family name.
    pub family: &'static str,
    /// Rejected input.
    pub input: String,
}

fn format_uuid_hyphenated(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn parse_uuid_hyphenated(input: &str) -> Option<[u8; 16]> {
    let bytes = input.as_bytes();
    if bytes.len() != 36
        || bytes[8] != b'-'
        || bytes[13] != b'-'
        || bytes[18] != b'-'
        || bytes[23] != b'-'
    {
        return None;
    }
    let mut out = [0_u8; 16];
    let groups = [(0, 4), (9, 2), (14, 2), (19, 2), (24, 6)];
    let mut idx = 0;
    for (start, count) in groups {
        for offset in 0..count {
            let hi = hex_nibble(bytes[start + offset * 2])?;
            let lo = hex_nibble(bytes[start + offset * 2 + 1])?;
            out[idx] = (hi << 4) | lo;
            idx += 1;
        }
    }
    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Opaque tag distinguishing typed string keys at compile time.
pub trait KeyTag: Send + Sync + 'static {
    /// Stable family name used in diagnostics.
    const NAME: &'static str;
}

macro_rules! define_key_tag {
    ($tag:ident, $name:literal) => {
        #[doc = concat!("Tag for [`", stringify!($tag), "`]-family keys.")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $tag {}
        impl KeyTag for $tag {
            const NAME: &'static str = $name;
        }
    };
}

define_key_tag!(AgentTag, "agent");
define_key_tag!(BundleTag, "bundle");
define_key_tag!(ComponentTag, "component");
define_key_tag!(CapabilityTag, "capability");
define_key_tag!(ToolTag, "tool");

/// Maximum UTF-8 byte length for a namespaced key (TDD §4).
pub const KEY_MAX_BYTES: usize = 128;

/// Validated namespaced string key for human-selected identities.
#[repr(transparent)]
pub struct Key<T: KeyTag> {
    value: Arc<str>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: KeyTag> Key<T> {
    /// Parse and validate a namespaced key.
    ///
    /// Accepted forms:
    /// - local aliases matching `[a-z][a-z0-9._-]{0,127}`
    /// - namespaced ids containing at least one `.` with the same character set
    ///
    /// # Errors
    ///
    /// Returns [`KeyParseError`] when the value is empty, oversized, or contains
    /// disallowed characters.
    pub fn parse(input: impl AsRef<str>) -> Result<Self, KeyParseError> {
        let input = input.as_ref();
        validate_key(input).map_err(|kind| KeyParseError {
            family: T::NAME,
            input: input.to_owned(),
            kind,
        })?;
        Ok(Self {
            value: Arc::<str>::from(input),
            _marker: PhantomData,
        })
    }

    /// Borrow the validated key text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl<T: KeyTag> Clone for Key<T> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            _marker: PhantomData,
        }
    }
}

impl<T: KeyTag> PartialEq for Key<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: KeyTag> Eq for Key<T> {}

impl<T: KeyTag> PartialOrd for Key<T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: KeyTag> Ord for Key<T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T: KeyTag> Hash for Key<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T: KeyTag> fmt::Debug for Key<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple(T::NAME).field(&self.value).finish()
    }
}

impl<T: KeyTag> fmt::Display for Key<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<T: KeyTag> AsRef<str> for Key<T> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<T: KeyTag> Serialize for Key<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de, T: KeyTag> Deserialize<'de> for Key<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(text).map_err(serde::de::Error::custom)
    }
}

/// Failure validating a namespaced key.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {family} key ({kind:?}): {input}")]
pub struct KeyParseError {
    /// Key family name.
    pub family: &'static str,
    /// Rejected input.
    pub input: String,
    /// Validation failure class.
    pub kind: KeyParseErrorKind,
}

/// Why a key failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyParseErrorKind {
    /// Empty input.
    Empty,
    /// Exceeded [`KEY_MAX_BYTES`].
    TooLong,
    /// First character was not lowercase ASCII alphabetic.
    BadStart,
    /// Contained a character outside `[a-z0-9._-]`.
    BadChar,
}

fn validate_key(input: &str) -> Result<(), KeyParseErrorKind> {
    if input.is_empty() {
        return Err(KeyParseErrorKind::Empty);
    }
    if input.len() > KEY_MAX_BYTES {
        return Err(KeyParseErrorKind::TooLong);
    }
    let mut chars = input.chars();
    let Some(first) = chars.next() else {
        return Err(KeyParseErrorKind::Empty);
    };
    if !first.is_ascii_lowercase() {
        return Err(KeyParseErrorKind::BadStart);
    }
    for ch in chars {
        let ok = ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-');
        if !ok {
            return Err(KeyParseErrorKind::BadChar);
        }
    }
    Ok(())
}

/// Session identifier.
pub type SessionId = Id<SessionTag>;
/// Lane identifier.
pub type LaneId = Id<LaneTag>;
/// Run identifier.
pub type RunId = Id<RunTag>;
/// Turn identifier.
pub type TurnId = Id<TurnTag>;
/// Message identifier.
pub type MessageId = Id<MessageTag>;
/// Conversation entry identifier.
pub type EntryId = Id<EntryTag>;
/// Model request identifier.
pub type ModelRequestId = Id<ModelRequestTag>;
/// Tool batch identifier.
pub type ToolBatchId = Id<ToolBatchTag>;
/// Effect identifier.
pub type EffectId = Id<EffectTag>;
/// Tool-call invocation identifier.
pub type ToolCallId = Id<ToolCallTag>;
/// Interaction identifier.
pub type InteractionId = Id<InteractionTag>;
/// Budget scope identifier.
pub type BudgetScopeId = Id<BudgetScopeTag>;
/// Budget reservation identifier.
pub type BudgetReservationId = Id<BudgetReservationTag>;
/// Cancellation request identifier.
pub type CancellationRequestId = Id<CancellationRequestTag>;
/// Event identifier.
pub type EventId = Id<EventTag>;
/// Journal record identifier.
pub type RecordId = Id<RecordTag>;
/// Append batch identifier.
pub type AppendBatchId = Id<AppendBatchTag>;
/// Artifact identifier.
pub type ArtifactId = Id<ArtifactTag>;

/// Agent configuration key.
pub type AgentId = Key<AgentTag>;
/// Bundle configuration key.
pub type BundleId = Key<BundleTag>;
/// Component configuration key.
pub type ComponentId = Key<ComponentTag>;
/// Capability configuration key.
pub type CapabilityId = Key<CapabilityTag>;
/// Registered tool name key.
pub type ToolId = Key<ToolTag>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialize_lowercase_and_reject_invalid() {
        let id = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("parse");
        assert_eq!(
            id.to_canonical_string(),
            "01234567-89ab-7cde-89ab-0123456789ab"
        );
        let json = serde_json::to_string(&id).expect("ser");
        assert_eq!(json, "\"01234567-89ab-7cde-89ab-0123456789ab\"");
        let round: RunId = serde_json::from_str(&json).expect("de");
        assert_eq!(round, id);
        assert!(SessionId::parse("not-a-uuid").is_err());
    }

    #[test]
    fn keys_validate_namespace_rules() {
        let local = ToolId::parse("filesystem").expect("local");
        assert_eq!(local.as_str(), "filesystem");
        let namespaced = ComponentId::parse("finstack.model.openai-compatible").expect("ns");
        assert!(namespaced.as_str().contains('.'));
        assert!(matches!(
            ToolId::parse("").expect_err("empty").kind,
            KeyParseErrorKind::Empty
        ));
        assert!(matches!(
            ToolId::parse("Filesystem").expect_err("case").kind,
            KeyParseErrorKind::BadStart
        ));
        assert!(matches!(
            ToolId::parse("a".repeat(KEY_MAX_BYTES + 1))
                .expect_err("long")
                .kind,
            KeyParseErrorKind::TooLong
        ));
    }
}
