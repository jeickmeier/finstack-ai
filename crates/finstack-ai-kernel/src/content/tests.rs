use super::*;
use crate::ids::ToolCallId;
use crate::raw_json::RawJson;
use serde::de::Deserialize;
use serde::de::value::SeqDeserializer;

#[test]
fn blob_ref_rejects_inline_semantics_and_invalid_labels() {
    let blob = BlobRef::try_new("b1", "image/png", 10, None, None::<&str>).expect("ok");
    let json = serde_json::to_string(&blob).expect("ser");
    assert!(!json.contains("payload"));
    assert!(!json.contains("bytes"));
    assert!(BlobRef::try_new("", "image/png", 1, None, None::<&str>).is_err());
    assert!(BlobRef::try_new("b1", "x".repeat(257), 1, None, None::<&str>).is_err());
}

#[test]
fn large_declared_length_serializes_as_small_reference() {
    let blob = BlobRef::try_new("huge", "application/pdf", 50_000_000, None, Some("doc.pdf"))
        .expect("blob");
    let json = serde_json::to_string(&blob).expect("ser");
    assert!(json.len() < 200);
    assert!(json.contains("50000000"));
    let round: BlobRef = serde_json::from_str(&json).expect("de");
    assert_eq!(round, blob);
}

#[test]
fn content_block_round_trip_and_unknown_kind() {
    let block = ContentBlock::Text(TextBlock::try_new("hi").expect("text"));
    let json = serde_json::to_string(&block).expect("ser");
    assert!(json.contains("\"kind\":\"text\""));
    let round: ContentBlock = serde_json::from_str(&json).expect("de");
    assert_eq!(round, block);
    let err = serde_json::from_str::<ContentBlock>(r#"{"kind":"reasoning","text":"x"}"#);
    assert!(err.is_err());
}

#[test]
fn content_blocks_reject_unknown_and_inapplicable_members() {
    let inline_media = serde_json::from_str::<ContentBlock>(
        r#"{
            "kind":"image",
            "blob":{"id":"b1","media_type":"image/png","length":1},
            "data_hex":"00"
        }"#,
    );
    assert!(inline_media.is_err());

    let extra_text =
        serde_json::from_str::<ContentBlock>(r#"{"kind":"text","text":"hi","extra":true}"#);
    assert!(extra_text.is_err());

    let conflicting_opaque = serde_json::from_str::<ContentBlock>(
        r#"{
            "kind":"opaque",
            "media_type":"application/vnd.example",
            "payload":{
                "encoding":"bytes",
                "data_hex":"00",
                "data":{"also":"present"}
            }
        }"#,
    );
    assert!(conflicting_opaque.is_err());
}

#[test]
fn nested_raw_json_preserves_strict_source_validation() {
    let duplicate = serde_json::from_str::<ContentBlock>(
        r#"{
            "kind":"tool_call",
            "tool_call_id":"01234567-89ab-7cde-89ab-0123456789ab",
            "tool_name":"lookup",
            "arguments":{"q":1,"q":2}
        }"#,
    );
    assert!(duplicate.is_err());

    let escaped = r"\u0061".repeat(crate::raw_json::RAW_JSON_MAX_BYTES / 6 + 1);
    let oversized_source = format!(
        r#"{{
            "kind":"json",
            "value":"{escaped}"
        }}"#
    );
    let oversized = serde_json::from_str::<ContentBlock>(&oversized_source);
    assert!(oversized.is_err());
}

#[test]
fn content_block_accepts_owned_and_reader_json_inputs() {
    let value = serde_json::json!({"kind": "text", "text": "owned"});
    let owned: ContentBlock = serde_json::from_value(value).expect("owned value");
    assert_eq!(owned.kind_name(), "text");

    let input = br#"{"kind":"text","text":"reader"}"#;
    let reader: ContentBlock = serde_json::from_reader(input.as_slice()).expect("reader input");
    assert_eq!(reader.kind_name(), "text");
}

#[test]
fn nested_tool_kind_is_rejected_before_its_payload_is_decoded() {
    let error = serde_json::from_str::<ContentBlock>(
        r#"{
            "kind":"tool_result",
            "tool_call_id":"01234567-89ab-7cde-89ab-0123456789ab",
            "content":[{
                "kind":"tool_result",
                "tool_call_id":"01234567-89ab-7cde-89ab-0123456789cd",
                "content":[{"kind":"reasoning","text":"must not be reached"}]
            }]
        }"#,
    )
    .expect_err("nested tool result");
    assert!(error.to_string().contains("cannot nest"));
}

#[test]
fn tool_result_rejects_nested_tool_blocks() {
    let call_id = ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    let nested = ContentBlock::ToolCall(
        ToolCallBlock::try_new(call_id, "lookup", RawJson::parse("{}").expect("json"))
            .expect("call"),
    );
    let err = ToolResultBlock::try_new(call_id, vec![nested], false).expect_err("nested");
    assert!(matches!(err, ContentError::NestedToolBlock));
}

#[test]
fn text_exact_and_one_over_ceiling() {
    let exact = "a".repeat(TEXT_MAX_BYTES);
    assert!(TextBlock::try_new(&exact).is_ok());
    let over = "a".repeat(TEXT_MAX_BYTES + 1);
    assert!(matches!(
        TextBlock::try_new(&over).expect_err("over"),
        ContentError::TextTooLarge { .. }
    ));
}

#[test]
fn content_item_size_hint_rejects_before_reading_elements() {
    let items = core::iter::repeat_with(|| -> serde_json::Value {
        panic!("oversized sequence should be rejected before reading an element")
    })
    .take(CONTENT_MAX_ITEMS + 1);
    let deserializer = SeqDeserializer::<_, serde_json::Error>::new(items);
    let Err(error) = ContentItems::deserialize(deserializer) else {
        panic!("oversized sequence unexpectedly succeeded");
    };
    assert!(error.to_string().contains("item count"));
}

#[test]
fn bounded_string_checks_escaped_length_before_decoding() {
    let exact: BoundedString<4> =
        serde_json::from_str(r#""\u0061\u0061\u0061\u0061""#).expect("exact escapes");
    assert_eq!(exact.into_inner(), "aaaa");

    let emoji: BoundedString<4> =
        serde_json::from_str(r#""\ud83d\ude00""#).expect("surrogate pair");
    assert_eq!(emoji.into_inner(), "😀");

    let over = serde_json::from_str::<BoundedString<4>>(r#""\u0061\u0061\u0061\u0061\u0061""#);
    assert!(over.is_err());
}
