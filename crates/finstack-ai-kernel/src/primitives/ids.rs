//! Typed UUID identifiers and namespaced string keys.
//!
//! Allocated runtime entity IDs use [`Id`] over lowercase UUID strings (ADR-029).
//! Human-selected configuration/package names use [`Key`].

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

/// Opaque tag distinguishing typed UUID identifiers at compile time.
pub trait IdTag: Send + Sync + 'static {
    /// Stable family name used in diagnostics.
    const NAME: &'static str;
}

macro_rules! define_id_tag {
    ($tag:ident, $name:literal) => {
        // Uninhabited: no value ever exists, so trait impls on the tag itself
        // are unreachable. `Id<T>` hand-implements every trait under
        // `T: IdTag`, and `PhantomData<fn() -> T>` imposes no auto-trait bound,
        // so deriving here only enlarged the public surface.
        #[doc = concat!("Tag for [`", stringify!($tag), "`]-family identifiers.")]
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
        let mut buffer = UuidBuffer::new();
        buffer.encode(&self.value).to_owned()
    }

    /// Parse a lowercase or uppercase hyphenated UUID string.
    ///
    /// # Arguments
    ///
    /// * `input` - Hyphenated 8-4-4-4-12 UUID text. Case is accepted; the stored
    ///   value is the 16 raw UUID bytes.
    ///
    /// # Errors
    ///
    /// Returns [`IdParseError`] when the string is not a valid UUID.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::RunId;
    ///
    /// let id = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// assert_eq!(id.to_canonical_string(), "01234567-89ab-7cde-89ab-0123456789ab");
    /// ```
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
        let mut buffer = UuidBuffer::new();
        f.debug_tuple(T::NAME)
            .field(&buffer.encode(&self.value))
            .finish()
    }
}

impl<T: IdTag> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buffer = UuidBuffer::new();
        f.write_str(buffer.encode(&self.value))
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
        let mut buffer = UuidBuffer::new();
        serializer.serialize_str(buffer.encode(&self.value))
    }
}

impl<'de, T: IdTag> Deserialize<'de> for Id<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdVisitor<T: IdTag>(PhantomData<fn() -> T>);

        impl<T: IdTag> de::Visitor<'_> for IdVisitor<T> {
            type Value = Id<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a hyphenated {} UUID string", T::NAME)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Id::parse(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(IdVisitor::<T>(PhantomData))
    }
}

/// Stack buffer for canonical hyphenated UUID text.
///
/// Encoding writes 36 ASCII bytes with a nibble lookup table, avoiding the
/// heap allocation and 16 `core::fmt` dispatches a `format!` would cost. IDs
/// are the most frequently serialized scalar in every canonical projection.
pub(crate) struct UuidBuffer([u8; 36]);

impl UuidBuffer {
    pub(crate) const fn new() -> Self {
        Self([b'-'; 36])
    }

    pub(crate) fn encode(&mut self, bytes: &[u8; 16]) -> &str {
        const GROUPS: [(usize, usize, usize); 5] =
            [(0, 0, 4), (9, 4, 2), (14, 6, 2), (19, 8, 2), (24, 10, 6)];
        for (text_start, byte_start, count) in GROUPS {
            for offset in 0..count {
                let byte = bytes[byte_start + offset];
                let position = text_start + offset * 2;
                self.0[position] = crate::primitives::HEX_DIGITS[usize::from(byte >> 4)];
                self.0[position + 1] = crate::primitives::HEX_DIGITS[usize::from(byte & 0x0f)];
            }
        }
        super::str_from_ascii(&self.0)
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
            let hi = crate::hex_nibble(bytes[start + offset * 2])?;
            let lo = crate::hex_nibble(bytes[start + offset * 2 + 1])?;
            out[idx] = (hi << 4) | lo;
            idx += 1;
        }
    }
    Some(out)
}

/// Opaque tag distinguishing typed string keys at compile time.
pub trait KeyTag: Send + Sync + 'static {
    /// Stable family name used in diagnostics.
    const NAME: &'static str;
    /// Whether values must contain a `.` namespace separator (contract section 4).
    const REQUIRES_NAMESPACE: bool;
}

macro_rules! define_key_tag {
    ($tag:ident, $name:literal, $requires_namespace:expr) => {
        // Uninhabited; see `define_id_tag` for why no derives belong here.
        #[doc = concat!("Tag for [`", stringify!($tag), "`]-family keys.")]
        pub enum $tag {}
        impl KeyTag for $tag {
            const NAME: &'static str = $name;
            const REQUIRES_NAMESPACE: bool = $requires_namespace;
        }
    };
}

// Local aliases are allowed for agent/bundle keys; globally registered
// component/tool/capability identities must be namespaced (contract section 4).
define_key_tag!(AgentTag, "agent", false);
define_key_tag!(BundleTag, "bundle", false);
define_key_tag!(ComponentTag, "component", true);
define_key_tag!(CapabilityTag, "capability", true);
define_key_tag!(ToolTag, "tool", true);
define_key_tag!(LimitTag, "limit", true);
define_key_tag!(EffectOutputTag, "effect-output", true);

/// Maximum UTF-8 byte length for a namespaced key (contract section 4).
pub const KEY_MAX_BYTES: usize = 128;

/// Validated namespaced string key for human-selected identities.
#[repr(transparent)]
pub struct Key<T: KeyTag> {
    value: Arc<str>,
    _marker: PhantomData<fn() -> T>,
}

/// Compile-time validated input for [`Key::from_static`].
///
/// Construct this through [`Key::static_literal`] or the
/// [`static_key!`](crate::static_key) macro.
#[doc(hidden)]
pub struct StaticKey<T: KeyTag> {
    value: &'static str,
    _marker: PhantomData<fn() -> T>,
}

impl<T: KeyTag> Clone for StaticKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: KeyTag> Copy for StaticKey<T> {}

impl<T: KeyTag> Key<T> {
    /// Validate a compile-time key literal without allocating.
    #[doc(hidden)]
    #[must_use]
    pub const fn static_literal(input: &'static str) -> Option<StaticKey<T>> {
        if key_literal_is_valid(input, T::REQUIRES_NAMESPACE) {
            Some(StaticKey {
                value: input,
                _marker: PhantomData,
            })
        } else {
            None
        }
    }

    /// Parse and validate a namespaced key.
    ///
    /// Accepted forms:
    /// - local aliases matching `[a-z][a-z0-9._-]{0,127}` when the family allows them
    /// - namespaced ids containing at least one `.` with the same character set
    ///
    /// Globally registered [`ComponentId`], [`ToolId`], and [`CapabilityId`] values
    /// always require a `.` namespace separator (contract section 4).
    ///
    /// # Errors
    ///
    /// Returns [`KeyParseError`] when the value is empty, oversized, missing a
    /// required namespace, or contains disallowed characters.
    ///
    /// # Arguments
    ///
    /// * `input` - Local alias matching `[a-z][a-z0-9._-]{0,127}`, or a
    ///   namespaced id with at least one `.`. [`ToolId`], [`ComponentId`], and
    ///   [`CapabilityId`] always require the namespace form.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::ToolId;
    ///
    /// let id = ToolId::parse("search.web").expect("tool");
    /// assert_eq!(id.as_str(), "search.web");
    /// assert!(ToolId::parse("web").is_err());
    /// ```
    pub fn parse(input: impl AsRef<str>) -> Result<Self, KeyParseError> {
        let input = input.as_ref();
        validate_key(input, T::REQUIRES_NAMESPACE).map_err(|kind| KeyParseError {
            family: T::NAME,
            input: input.to_owned(),
            kind,
        })?;
        Ok(Self {
            value: Arc::<str>::from(input),
            _marker: PhantomData,
        })
    }

    /// Construct a key from a compile-time literal that the crate already owns.
    ///
    /// # Arguments
    ///
    /// * `input` - A compile-time-validated local alias or namespaced id.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{ToolId, static_key};
    ///
    /// let id = static_key!(ToolId, "search.web");
    /// assert_eq!(id.as_str(), "search.web");
    /// ```
    #[must_use]
    pub fn from_static(input: StaticKey<T>) -> Self {
        Self {
            value: Arc::from(input.value),
            _marker: PhantomData,
        }
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
    /// Family requires a `.` namespace separator.
    MissingNamespace,
}

fn validate_key(input: &str, requires_namespace: bool) -> Result<(), KeyParseErrorKind> {
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
    let mut saw_dot = false;
    for ch in chars {
        let ok = ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-');
        if !ok {
            return Err(KeyParseErrorKind::BadChar);
        }
        if ch == '.' {
            saw_dot = true;
        }
    }
    if requires_namespace && !saw_dot {
        return Err(KeyParseErrorKind::MissingNamespace);
    }
    Ok(())
}

const fn key_literal_is_valid(input: &str, requires_namespace: bool) -> bool {
    let bytes = input.as_bytes();
    if bytes.is_empty() || bytes.len() > KEY_MAX_BYTES || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut index = 1;
    let mut saw_dot = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'.' {
            saw_dot = true;
        } else if !(byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || byte == b'_'
            || byte == b'-')
        {
            return false;
        }
        index += 1;
    }
    !requires_namespace || saw_dot
}

/// Construct a [`Key`] alias from a compile-time-validated literal.
///
/// Invalid literals fail during compilation:
///
/// ```compile_fail
/// use finstack_ai_kernel::{ToolId, static_key};
///
/// let _ = static_key!(ToolId, "missing_namespace");
/// ```
#[macro_export]
macro_rules! static_key {
    ($key:ty, $input:expr) => {{
        let validated = const {
            match <$key>::static_literal($input) {
                Some(validated) => validated,
                None => panic!("invalid static key"),
            }
        };
        <$key>::from_static(validated)
    }};
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
/// Namespaced extension/limit counter key.
pub type LimitKey = Key<LimitTag>;
/// Namespaced custom effect-output kind key.
pub type EffectOutputKey = Key<EffectOutputTag>;

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
        let local = AgentId::parse("research-agent").expect("local alias");
        assert_eq!(local.as_str(), "research-agent");
        let namespaced = ComponentId::parse("finstack.model.openai").expect("ns");
        assert!(namespaced.as_str().contains('.'));
        assert!(matches!(
            ToolId::parse("filesystem").expect_err("global tool").kind,
            KeyParseErrorKind::MissingNamespace
        ));
        assert!(matches!(
            CapabilityId::parse("research")
                .expect_err("global capability")
                .kind,
            KeyParseErrorKind::MissingNamespace
        ));
        assert!(ToolId::parse("finstack.tools.filesystem").is_ok());
        assert!(matches!(
            ToolId::parse("").expect_err("empty").kind,
            KeyParseErrorKind::Empty
        ));
        assert!(matches!(
            ToolId::parse("Filesystem").expect_err("case").kind,
            KeyParseErrorKind::BadStart
        ));
        assert!(matches!(
            AgentId::parse("a".repeat(KEY_MAX_BYTES + 1))
                .expect_err("long")
                .kind,
            KeyParseErrorKind::TooLong
        ));
    }

    #[test]
    fn static_keys_share_the_runtime_validation_contract() {
        for key in [
            "finstack.tools.filesystem",
            "missing_namespace",
            "Invalid.start",
            "bad/character",
            "",
        ] {
            assert_eq!(
                ToolId::static_literal(key).is_some(),
                ToolId::parse(key).is_ok(),
                "static/runtime validation drifted for {key:?}",
            );
        }
        assert_eq!(
            crate::static_key!(ToolId, "finstack.tools.filesystem").as_str(),
            "finstack.tools.filesystem",
        );
    }
}
