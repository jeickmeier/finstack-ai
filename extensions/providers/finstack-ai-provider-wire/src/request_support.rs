//! Request-mapping helpers identical across the first-party provider leaves.
//!
//! Every helper takes the leaf's [`ProviderErrorCodes`] so a failure carries
//! that leaf's stable `request_invalid` code under
//! [`ErrorCategory::Validation`](finstack_ai_kernel::ErrorCategory::Validation).

use std::collections::BTreeMap;

use finstack_ai_kernel::{ContentBlock, Message, MessageRole, RawJson};
use finstack_ai_runtime::ports::model::ModelError;
use serde_json::Value;

use crate::adapter_error::{ProviderErrorCodes, request_error};

/// Decode canonical provider JSON into a [`Value`].
///
/// # Errors
///
/// Returns the leaf's `request_invalid` error when the bytes do not decode.
pub fn raw_value(codes: &ProviderErrorCodes, value: &RawJson) -> Result<Value, ModelError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| request_error(codes, "canonical provider JSON could not be decoded"))
}

/// Decode provider settings into an owned member map.
///
/// # Errors
///
/// Returns the leaf's `request_invalid` error when the settings are not a
/// JSON object.
pub fn parse_settings(
    codes: &ProviderErrorCodes,
    settings: &RawJson,
) -> Result<BTreeMap<String, Value>, ModelError> {
    let Value::Object(values) = raw_value(codes, settings)? else {
        return Err(request_error(
            codes,
            "provider settings must be a JSON object",
        ));
    };
    Ok(values.into_iter().collect())
}

/// Render text and JSON blocks to one string.
///
/// # Errors
///
/// Returns the leaf's `request_invalid` error when any other block kind is
/// present.
pub fn render_text(
    codes: &ProviderErrorCodes,
    content: &[ContentBlock],
) -> Result<String, ModelError> {
    let mut rendered = String::new();
    for block in content {
        match block {
            ContentBlock::Text(value) => rendered.push_str(value.text()),
            ContentBlock::Json(value) => rendered.push_str(value.value().as_str()),
            _ => {
                return Err(request_error(
                    codes,
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    Ok(rendered)
}

/// Length of the leading system/developer prefix.
///
/// # Errors
///
/// Returns the leaf's `request_invalid` error when a system or developer
/// message appears after the first conversational message.
pub fn system_prefix_len(
    codes: &ProviderErrorCodes,
    messages: &[Message],
) -> Result<usize, ModelError> {
    let is_system =
        |message: &Message| matches!(message.role(), MessageRole::System | MessageRole::Developer);
    let prefix_len = messages
        .iter()
        .take_while(|message| is_system(message))
        .count();
    if messages
        .get(prefix_len..)
        .is_some_and(|rest| rest.iter().any(is_system))
    {
        return Err(request_error(
            codes,
            "system instructions must remain a stable leading prefix",
        ));
    }
    Ok(prefix_len)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::{
        ErrorCategory, MessageId, Metadata, ProviderIds, TextBlock, Timestamp,
    };

    use super::*;

    const CODES: ProviderErrorCodes = ProviderErrorCodes {
        config_invalid: "test_config_invalid",
        request_invalid: "test_request_invalid",
        http_error: "test_http_error",
        transport_error: "test_transport_error",
        timeout: "test_timeout",
        cancelled: "test_cancelled",
        stream_invalid: "test_stream_invalid",
        stream_limit_exceeded: "test_stream_limit_exceeded",
        response_invalid: "test_response_invalid",
    };

    fn message(role: MessageRole, index: u8) -> Message {
        Message::try_new(
            MessageId::parse(&format!("01234567-89ab-7cde-89ab-0123456789a{index}")).unwrap(),
            role,
            vec![ContentBlock::Text(TextBlock::try_new("x").unwrap())],
            Timestamp::from_unix_ms(1).unwrap(),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .unwrap()
    }

    #[test]
    fn settings_must_be_an_object() {
        let settings = RawJson::parse(br#"{"b":1,"a":"x"}"#).unwrap();
        let parsed = parse_settings(&CODES, &settings).unwrap();
        assert_eq!(parsed.keys().collect::<Vec<_>>(), vec!["a", "b"]);

        let error = parse_settings(&CODES, &RawJson::parse(b"[1]").unwrap()).unwrap_err();
        assert_eq!(error.code().as_str(), "test_request_invalid");
        assert_eq!(error.category(), ErrorCategory::Validation);
    }

    #[test]
    fn render_text_concatenates_text_and_json_only() {
        let blocks = vec![
            ContentBlock::Text(TextBlock::try_new("a").unwrap()),
            ContentBlock::Json(finstack_ai_kernel::JsonBlock::new(
                RawJson::parse(br#"{"k":1}"#).unwrap(),
            )),
        ];
        assert_eq!(render_text(&CODES, &blocks).unwrap(), r#"a{"k":1}"#);
        let blob =
            finstack_ai_kernel::BlobRef::try_new("b", "image/png", 1, None, None::<&str>).unwrap();
        let media = vec![ContentBlock::Image(finstack_ai_kernel::MediaRef::new(blob))];
        assert_eq!(
            render_text(&CODES, &media).unwrap_err().code().as_str(),
            "test_request_invalid"
        );
    }

    #[test]
    fn system_prefix_is_leading_only() {
        let messages: Arc<[Message]> = Arc::from([
            message(MessageRole::System, 1),
            message(MessageRole::Developer, 2),
            message(MessageRole::User, 3),
        ]);
        assert_eq!(system_prefix_len(&CODES, &messages).unwrap(), 2);
        let late: Arc<[Message]> = Arc::from([
            message(MessageRole::User, 1),
            message(MessageRole::System, 2),
        ]);
        let error = system_prefix_len(&CODES, &late).unwrap_err();
        assert_eq!(error.code().as_str(), "test_request_invalid");
        assert_eq!(
            error.message(),
            "system instructions must remain a stable leading prefix"
        );
        assert_eq!(system_prefix_len(&CODES, &[]).unwrap(), 0);
    }
}
