use std::collections::BTreeMap;

use super::tree::{apply_conversation_entry, extract_history, walk_conversation};
use super::*;
use crate::content::{BlobRef, ContentBlock, MediaRef, TextBlock, ToolCallBlock, ToolResultBlock};
use crate::primitives::Timestamp;
use crate::primitives::{Id, IdTag, MessageId, ToolCallId, ToolCallTag};
use crate::primitives::{Metadata, RawJson};
use crate::records::{
    LaneCreated, LaneMoved, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft,
    RecordEnvelope, SnapshotWritten,
};

fn mid() -> MessageId {
    MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id")
}

fn tid() -> ToolCallId {
    ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789cd").expect("id")
}

#[test]
fn role_block_matrix_rejects_tool_result_on_user() {
    let result = ToolResultBlock::try_new(
        tid(),
        vec![ContentBlock::Text(TextBlock::try_new("ok").expect("t"))],
        false,
    )
    .expect("result");
    let err = Message::try_new(
        mid(),
        MessageRole::User,
        vec![ContentBlock::ToolResult(result)],
        Timestamp::from_unix_ms(0).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect_err("mismatch");
    assert_eq!(err.code(), "role_block_mismatch");
}

#[test]
fn invalid_tool_associations_are_rejected() {
    let result = ToolResultBlock::try_new(
        tid(),
        vec![ContentBlock::Text(TextBlock::try_new("ok").expect("t"))],
        false,
    )
    .expect("result");
    let message = Message::try_new(
        mid(),
        MessageRole::Tool,
        vec![ContentBlock::ToolResult(result.clone())],
        Timestamp::from_unix_ms(0).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message");
    let unknown = message
        .validate_tool_associations(Some(&[]))
        .expect_err("unknown");
    assert_eq!(unknown.code(), "unknown_tool_association");

    let duplicate = Message::try_new(
        mid(),
        MessageRole::Tool,
        vec![
            ContentBlock::ToolResult(result.clone()),
            ContentBlock::ToolResult(result),
        ],
        Timestamp::from_unix_ms(0).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect_err("dup");
    assert_eq!(duplicate.code(), "duplicate_tool_association");
}

#[test]
fn message_round_trip_preserves_timestamp_and_file_ref() {
    let blob =
        BlobRef::try_new("b", "application/pdf", 9_000_000, None, None::<&str>).expect("blob");
    let message = Message::try_new(
        mid(),
        MessageRole::User,
        vec![ContentBlock::File(MediaRef::new(blob))],
        Timestamp::from_unix_ms(1_700_000_000_000).expect("ts"),
        None,
        ProviderIds::try_new(Some("req-1"), None::<&str>, None::<&str>).expect("ids"),
        Metadata::empty(),
    )
    .expect("message");
    let json = serde_json::to_string(&message).expect("ser");
    assert!(!json.contains("payload"));
    let round: Message = serde_json::from_str(&json).expect("de");
    assert_eq!(round, message);
}

#[test]
fn assistant_tool_call_allowed() {
    let call = ToolCallBlock::try_new(tid(), "lookup", RawJson::parse(r#"{"q":1}"#).expect("j"))
        .expect("call");
    let message = Message::try_new(
        mid(),
        MessageRole::Assistant,
        vec![ContentBlock::ToolCall(call)],
        Timestamp::from_unix_ms(0).expect("ts"),
        Some(
            ModelRef::try_new_with_options(
                "openai",
                "gpt-test",
                Some(ThinkingLevel::Medium),
                Some(128_000),
                Some(true),
            )
            .expect("model"),
        ),
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message");
    assert_eq!(message.content().len(), 1);
}

#[test]
fn model_ref_optionals_independent() {
    let base = ModelRef::try_new("p", "m").expect("base");
    assert_eq!(
        serde_json::to_string(&base).expect("base JSON"),
        r#"{"provider":"p","model":"m"}"#
    );

    for thinking_level in [None, Some(ThinkingLevel::High)] {
        for context_length in [None, Some(8_192)] {
            for fast in [None, Some(true)] {
                let model =
                    ModelRef::try_new_with_options("p", "m", thinking_level, context_length, fast)
                        .expect("combination");
                let json = serde_json::to_string(&model).expect("serialize");
                let round: ModelRef = serde_json::from_str(&json).expect("deserialize");
                assert_eq!(round, model);
                assert_eq!(round.thinking_level(), thinking_level);
                assert_eq!(round.context_length(), context_length);
                assert_eq!(round.fast(), fast);
            }
        }
    }

    assert_eq!(
        ModelRef::try_new_with_options("p", "m", None, Some(0), None)
            .unwrap_err()
            .code(),
        "invalid_context_length"
    );
}

#[test]
fn model_ref_context_length_stays_in_portable_json_range() {
    const PORTABLE_MAX: u64 = 9_007_199_254_740_991;
    assert!(ModelRef::try_new_with_options("p", "m", None, Some(PORTABLE_MAX), None).is_ok());
    assert_eq!(
        ModelRef::try_new_with_options("p", "m", None, Some(PORTABLE_MAX + 1), None)
            .unwrap_err()
            .code(),
        "invalid_context_length"
    );
}

#[test]
fn messages_and_model_refs_reject_unknown_members() {
    let unknown_model =
        serde_json::from_str::<ModelRef>(r#"{"provider":"p","model":"m","unsupported":true}"#);
    assert!(unknown_model.is_err());

    let unknown_message = serde_json::from_str::<Message>(
        r#"{
            "id":"01234567-89ab-7cde-89ab-0123456789ab",
            "role":"user",
            "content":[],
            "created_at":"1970-01-01T00:00:00.000Z",
            "provider_ids":{},
            "metadata":{},
            "unsupported":true
        }"#,
    );
    assert!(unknown_message.is_err());
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
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
fn preview_validates_against_an_overlay_without_cloning_committed_entries() {
    let mut projection = SessionProjection::new(id(1));
    let root = entry(10, None, 1, MessageRole::User, "a");
    apply_conversation_entry(&mut projection.entries, root.clone()).expect("root");
    let child = entry(11, Some(10), 2, MessageRole::Assistant, "b");
    let grandchild = entry(12, Some(11), 3, MessageRole::User, "c");
    projection
        .preview_conversation_entries([&child, &grandchild])
        .expect("overlay parent");
    assert_eq!(projection.entries().len(), 1);
    let rewritten = ConversationEntry::try_new(
        root.id(),
        root.parent_id(),
        root.lane_id(),
        root.sequence(),
        crate::conversation::EntryBody::Message(text_message(10, MessageRole::User, "changed")),
    )
    .expect("rewrite");
    assert_eq!(
        projection.preview_conversation_entries([&rewritten]),
        Err(ConversationError::ImmutableConflict)
    );
    let orphan = entry(13, Some(99), 4, MessageRole::User, "orphan");
    assert_eq!(
        projection.preview_conversation_entries([&orphan]),
        Err(ConversationError::MissingParent)
    );
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
        id::<ToolCallTag>(20),
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
    let call_id = id::<ToolCallTag>(20);
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
    projection
        .apply_lane_created(id(2), &LaneCreated::try_new("main").expect("lane"))
        .expect("create lane");
    apply_conversation_entry(&mut projection.entries, user.clone()).expect("entry");
    projection.lanes.get_mut(&id(2)).expect("lane").leaf_id = Some(user.id());
    let without_snapshot = projection.clone();
    let snapshot = RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<crate::primitives::RecordTag>(20),
        id::<crate::primitives::SessionTag>(1),
        id::<crate::primitives::LaneTag>(2),
        None,
        4,
        Timestamp::from_unix_ms(0).expect("ts"),
        None,
        crate::primitives::Digest::raw_json(b"payload"),
        None,
        crate::primitives::Digest::raw_json(b"checksum"),
        vec![],
        RecordBody::SnapshotWritten(SnapshotWritten::new(
            9,
            crate::primitives::Digest::raw_json(b"snap"),
        )),
    )
    .expect("snapshot envelope");
    projection
        .apply_envelope(&snapshot)
        .expect("apply snapshot");
    assert_eq!(projection.entries(), without_snapshot.entries());
    assert_eq!(projection.lanes, without_snapshot.lanes);
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

#[test]
fn structural_preview_rejects_duplicate_lanes_and_unknown_targets() {
    let session_id = id::<crate::SessionTag>(1);
    let main_lane = id::<crate::LaneTag>(2);
    let other_lane = id::<crate::LaneTag>(3);
    let timestamp = Timestamp::from_unix_ms(0).expect("timestamp");
    let draft = |ordinal, lane_id, body| {
        RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<crate::RecordTag>(ordinal),
            session_id,
            lane_id,
            None,
            timestamp,
            vec![],
            body,
        )
        .expect("draft")
    };
    let main = draft(
        10,
        main_lane,
        RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
    );
    let duplicate_id = draft(
        11,
        main_lane,
        RecordBody::LaneCreated(LaneCreated::try_new("other").expect("lane")),
    );
    let duplicate_name = draft(
        12,
        other_lane,
        RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
    );
    let projection = SessionProjection::new(session_id)
        .preview_structural_drafts([&main])
        .expect("main preview");
    assert_eq!(
        projection
            .preview_structural_drafts([&duplicate_id])
            .expect_err("duplicate id"),
        ConversationError::DuplicateLaneId
    );
    assert_eq!(
        projection
            .preview_structural_drafts([&duplicate_name])
            .expect_err("duplicate name"),
        ConversationError::DuplicateLaneName
    );

    let unknown_leaf = id::<crate::EntryTag>(99);
    let moved = draft(
        13,
        main_lane,
        RecordBody::LaneMoved(LaneMoved::new(unknown_leaf)),
    );
    assert_eq!(
        projection
            .preview_structural_drafts([&moved])
            .expect_err("unknown move"),
        ConversationError::UnknownLeaf
    );
    let fork = entry(20, Some(99), 4, MessageRole::User, "fork");
    let fork = draft(14, main_lane, RecordBody::ConversationEntry(fork));
    assert_eq!(
        projection
            .preview_structural_drafts([&fork])
            .expect_err("unknown fork parent"),
        ConversationError::MissingParent
    );
}

#[test]
fn committed_lane_creation_rejects_duplicate_id_and_name() {
    let session_id = id::<crate::SessionTag>(1);
    let timestamp = Timestamp::from_unix_ms(0).expect("timestamp");
    let envelope = |sequence, lane_id, name: &str| {
        let body = RecordBody::LaneCreated(LaneCreated::try_new(name).expect("lane"));
        RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<crate::RecordTag>(sequence + 20),
            session_id,
            lane_id,
            None,
            sequence,
            timestamp,
            None,
            crate::Digest::raw_json(b"payload"),
            None,
            crate::Digest::raw_json(b"checksum"),
            vec![],
            body,
        )
        .expect("envelope")
    };
    let mut projection = SessionProjection::new(session_id);
    projection
        .apply_envelope(&envelope(1, id(2), "main"))
        .expect("first lane");
    assert_eq!(
        projection
            .apply_envelope(&envelope(2, id(2), "other"))
            .expect_err("duplicate id"),
        ConversationError::DuplicateLaneId
    );
    assert_eq!(
        projection
            .apply_envelope(&envelope(3, id(3), "main"))
            .expect_err("duplicate name"),
        ConversationError::DuplicateLaneName
    );
}
