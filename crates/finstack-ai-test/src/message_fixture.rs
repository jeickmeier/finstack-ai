//! Public-rust-api fixture runners for content blocks, blob refs, and messages.

use finstack_ai_kernel::{BlobRef, ContentBlock, Message, ToolCallId};
use serde_json::Value;

use crate::public_api_fixture::{Expect, PublicApiFixture, PublicApiFixtureError};

/// Execute a blob-ref / content-block / message fixture.
pub(crate) fn run_message_subject(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    match fixture.subject.as_str() {
        "blob_ref" => run_blob_ref(fixture),
        "content_block" => run_content_block(fixture),
        "message" => run_message(fixture),
        other => Err(fail(format!("unsupported message subject {other}"))),
    }
}

fn run_blob_ref(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_text(fixture)?;
    match fixture.operation.as_str() {
        "roundtrip" | "parse" => match serde_json::from_str::<BlobRef>(&input) {
            Ok(blob) => {
                if !fixture.expect.ok {
                    return Err(fail("expected blob_ref failure"));
                }
                assert_serialized_json(&fixture.expect, &blob)?;
                if let Some(length) = fixture.expect.extras.get("length") {
                    let expected = length
                        .as_u64()
                        .ok_or_else(|| fail("expect.length must be u64"))?;
                    if blob.length() != expected {
                        return Err(fail(format!(
                            "length mismatch: {} != {expected}",
                            blob.length()
                        )));
                    }
                }
                if let Some(max) = fixture
                    .expect
                    .extras
                    .get("max_serialized_bytes")
                    .and_then(Value::as_u64)
                {
                    let bytes =
                        serde_json::to_vec(&blob).map_err(|error| fail(error.to_string()))?;
                    if (bytes.len() as u64) > max {
                        return Err(fail("serialized blob_ref exceeded max_serialized_bytes"));
                    }
                }
                Ok(())
            }
            Err(error) => assert_error_code(&fixture.expect, classify_parse_error(&error)),
        },
        other => Err(fail(format!("unsupported blob_ref operation {other}"))),
    }
}

fn run_content_block(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_text(fixture)?;
    match fixture.operation.as_str() {
        "roundtrip" | "parse" => match serde_json::from_str::<ContentBlock>(&input) {
            Ok(block) => {
                if !fixture.expect.ok {
                    return Err(fail("expected content_block failure"));
                }
                assert_serialized_json(&fixture.expect, &block)?;
                if let Some(kind) = fixture.expect.extras.get("kind").and_then(Value::as_str)
                    && block.kind_name() != kind
                {
                    return Err(fail(format!(
                        "kind mismatch: {} != {kind}",
                        block.kind_name()
                    )));
                }
                Ok(())
            }
            Err(error) => assert_error_code(&fixture.expect, classify_parse_error(&error)),
        },
        other => Err(fail(format!("unsupported content_block operation {other}"))),
    }
}

fn run_message(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = require_input_text(fixture)?;
    match fixture.operation.as_str() {
        "roundtrip" | "parse" => match serde_json::from_str::<Message>(&input) {
            Ok(message) => {
                if !fixture.expect.ok {
                    return Err(fail("expected message failure"));
                }
                if let Some(known) = fixture.expect.extras.get("known_tool_call_ids") {
                    let ids = parse_tool_call_ids(known)?;
                    if let Err(error) = message.validate_tool_associations(Some(&ids)) {
                        return assert_error_code(&fixture.expect, error.code());
                    }
                }
                assert_serialized_json(&fixture.expect, &message)?;
                if let Some(role) = fixture.expect.extras.get("role").and_then(Value::as_str)
                    && message.role().as_str() != role
                {
                    return Err(fail(format!(
                        "role mismatch: {} != {role}",
                        message.role().as_str()
                    )));
                }
                Ok(())
            }
            Err(error) => {
                let code = classify_message_error(&error);
                if fixture.expect.ok {
                    return Err(fail(format!(
                        "expected success, got error {code} ({error})"
                    )));
                }
                assert_error_code(&fixture.expect, code)
            }
        },
        "validate_associations" => {
            let message: Message =
                serde_json::from_str(&input).map_err(|error| fail(error.to_string()))?;
            let known = match fixture.expect.extras.get("known_tool_call_ids") {
                Some(value) => Some(parse_tool_call_ids(value)?),
                None => None,
            };
            match message.validate_tool_associations(known.as_deref()) {
                Ok(()) => {
                    if !fixture.expect.ok {
                        return Err(fail("expected association validation failure"));
                    }
                    Ok(())
                }
                Err(error) => assert_error_code(&fixture.expect, error.code()),
            }
        }
        other => Err(fail(format!("unsupported message operation {other}"))),
    }
}

fn require_input_text(fixture: &PublicApiFixture) -> Result<String, PublicApiFixtureError> {
    let input = fixture
        .input
        .as_ref()
        .ok_or_else(|| fail("fixture requires input"))?;
    // Re-serialize fixture input so nested content uses ordinary JSON text.
    serde_json::to_string(input).map_err(|error| fail(error.to_string()))
}

fn assert_serialized_json<T: serde::Serialize>(
    expect: &Expect,
    value: &T,
) -> Result<(), PublicApiFixtureError> {
    let actual = serde_json::to_string(value).map_err(|error| fail(error.to_string()))?;
    if let Some(expected) = expect.extras.get("serialized_json").and_then(Value::as_str)
        && actual != expected
    {
        return Err(fail(format!(
            "serialized_json mismatch:\nactual:   {actual}\nexpected: {expected}"
        )));
    }
    Ok(())
}

fn parse_tool_call_ids(value: &Value) -> Result<Vec<ToolCallId>, PublicApiFixtureError> {
    let array = value
        .as_array()
        .ok_or_else(|| fail("known_tool_call_ids must be an array"))?;
    let mut ids = Vec::with_capacity(array.len());
    for item in array {
        let text = item
            .as_str()
            .ok_or_else(|| fail("known_tool_call_ids entries must be strings"))?;
        ids.push(ToolCallId::parse(text).map_err(|error| fail(error.to_string()))?);
    }
    Ok(ids)
}

fn classify_parse_error(error: &serde_json::Error) -> &'static str {
    let text = error.to_string();
    if text.contains("unknown variant") || text.contains("unknown opaque encoding") {
        return "unknown_kind";
    }
    if text.contains("empty, oversized, or contains NUL") {
        return "invalid_label";
    }
    if text.contains("role") && text.contains("does not allow") {
        return "role_block_mismatch";
    }
    if text.contains("duplicate tool_call_id") {
        return "duplicate_tool_association";
    }
    if text.contains("unknown tool_call_id") {
        return "unknown_tool_association";
    }
    if text.contains("requires at least one tool_result") {
        return "missing_tool_result";
    }
    if text.contains("nest tool_call") {
        return "nested_tool_block";
    }
    "parse"
}

fn classify_message_error(error: &serde_json::Error) -> &'static str {
    let text = error.to_string();
    for code in [
        "role_block_mismatch",
        "duplicate_tool_association",
        "unknown_tool_association",
        "missing_tool_result",
        "invalid_label",
        "nested_tool_block",
        "text_too_large",
        "too_many_items",
    ] {
        let matched = text.contains(code)
            || match code {
                "role_block_mismatch" => text.contains("does not allow content kind"),
                "duplicate_tool_association" => text.contains("duplicate tool_call_id"),
                "unknown_tool_association" => text.contains("unknown tool_call_id"),
                "missing_tool_result" => text.contains("requires at least one tool_result"),
                "invalid_label" => text.contains("empty, oversized, or contains NUL"),
                "nested_tool_block" => text.contains("cannot nest tool_call"),
                "text_too_large" => text.contains("text length"),
                "too_many_items" => text.contains("content item count"),
                _ => false,
            };
        if matched {
            return code;
        }
    }
    classify_parse_error(error)
}

fn assert_error_code(expect: &Expect, actual: &str) -> Result<(), PublicApiFixtureError> {
    if expect.ok {
        return Err(fail(format!("expected success, got error {actual}")));
    }
    let Some(expected) = expect.error_code.as_deref() else {
        return Err(fail("failed case requires expect.error_code"));
    };
    if expected != actual {
        return Err(fail(format!(
            "error_code mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn fail(message: impl Into<String>) -> PublicApiFixtureError {
    PublicApiFixtureError::Failed(message.into())
}
