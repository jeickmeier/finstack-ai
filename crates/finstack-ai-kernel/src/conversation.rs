//! Immutable conversation tree (TDD §24.1). Session-level, not `KernelState`.

use std::collections::{BTreeMap, BTreeSet};

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::ContentBlock;
use crate::ids::{EffectId, EntryId, LaneId, RunId, SessionId};
use crate::message::{Message, MessageRole};
use crate::raw_json::Metadata;
use crate::records::{RecordBody, RecordEnvelope};
use crate::run::{
    ChildRunPrepared, RunAccepted, RunPropagationPolicy, RunRelation, RunSecurityContext,
};
use crate::session::{LaneCreated, LaneMoved};
use crate::tools::ToolCallSettled;

/// Canonical conversation entry with an immutable parent link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationEntry {
    id: EntryId,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<EntryId>,
    lane_id: LaneId,
    sequence: u64,
    body: EntryBody,
}

impl ConversationEntry {
    /// Construct a conversation entry.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::SelfParent`] when `parent_id` equals `id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     ContentBlock, ConversationEntry, EntryBody, EntryId, LaneId, Message, MessageId,
    ///     MessageRole, Metadata, ProviderIds, TextBlock, Timestamp,
    /// };
    ///
    /// let message = Message::try_new(
    ///     MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
    ///     MessageRole::User,
    ///     vec![ContentBlock::Text(TextBlock::try_new("hello").expect("text"))],
    ///     Timestamp::from_unix_ms(0).expect("epoch"),
    ///     None,
    ///     ProviderIds::empty(),
    ///     Metadata::empty(),
    /// )
    /// .expect("message");
    /// let entry = ConversationEntry::try_new(
    ///     EntryId::from_bytes(message.id().to_bytes()),
    ///     None,
    ///     LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane"),
    ///     1,
    ///     EntryBody::Message(message),
    /// )
    /// .expect("entry");
    /// assert!(entry.parent_id().is_none());
    /// ```
    pub fn try_new(
        id: EntryId,
        parent_id: Option<EntryId>,
        lane_id: LaneId,
        sequence: u64,
        body: EntryBody,
    ) -> Result<Self, ConversationError> {
        if parent_id == Some(id) {
            return Err(ConversationError::SelfParent);
        }
        Ok(Self {
            id,
            parent_id,
            lane_id,
            sequence,
            body,
        })
    }

    /// Entry identity.
    #[must_use]
    pub const fn id(&self) -> EntryId {
        self.id
    }

    /// Immutable parent link.
    #[must_use]
    pub const fn parent_id(&self) -> Option<EntryId> {
        self.parent_id
    }

    /// Lane that owns this entry.
    #[must_use]
    pub const fn lane_id(&self) -> LaneId {
        self.lane_id
    }

    /// Session journal sequence of the committing envelope.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Entry body.
    #[must_use]
    pub const fn body(&self) -> &EntryBody {
        &self.body
    }

    /// Build an entry whose identity shares the message UUID bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError::SelfParent`] when `parent_id` equals the
    /// derived entry identity.
    pub fn from_message(
        message: &Message,
        parent_id: Option<EntryId>,
        lane_id: LaneId,
        sequence: u64,
    ) -> Result<Self, ConversationError> {
        Self::try_new(
            EntryId::from_bytes(message.id().to_bytes()),
            parent_id,
            lane_id,
            sequence,
            EntryBody::Message(message.clone()),
        )
    }
}

impl<'de> Deserialize<'de> for ConversationEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: EntryId,
            #[serde(default)]
            parent_id: Option<EntryId>,
            lane_id: LaneId,
            sequence: u64,
            body: EntryBody,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.id,
            wire.parent_id,
            wire.lane_id,
            wire.sequence,
            wire.body,
        )
        .map_err(de::Error::custom)
    }
}

/// v1 conversation body. Role distinguishes user, assistant, and tool messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum EntryBody {
    /// Canonical conversation message.
    Message(Message),
}

/// Conversation-tree construction or extraction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConversationError {
    /// An entry cannot be its own parent.
    #[error("conversation entry cannot parent itself")]
    SelfParent,
    /// Parent identity is not present in the tree.
    #[error("conversation parent is missing")]
    MissingParent,
    /// A later record reused an entry identity with different content.
    #[error("conversation entry parent or body cannot change after commit")]
    ImmutableConflict,
    /// History walk encountered a cycle.
    #[error("conversation parent chain contains a cycle")]
    Cycle,
    /// History walk started at an unknown leaf.
    #[error("conversation leaf is missing")]
    MissingLeaf,
    /// A tool-call/result pair on the extracted path is incomplete or split.
    #[error("conversation history tool-call/result pair is invalid")]
    InvalidToolPair,
    /// Envelope lane or sequence does not match the entry body.
    #[error("conversation entry envelope identity mismatch")]
    EnvelopeMismatch,
    /// `LaneMoved` pointed at an unknown entry.
    #[error("lane leaf does not exist")]
    UnknownLeaf,
}

impl ConversationError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SelfParent => "conversation_self_parent",
            Self::MissingParent => "conversation_missing_parent",
            Self::ImmutableConflict => "conversation_immutable",
            Self::Cycle => "conversation_cycle",
            Self::MissingLeaf => "conversation_missing_leaf",
            Self::InvalidToolPair => "conversation_invalid_tool_pair",
            Self::EnvelopeMismatch => "conversation_envelope_mismatch",
            Self::UnknownLeaf => "conversation_unknown_leaf",
        }
    }
}

/// Apply one committed conversation entry to an in-memory map.
///
/// # Errors
///
/// Returns [`ConversationError`] for a missing parent, self-parent, or a
/// conflicting reuse of an existing identity.
pub fn apply_conversation_entry(
    entries: &mut BTreeMap<EntryId, ConversationEntry>,
    entry: ConversationEntry,
) -> Result<(), ConversationError> {
    if let Some(parent_id) = entry.parent_id()
        && !entries.contains_key(&parent_id)
    {
        return Err(ConversationError::MissingParent);
    }
    match entries.get(&entry.id()) {
        Some(existing) if same_committed_entry(existing, &entry) => Ok(()),
        Some(_) => Err(ConversationError::ImmutableConflict),
        None => {
            entries.insert(entry.id(), entry);
            Ok(())
        }
    }
}

/// Walk `parent_id` from `leaf_id` to the root without requiring closed tool pairs.
///
/// Inspect and mid-run restore use this path. Model-facing history still goes
/// through [`extract_history`], which rejects an open tool-call/result pair.
///
/// # Errors
///
/// Returns [`ConversationError`] for a missing leaf/parent or a cycle.
pub fn walk_conversation(
    entries: &BTreeMap<EntryId, ConversationEntry>,
    leaf_id: EntryId,
) -> Result<Vec<ConversationEntry>, ConversationError> {
    let mut walk = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = Some(leaf_id);
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(ConversationError::Cycle);
        }
        let entry = entries
            .get(&id)
            .ok_or(if walk.is_empty() {
                ConversationError::MissingLeaf
            } else {
                ConversationError::MissingParent
            })?
            .clone();
        current = entry.parent_id();
        walk.push(entry);
    }
    walk.reverse();
    Ok(walk)
}

/// Walk `parent_id` from `leaf_id` to the root and validate tool-call pairs.
///
/// # Errors
///
/// Returns [`ConversationError`] for a missing leaf/parent, a cycle, or an
/// invalid tool-call/result pair.
pub fn extract_history(
    entries: &BTreeMap<EntryId, ConversationEntry>,
    leaf_id: EntryId,
) -> Result<Vec<ConversationEntry>, ConversationError> {
    let walk = walk_conversation(entries, leaf_id)?;
    validate_tool_pairs(&walk)?;
    Ok(walk)
}

fn same_committed_entry(left: &ConversationEntry, right: &ConversationEntry) -> bool {
    left.id == right.id
        && left.parent_id == right.parent_id
        && left.lane_id == right.lane_id
        && left.body == right.body
}

fn validate_tool_pairs(path: &[ConversationEntry]) -> Result<(), ConversationError> {
    let mut open = BTreeSet::new();
    let mut closed = BTreeSet::new();
    for entry in path {
        let EntryBody::Message(message) = entry.body();
        match message.role() {
            MessageRole::Assistant => {
                for block in message.content() {
                    if let ContentBlock::ToolCall(call) = block
                        && !open.insert(*call.tool_call_id())
                    {
                        return Err(ConversationError::InvalidToolPair);
                    }
                }
            }
            MessageRole::Tool => {
                for block in message.content() {
                    if let ContentBlock::ToolResult(result) = block {
                        let id = *result.tool_call_id();
                        if !open.remove(&id) || !closed.insert(id) {
                            return Err(ConversationError::InvalidToolPair);
                        }
                    }
                }
            }
            MessageRole::User | MessageRole::System | MessageRole::Developer => {}
        }
    }
    if open.is_empty() {
        Ok(())
    } else {
        Err(ConversationError::InvalidToolPair)
    }
}

/// One restored lane leaf and application key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneProjection {
    /// Stable application key (`main` for the mandatory lane).
    pub name: std::sync::Arc<str>,
    /// Current leaf, when any conversation entry exists.
    pub leaf_id: Option<EntryId>,
}

/// Restored operation relation. Lanes are not the lineage model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSummary {
    /// Operation identity.
    pub run_id: RunId,
    /// Lane that accepted the operation.
    pub lane_id: LaneId,
    /// Immutable run relation.
    pub relation: RunRelation,
    /// Accepted security context.
    pub security: RunSecurityContext,
    /// Accepted propagation policy.
    pub propagation: RunPropagationPolicy,
    /// Whether a terminal record has been applied.
    pub terminal: bool,
}

impl OperationSummary {
    /// Parent invocation effect when this is a child or delegated run.
    #[must_use]
    pub fn invocation_effect_id(&self) -> Option<EffectId> {
        self.relation.parent_effect_id()
    }
}

/// Session-level conversation, lane, operation, and child-mapping projection.
///
/// Rebuilt from the journal. Not part of `kernel-state` and not hashed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionProjection {
    session_id: Option<SessionId>,
    metadata: Metadata,
    lanes: BTreeMap<LaneId, LaneProjection>,
    entries: BTreeMap<EntryId, ConversationEntry>,
    operations: BTreeMap<RunId, OperationSummary>,
    child_mappings: BTreeMap<(RunId, EffectId), ChildRunPrepared>,
}

impl SessionProjection {
    /// Empty projection for `session_id`.
    #[must_use]
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id: Some(session_id),
            ..Self::default()
        }
    }

    /// Session identity when known.
    #[must_use]
    pub const fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    /// Session metadata. Never grants authority.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Lane projections keyed by durable lane identity.
    #[must_use]
    pub const fn lanes(&self) -> &BTreeMap<LaneId, LaneProjection> {
        &self.lanes
    }

    /// Immutable conversation entries.
    #[must_use]
    pub const fn entries(&self) -> &BTreeMap<EntryId, ConversationEntry> {
        &self.entries
    }

    /// Restored operations keyed by run identity.
    #[must_use]
    pub const fn operations(&self) -> &BTreeMap<RunId, OperationSummary> {
        &self.operations
    }

    /// Parent-effect to child locator mappings.
    #[must_use]
    pub const fn child_mappings(&self) -> &BTreeMap<(RunId, EffectId), ChildRunPrepared> {
        &self.child_mappings
    }

    /// Mandatory `main` lane, when created.
    #[must_use]
    pub fn main_lane(&self) -> Option<(LaneId, &LaneProjection)> {
        self.lane("main")
    }

    /// Lane projection for a stable application name.
    #[must_use]
    pub fn lane(&self, name: &str) -> Option<(LaneId, &LaneProjection)> {
        self.lanes
            .iter()
            .find(|(_, lane)| lane.name.as_ref() == name)
            .map(|(id, lane)| (*id, lane))
    }

    /// Lane projection for a durable lane identity.
    #[must_use]
    pub fn lane_by_id(&self, lane_id: LaneId) -> Option<&LaneProjection> {
        self.lanes.get(&lane_id)
    }

    /// Non-terminal operation on `lane_id`, when the lane is busy.
    #[must_use]
    pub fn active_on_lane(&self, lane_id: LaneId) -> Option<RunId> {
        self.operations.values().find_map(|operation| {
            (operation.lane_id == lane_id && !operation.terminal).then_some(operation.run_id)
        })
    }

    /// Lookup the prepared child for one parent effect.
    #[must_use]
    pub fn child_mapping(
        &self,
        parent_run_id: RunId,
        parent_effect_id: EffectId,
    ) -> Option<&ChildRunPrepared> {
        self.child_mappings.get(&(parent_run_id, parent_effect_id))
    }

    /// Walk ancestors ending at `leaf_id` without requiring closed tool pairs.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError`] when the path is incomplete or cyclic.
    pub fn walk(&self, leaf_id: EntryId) -> Result<Vec<ConversationEntry>, ConversationError> {
        walk_conversation(&self.entries, leaf_id)
    }

    /// Extract model-ready history ending at `leaf_id`.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError`] when the path is incomplete or tool pairs
    /// are invalid.
    pub fn history(&self, leaf_id: EntryId) -> Result<Vec<ConversationEntry>, ConversationError> {
        extract_history(&self.entries, leaf_id)
    }

    /// Apply one committed envelope. Operation records that are not conversation
    /// facts are ignored except `RunAccepted`, terminals, and `ChildRunPrepared`.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationError`] for an immutable conflict or invalid leaf.
    pub fn apply_envelope(&mut self, record: &RecordEnvelope) -> Result<(), ConversationError> {
        if let Some(session_id) = self.session_id
            && record.session_id() != session_id
        {
            return Ok(());
        }
        if self.session_id.is_none() {
            self.session_id = Some(record.session_id());
        }
        match record.body() {
            RecordBody::SessionCreated(created) => {
                self.metadata = created.metadata().clone();
            }
            RecordBody::LaneCreated(created) => self.apply_lane_created(record.lane_id(), created),
            RecordBody::ConversationEntry(entry) => self.apply_recorded_entry(record, entry)?,
            RecordBody::LaneMoved(moved) => self.apply_lane_moved(record.lane_id(), moved)?,
            RecordBody::EntryAppended(entry) => {
                self.synthesize_from_message(
                    &entry.message,
                    record,
                    entry
                        .parent_message_id
                        .map(|id| EntryId::from_bytes(id.to_bytes())),
                )?;
            }
            RecordBody::ToolCallSettled(settled) => {
                self.synthesize_from_tool(settled, record)?;
            }
            RecordBody::RunAccepted(accepted) => {
                self.apply_run_accepted(record.lane_id(), accepted);
            }
            RecordBody::RunCompleted(_)
            | RecordBody::RunFailed(_)
            | RecordBody::RunCancelled(_) => {
                if let Some(run_id) = record.run_id()
                    && let Some(operation) = self.operations.get_mut(&run_id)
                {
                    operation.terminal = true;
                }
            }
            RecordBody::ChildRunPrepared(prepared) => {
                self.child_mappings.insert(
                    (prepared.parent_run_id, prepared.parent_effect_id),
                    prepared.clone(),
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_lane_created(&mut self, lane_id: LaneId, created: &LaneCreated) {
        self.lanes.entry(lane_id).or_insert(LaneProjection {
            name: std::sync::Arc::from(created.name()),
            leaf_id: None,
        });
    }

    fn apply_recorded_entry(
        &mut self,
        record: &RecordEnvelope,
        entry: &ConversationEntry,
    ) -> Result<(), ConversationError> {
        if entry.lane_id() != record.lane_id() || entry.sequence() != record.sequence() {
            return Err(ConversationError::EnvelopeMismatch);
        }
        apply_conversation_entry(&mut self.entries, entry.clone())?;
        if let Some(lane) = self.lanes.get_mut(&record.lane_id()) {
            lane.leaf_id = Some(entry.id());
        }
        Ok(())
    }

    fn apply_lane_moved(
        &mut self,
        lane_id: LaneId,
        moved: &LaneMoved,
    ) -> Result<(), ConversationError> {
        if !self.entries.contains_key(&moved.leaf_id()) {
            return Err(ConversationError::UnknownLeaf);
        }
        if let Some(lane) = self.lanes.get_mut(&lane_id) {
            lane.leaf_id = Some(moved.leaf_id());
        }
        Ok(())
    }

    fn synthesize_from_message(
        &mut self,
        message: &Message,
        record: &RecordEnvelope,
        parent_id: Option<EntryId>,
    ) -> Result<(), ConversationError> {
        let id = EntryId::from_bytes(message.id().to_bytes());
        if self.entries.contains_key(&id) {
            return Ok(());
        }
        let parent = parent_id.or_else(|| {
            self.lanes
                .get(&record.lane_id())
                .and_then(|lane| lane.leaf_id)
        });
        let entry =
            ConversationEntry::from_message(message, parent, record.lane_id(), record.sequence())?;
        apply_conversation_entry(&mut self.entries, entry)?;
        if let Some(lane) = self.lanes.get_mut(&record.lane_id()) {
            lane.leaf_id = Some(id);
        }
        Ok(())
    }

    fn synthesize_from_tool(
        &mut self,
        settled: &ToolCallSettled,
        record: &RecordEnvelope,
    ) -> Result<(), ConversationError> {
        self.synthesize_from_message(&settled.message, record, None)
    }

    fn apply_run_accepted(&mut self, lane_id: LaneId, accepted: &RunAccepted) {
        self.operations
            .entry(accepted.run_id())
            .or_insert(OperationSummary {
                run_id: accepted.run_id(),
                lane_id,
                relation: accepted.relation().clone(),
                security: accepted.security().clone(),
                propagation: accepted.propagation(),
                terminal: false,
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{TextBlock, ToolCallBlock, ToolResultBlock};
    use crate::ids::Id;
    use crate::message::ProviderIds;
    use crate::raw_json::RawJson;
    use crate::time::Timestamp;

    fn id<T: crate::ids::IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn text_message(ordinal: u64, role: MessageRole, text: &str) -> Message {
        Message::try_new(
            id(ordinal),
            role,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
            Timestamp::from_unix_ms(0).expect("ts"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn entry(
        ordinal: u64,
        parent: Option<u64>,
        sequence: u64,
        role: MessageRole,
        text: &str,
    ) -> ConversationEntry {
        let message = text_message(ordinal, role, text);
        ConversationEntry::from_message(&message, parent.map(id), id(2), sequence).expect("entry")
    }

    #[test]
    fn parent_chain_is_immutable_and_equal_replay_is_idempotent() {
        let mut entries = BTreeMap::new();
        let a = entry(10, None, 1, MessageRole::User, "a");
        apply_conversation_entry(&mut entries, a.clone()).expect("a");
        apply_conversation_entry(&mut entries, a.clone()).expect("equal");
        let rewritten = ConversationEntry::try_new(
            a.id(),
            a.parent_id(),
            a.lane_id(),
            a.sequence(),
            crate::conversation::EntryBody::Message(text_message(10, MessageRole::User, "changed")),
        )
        .expect("rewrite");
        assert_eq!(
            apply_conversation_entry(&mut entries, rewritten),
            Err(ConversationError::ImmutableConflict)
        );
        assert_eq!(entries.get(&a.id()).expect("kept").parent_id(), None);
        let same_body_later_sequence =
            ConversationEntry::try_new(a.id(), a.parent_id(), a.lane_id(), 9, a.body().clone())
                .expect("later sequence");
        apply_conversation_entry(&mut entries, same_body_later_sequence).expect("sequence");
    }

    #[test]
    fn branch_foundation_does_not_rewrite_the_shared_parent() {
        let mut entries = BTreeMap::new();
        let a = entry(10, None, 1, MessageRole::User, "a");
        let b = entry(11, Some(10), 2, MessageRole::Assistant, "b");
        let c = entry(12, Some(11), 3, MessageRole::User, "c");
        let d = entry(13, Some(11), 4, MessageRole::User, "d");
        for item in [a, b.clone(), c.clone(), d.clone()] {
            apply_conversation_entry(&mut entries, item).expect("apply");
        }
        assert_eq!(entries.get(&b.id()).expect("b").parent_id(), Some(id(10)));
        let history_d = extract_history(&entries, d.id()).expect("d");
        let history_c = extract_history(&entries, c.id()).expect("c");
        assert_eq!(
            history_d
                .iter()
                .map(ConversationEntry::id)
                .collect::<Vec<_>>(),
            vec![id(10), id(11), id(13)]
        );
        assert_eq!(
            history_c
                .iter()
                .map(ConversationEntry::id)
                .collect::<Vec<_>>(),
            vec![id(10), id(11), id(12)]
        );
    }

    #[test]
    fn extract_history_rejects_an_incomplete_tool_pair() {
        let mut entries = BTreeMap::new();
        let user = text_message(10, MessageRole::User, "q");
        let call = ToolCallBlock::try_new(
            id::<crate::ids::ToolCallTag>(20),
            "lookup",
            RawJson::parse(r#"{"q":1}"#).expect("json"),
        )
        .expect("call");
        let assistant = Message::try_new(
            id(11),
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(call)],
            Timestamp::from_unix_ms(0).expect("ts"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("assistant");
        apply_conversation_entry(
            &mut entries,
            ConversationEntry::from_message(&user, None, id(2), 1).expect("user"),
        )
        .expect("user");
        apply_conversation_entry(
            &mut entries,
            ConversationEntry::from_message(&assistant, Some(id(10)), id(2), 2).expect("asst"),
        )
        .expect("asst");
        assert_eq!(
            extract_history(&entries, id(11)),
            Err(ConversationError::InvalidToolPair)
        );
        assert_eq!(walk_conversation(&entries, id(11)).expect("walk").len(), 2);
    }

    #[test]
    fn extract_history_accepts_a_closed_tool_pair() {
        let mut entries = BTreeMap::new();
        let call_id = id::<crate::ids::ToolCallTag>(20);
        let user = text_message(10, MessageRole::User, "q");
        let call = ToolCallBlock::try_new(
            call_id,
            "lookup",
            RawJson::parse(r#"{"q":1}"#).expect("json"),
        )
        .expect("call");
        let assistant = Message::try_new(
            id(11),
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(call)],
            Timestamp::from_unix_ms(0).expect("ts"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("assistant");
        let result = ToolResultBlock::try_new(
            call_id,
            vec![ContentBlock::Text(TextBlock::try_new("ok").expect("text"))],
            false,
        )
        .expect("result");
        let tool = Message::try_new(
            id(12),
            MessageRole::Tool,
            vec![ContentBlock::ToolResult(result)],
            Timestamp::from_unix_ms(0).expect("ts"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("tool");
        apply_conversation_entry(
            &mut entries,
            ConversationEntry::from_message(&user, None, id(2), 1).expect("user"),
        )
        .expect("user");
        apply_conversation_entry(
            &mut entries,
            ConversationEntry::from_message(&assistant, Some(id(10)), id(2), 2).expect("asst"),
        )
        .expect("asst");
        apply_conversation_entry(
            &mut entries,
            ConversationEntry::from_message(&tool, Some(id(11)), id(2), 3).expect("tool"),
        )
        .expect("tool");
        let history = extract_history(&entries, id(12)).expect("history");
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn drop_snapshot_records_leaves_the_tree() {
        let mut projection = SessionProjection::new(id(1));
        let user = entry(10, None, 3, MessageRole::User, "hello");
        projection.apply_lane_created(id(2), &LaneCreated::try_new("main").expect("lane"));
        apply_conversation_entry(&mut projection.entries, user.clone()).expect("entry");
        projection.lanes.get_mut(&id(2)).expect("lane").leaf_id = Some(user.id());
        let without_snapshot = projection.clone();
        let _ = RecordBody::SnapshotWritten(crate::session::SnapshotWritten::new(
            9,
            crate::digest::Digest::raw_json(b"snap"),
        ));
        assert_eq!(projection.entries(), without_snapshot.entries());
        assert_eq!(
            projection.main_lane().expect("main").1.leaf_id,
            Some(user.id())
        );
        assert_eq!(
            projection.lane("main").expect("named").0,
            projection.main_lane().expect("main").0
        );
        assert!(projection.lane("research").is_none());
        assert!(projection.lane_by_id(id(2)).is_some());
    }
}
