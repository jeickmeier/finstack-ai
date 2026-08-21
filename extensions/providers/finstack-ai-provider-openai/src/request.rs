//! Private official `OpenAI` Responses request translation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use base64::Engine;
use finstack_ai_kernel::{
    ContentBlock, MediaRef, Message, MessageRole, OutputSpec, RawJson, SUBMIT_FINAL_OUTPUT_TOOL,
    ToolCallId,
};
use finstack_ai_runtime::{ModelError, ModelRequestDraft, ResolvedMedia};
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};

use crate::OpenAiModelConfig;
use crate::error::request_error;

const CONTINUATION_PROVIDER: &str = "openai.responses";
const CONTINUATION_VERSION: u64 = 1;
const RESERVED_SETTINGS: &[&str] = &[
    "model",
    "input",
    "instructions",
    "tools",
    "stream",
    "store",
    "max_output_tokens",
    "previous_response_id",
    "reasoning",
    "text",
];
const REASONING_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
const REASONING_SUMMARIES: &[&str] = &["auto", "concise", "detailed"];

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<Value>,
    stream: bool,
    store: bool,
    max_output_tokens: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,
    #[serde(flatten)]
    settings: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    description: String,
    parameters: Value,
    strict: bool,
}

#[derive(Deserialize)]
struct ContinuationEnvelope {
    provider: String,
    version: u64,
    replay_items: Vec<Value>,
}

impl ResponsesRequest {
    pub(crate) fn try_from_draft(
        draft: &ModelRequestDraft,
        model: &OpenAiModelConfig,
        continuation_state: Option<&RawJson>,
        resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    ) -> Result<Self, ModelError> {
        draft.validate()?;
        if draft.model != model.name {
            return Err(request_error("requested model is not configured"));
        }
        let (instructions, input) =
            map_input(&draft.messages, continuation_state, resolved, model)?;
        let mut settings = parse_settings(&draft.settings.values)?;
        for reserved in RESERVED_SETTINGS {
            if settings.remove(*reserved).is_some() {
                return Err(request_error("provider settings contain a reserved field"));
            }
        }
        let reasoning_effort =
            take_string_setting(&mut settings, "reasoning_effort", REASONING_EFFORTS)?;
        let reasoning_summary =
            take_string_setting(&mut settings, "reasoning_summary", REASONING_SUMMARIES)?;
        let reasoning = match (reasoning_effort, reasoning_summary) {
            (None, None) => None,
            (Some(effort), None) => Some(json!({ "effort": effort })),
            (None, Some(summary)) => Some(json!({ "summary": summary })),
            (Some(effort), Some(summary)) => Some(json!({ "effort": effort, "summary": summary })),
        };

        let mut text = None;
        let mut tools = Vec::with_capacity(draft.tools.len());
        for tool in draft.tools.iter() {
            if tool.model_name.as_ref() == SUBMIT_FINAL_OUTPUT_TOOL
                && let OutputSpec::JsonSchema { schema } = &draft.output
            {
                if tool.input_schema.digest() != schema.schema_digest {
                    return Err(request_error(
                        "structured-output schema does not match the committed schema reference",
                    ));
                }
                let schema_value = raw_value(&tool.input_schema)?;
                text = Some(json!({
                    "format": {
                        "type": "json_schema",
                        "name": "finstack_final_output",
                        "strict": true,
                        "schema": schema_value
                    }
                }));
                continue;
            }
            let parameters = raw_value(&tool.input_schema)?;
            tools.push(WireTool {
                kind: "function",
                name: tool.model_name.to_string(),
                description: tool.description.to_string(),
                strict: supports_strict(&parameters),
                parameters,
            });
        }
        if matches!(draft.output, OutputSpec::JsonSchema { .. }) && text.is_none() {
            return Err(request_error(
                "structured output requires the matching framework schema tool",
            ));
        }

        Ok(Self {
            model: draft.model.as_str().to_owned(),
            instructions,
            input,
            stream: true,
            store: false,
            max_output_tokens: draft.limits.max_output_tokens.min(model.max_output_tokens),
            parallel_tool_calls: (!tools.is_empty()).then_some(model.parallel_tool_calls),
            tools,
            text,
            reasoning,
            settings,
        })
    }
}

/// Whether `schema` satisfies the Responses `strict` function-calling subset.
///
/// `strict` is a claim about the schema rather than a preference: the endpoint
/// rejects the *entire* request when any one tool violates the subset, so
/// asserting it unconditionally lets a single non-conforming tool fail every
/// run. Tools that cannot satisfy it — free-form objects, or a `required`
/// array that omits declared properties — are sent unstrict instead.
fn supports_strict(schema: &Value) -> bool {
    match schema {
        Value::Object(node) => {
            if declares_object(node) && !object_node_is_strict(node) {
                return false;
            }
            node.iter()
                .filter(|(key, _)| key.as_str() != "required")
                .all(|(_, value)| supports_strict(value))
        }
        Value::Array(items) => items.iter().all(supports_strict),
        _ => true,
    }
}

fn declares_object(node: &serde_json::Map<String, Value>) -> bool {
    match node.get("type") {
        Some(Value::String(name)) => name == "object",
        Some(Value::Array(names)) => names.iter().any(|name| name.as_str() == Some("object")),
        _ => false,
    }
}

fn object_node_is_strict(node: &serde_json::Map<String, Value>) -> bool {
    if node.get("additionalProperties") != Some(&Value::Bool(false)) {
        return false;
    }
    let Some(Value::Object(properties)) = node.get("properties") else {
        return false;
    };
    let required: BTreeSet<&str> = node
        .get("required")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    properties.keys().all(|key| required.contains(key.as_str()))
}

fn parse_settings(settings: &RawJson) -> Result<BTreeMap<String, Value>, ModelError> {
    let value = raw_value(settings)?;
    let Value::Object(values) = value else {
        return Err(request_error("provider settings must be a JSON object"));
    };
    Ok(values.into_iter().collect())
}

fn take_string_setting(
    settings: &mut BTreeMap<String, Value>,
    key: &str,
    allowed: &[&str],
) -> Result<Option<String>, ModelError> {
    let Some(value) = settings.remove(key) else {
        return Ok(None);
    };
    let Value::String(value) = value else {
        return Err(request_error("provider reasoning setting must be a string"));
    };
    if !allowed.contains(&value.as_str()) {
        return Err(request_error("provider reasoning setting is unsupported"));
    }
    Ok(Some(value))
}

fn raw_value(value: &RawJson) -> Result<Value, ModelError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| request_error("canonical provider JSON could not be decoded"))
}

fn map_input(
    messages: &[Message],
    continuation_state: Option<&RawJson>,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &OpenAiModelConfig,
) -> Result<(Option<String>, Vec<Value>), ModelError> {
    let prefix_len = messages
        .iter()
        .take_while(|message| {
            matches!(message.role(), MessageRole::System | MessageRole::Developer)
        })
        .count();
    if messages[prefix_len..]
        .iter()
        .any(|message| matches!(message.role(), MessageRole::System | MessageRole::Developer))
    {
        return Err(request_error(
            "system instructions must remain a stable leading prefix",
        ));
    }
    let instructions = concatenate_instructions(&messages[..prefix_len])?;
    let call_ids = provider_call_ids(messages);
    let conversation = if let Some(state) = continuation_state {
        let envelope = parse_continuation(state)?;
        let suffix_start = messages
            .iter()
            .rposition(|message| message.role() == MessageRole::Assistant)
            .map_or(prefix_len, |index| index.saturating_add(1));
        let mut input = envelope.replay_items;
        for message in &messages[suffix_start..] {
            input.extend(map_conversation_message(
                message, &call_ids, resolved, model,
            )?);
        }
        input
    } else {
        let mut input = Vec::new();
        for message in &messages[prefix_len..] {
            input.extend(map_conversation_message(
                message, &call_ids, resolved, model,
            )?);
        }
        input
    };
    Ok((instructions, conversation))
}

fn parse_continuation(state: &RawJson) -> Result<ContinuationEnvelope, ModelError> {
    let envelope: ContinuationEnvelope = serde_json::from_slice(state.as_bytes())
        .map_err(|_| request_error("continuation state is invalid"))?;
    if envelope.provider != CONTINUATION_PROVIDER || envelope.version != CONTINUATION_VERSION {
        return Err(request_error("continuation state is invalid"));
    }
    Ok(envelope)
}

fn concatenate_instructions(messages: &[Message]) -> Result<Option<String>, ModelError> {
    let mut parts = Vec::new();
    for message in messages {
        let text = render_text(message.content())?;
        if !text.is_empty() {
            parts.push(text);
        }
    }
    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join("\n\n")))
    }
}

fn provider_call_ids(messages: &[Message]) -> BTreeMap<ToolCallId, String> {
    let mut ids = BTreeMap::new();
    for message in messages {
        for block in message.content() {
            if let ContentBlock::ToolCall(call) = block {
                let call_id = call
                    .provider_call_id()
                    .map_or_else(|| call.tool_call_id().to_string(), str::to_owned);
                ids.insert(*call.tool_call_id(), call_id);
            }
        }
    }
    ids
}

fn map_conversation_message(
    message: &Message,
    call_ids: &BTreeMap<ToolCallId, String>,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &OpenAiModelConfig,
) -> Result<Vec<Value>, ModelError> {
    match message.role() {
        MessageRole::User => Ok(vec![json!({
            "type": "message",
            "role": "user",
            "content": map_user_content(message.content(), resolved, model)?
        })]),
        MessageRole::Assistant => map_assistant_items(message.content(), call_ids),
        MessageRole::Tool => map_tool_results(message.content(), call_ids),
        MessageRole::System | MessageRole::Developer => Err(request_error(
            "system instructions must remain a stable leading prefix",
        )),
    }
}

fn map_user_content(
    content: &[ContentBlock],
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &OpenAiModelConfig,
) -> Result<Vec<Value>, ModelError> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let flush = |parts: &mut Vec<Value>, text: &mut String| {
        if !text.is_empty() {
            parts.push(json!({ "type": "input_text", "text": text.as_str() }));
            text.clear();
        }
    };
    for block in content {
        match block {
            ContentBlock::Text(value) => text.push_str(value.text()),
            ContentBlock::Json(value) => text.push_str(value.value().as_str()),
            ContentBlock::Image(media) => {
                if !model.input_images {
                    return Err(request_error("model is not configured for image input"));
                }
                flush(&mut parts, &mut text);
                parts.push(json!({
                    "type": "input_image",
                    "image_url": media_reference(media, resolved)?
                }));
            }
            ContentBlock::File(media) => {
                if !model.input_files {
                    return Err(request_error("model is not configured for file input"));
                }
                flush(&mut parts, &mut text);
                parts.push(json!({
                    "type": "input_file",
                    "file_url": media_reference(media, resolved)?
                }));
            }
            ContentBlock::Audio(media) => {
                if !model.input_audio {
                    return Err(request_error("model is not configured for audio input"));
                }
                flush(&mut parts, &mut text);
                let (data, format) = media_reference_audio(media, resolved)?;
                parts.push(json!({
                    "type": "input_audio",
                    "input_audio": { "data": data, "format": format }
                }));
            }
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    flush(&mut parts, &mut text);
    Ok(parts)
}

fn media_reference(
    media: &MediaRef,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
) -> Result<String, ModelError> {
    match resolved.get(media.blob().id()) {
        Some(ResolvedMedia::Url(url)) => Ok(url.to_string()),
        Some(ResolvedMedia::Bytes { media_type, bytes }) => Ok(format!(
            "data:{media_type};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )),
        None => Err(request_error(
            "media content requires a configured media resolver",
        )),
    }
}

fn media_reference_audio(
    media: &MediaRef,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
) -> Result<(String, String), ModelError> {
    match resolved.get(media.blob().id()) {
        Some(ResolvedMedia::Bytes { media_type, bytes }) => {
            let format = media_type.rsplit('/').next().unwrap_or("mp3").to_owned();
            Ok((
                base64::engine::general_purpose::STANDARD.encode(bytes),
                format,
            ))
        }
        Some(ResolvedMedia::Url(_)) => Err(request_error(
            "audio input requires resolved bytes, not a URL",
        )),
        None => Err(request_error(
            "media content requires a configured media resolver",
        )),
    }
}

fn map_assistant_items(
    content: &[ContentBlock],
    call_ids: &BTreeMap<ToolCallId, String>,
) -> Result<Vec<Value>, ModelError> {
    let mut items = Vec::new();
    let mut text = String::new();
    for block in content {
        match block {
            ContentBlock::Text(value) => text.push_str(value.text()),
            ContentBlock::Json(value) => text.push_str(value.value().as_str()),
            ContentBlock::ToolCall(call) => {
                flush_assistant_text(&mut items, &mut text);
                let call_id = call_ids
                    .get(call.tool_call_id())
                    .cloned()
                    .unwrap_or_else(|| call.tool_call_id().to_string());
                items.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": call.tool_name(),
                    "arguments": call.arguments().as_str()
                }));
            }
            ContentBlock::Opaque(_) => {}
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    flush_assistant_text(&mut items, &mut text);
    Ok(items)
}

fn flush_assistant_text(items: &mut Vec<Value>, text: &mut String) {
    if text.is_empty() {
        return;
    }
    items.push(json!({
        "type": "message",
        "role": "assistant",
        "content": [{ "type": "output_text", "text": text.as_str() }]
    }));
    text.clear();
}

fn map_tool_results(
    content: &[ContentBlock],
    call_ids: &BTreeMap<ToolCallId, String>,
) -> Result<Vec<Value>, ModelError> {
    content
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult(result) => {
                let call_id = call_ids
                    .get(result.tool_call_id())
                    .cloned()
                    .unwrap_or_else(|| result.tool_call_id().to_string());
                Ok(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": render_text(result.content())?
                }))
            }
            _ => Err(request_error("tool messages contain unsupported content")),
        })
        .collect()
}

fn render_text(content: &[ContentBlock]) -> Result<String, ModelError> {
    let mut rendered = String::new();
    for block in content {
        match block {
            ContentBlock::Text(value) => rendered.push_str(value.text()),
            ContentBlock::Json(value) => rendered.push_str(value.value().as_str()),
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    Ok(rendered)
}

pub(crate) fn serialize_request(request: &ResponsesRequest) -> Result<Vec<u8>, ModelError> {
    serde_json::to_vec(request).map_err(|_| request_error("provider request could not be encoded"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use finstack_ai_kernel::{
        MessageId, Metadata, ProviderIds, RetrySafety, TextBlock, Timestamp, ToolCallBlock,
        ToolExecutionMode, ToolId, ToolResultBlock,
    };
    use finstack_ai_runtime::{
        ApprovalMetadata, ApprovalRequirement, ModelName, ModelRequestLimits, ModelSettings,
        SideEffectClass, ToolSpec,
    };

    #[test]
    fn encodes_responses_fields_flattened_tools_and_reasoning() {
        let tools = Arc::from([tool("lookup", STRICT_SCHEMA)]);
        let mut draft =
            draft(br#"{"reasoning_effort":"low","reasoning_summary":"auto","temperature":0}"#);
        draft.tools = tools;
        let request = ResponsesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["store"], false);
        assert_eq!(value["stream"], true);
        assert_eq!(value["max_output_tokens"], 1_024);
        assert!(value.get("previous_response_id").is_none());
        assert_eq!(value["input"][0]["type"], "message");
        assert_eq!(value["input"][0]["role"], "user");
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(value["tools"][0]["name"], "lookup");
        assert_eq!(value["tools"][0]["strict"], true);
        assert!(value["tools"][0].get("function").is_none());
        assert_eq!(value["reasoning"]["effort"], "low");
        assert_eq!(value["reasoning"]["summary"], "auto");
        assert_eq!(value["temperature"], 0);
        assert!(value.get("reasoning_effort").is_none());
        assert!(value.get("reasoning_summary").is_none());
    }

    /// A tool whose schema cannot meet the strict subset must not poison the
    /// whole request: the endpoint rejects every tool when one is mislabelled.
    #[test]
    fn tools_outside_the_strict_subset_are_sent_unstrict() {
        let mut draft = draft(b"{}");
        draft.tools = Arc::from([
            tool("conforming", STRICT_SCHEMA),
            tool("free_form_object", br#"{"type":"object"}"#),
            tool(
                "partial_required",
                br#"{"additionalProperties":false,"properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a"],"type":"object"}"#,
            ),
            tool(
                "nested_free_form",
                br#"{"additionalProperties":false,"properties":{"payload":{"type":"object"}},"required":["payload"],"type":"object"}"#,
            ),
        ]);
        let request = ResponsesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["tools"][0]["strict"], true);
        assert_eq!(value["tools"][1]["strict"], false);
        assert_eq!(value["tools"][2]["strict"], false);
        assert_eq!(value["tools"][3]["strict"], false);
    }

    #[test]
    fn nullable_optional_properties_stay_strict() {
        let mut draft = draft(b"{}");
        draft.tools = Arc::from([tool(
            "lookup",
            br#"{"additionalProperties":false,"properties":{"limit":{"type":["integer","null"]},"query":{"type":"string"}},"required":["query","limit"],"type":"object"}"#,
        )]);
        let request = ResponsesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["tools"][0]["strict"], true);
    }

    #[test]
    fn prompt_cache_key_is_passed_through_unchanged() {
        let mut draft = draft(br#"{"prompt_cache_key":"stale-previous-tools"}"#);
        draft.tools = Arc::from([tool("lookup", STRICT_SCHEMA)]);
        let request = ResponsesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["prompt_cache_key"], "stale-previous-tools");
    }

    #[test]
    fn rejects_reserved_settings() {
        let draft = draft(br#"{"model":"shadow"}"#);
        let error = ResponsesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect_err("reserved setting must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn rejects_unsupported_reasoning_settings() {
        for settings in [
            br#"{"reasoning_effort":"turbo"}"#.as_slice(),
            br#"{"reasoning_summary":"verbose"}"#.as_slice(),
            br#"{"reasoning_effort":1}"#.as_slice(),
        ] {
            let error = ResponsesRequest::try_from_draft(
                &draft(settings),
                &model(),
                None,
                &BTreeMap::new(),
            )
            .expect_err("unsupported reasoning must fail");
            assert_eq!(error.code(), crate::error::REQUEST_INVALID);
        }
    }

    #[test]
    fn continuation_replays_output_items_and_appends_new_tool_results() {
        let tool_call_id =
            finstack_ai_kernel::ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789ac")
                .expect("tool call id");
        let assistant = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("message id"),
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(
                ToolCallBlock::try_new_with_provider_call_id(
                    tool_call_id,
                    "lookup",
                    RawJson::parse(br#"{"q":1}"#).expect("args"),
                    Some("call_abc"),
                )
                .expect("call"),
            )],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("assistant");
        let tool = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("message id"),
            MessageRole::Tool,
            vec![ContentBlock::ToolResult(
                ToolResultBlock::try_new(
                    tool_call_id,
                    vec![ContentBlock::Text(TextBlock::try_new("ok").expect("text"))],
                    false,
                )
                .expect("result"),
            )],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("tool");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([
            text_message(MessageRole::System, "Be brief."),
            text_message(MessageRole::User, "hello"),
            assistant,
            tool,
        ]);
        let continuation = RawJson::parse(
            br#"{"provider":"openai.responses","replay_items":[{"call_id":"call_abc","type":"function_call"},{"encrypted_content":"secret-reasoning","type":"reasoning"}],"version":1}"#,
        )
        .expect("continuation");
        let request = ResponsesRequest::try_from_draft(
            &draft,
            &model(),
            Some(&continuation),
            &BTreeMap::new(),
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["instructions"], "Be brief.");
        assert_eq!(value["input"].as_array().expect("input").len(), 3);
        assert_eq!(value["input"][0]["type"], "function_call");
        assert_eq!(value["input"][1]["type"], "reasoning");
        assert_eq!(value["input"][2]["type"], "function_call_output");
        assert_eq!(value["input"][2]["call_id"], "call_abc");
        assert_eq!(value["input"][2]["output"], "ok");
        assert_ne!(value["input"][0]["type"], "message");
    }

    #[test]
    fn image_blocks_map_to_input_image_items() {
        use finstack_ai_kernel::BlobRef;

        let blob = BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>).expect("blob");
        let media = MediaRef::new(blob);
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::Image(media)],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([message]);

        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );

        let request = ResponsesRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &resolved,
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["input"][0]["content"][0]["type"], "input_image");
        assert_eq!(
            value["input"][0]["content"][0]["image_url"],
            "https://cdn.example/a.png"
        );
    }

    #[test]
    fn image_input_is_rejected_when_the_model_flag_is_off() {
        use finstack_ai_kernel::BlobRef;

        let blob = BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>).expect("blob");
        let media = MediaRef::new(blob);
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::Image(media)],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([message]);
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );
        let error = ResponsesRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("image input must be rejected when the flag is off");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn file_input_is_rejected_when_the_model_flag_is_off() {
        use finstack_ai_kernel::BlobRef;

        let blob =
            BlobRef::try_new("blob-1", "application/pdf", 4, None, None::<&str>).expect("blob");
        let media = MediaRef::new(blob);
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::File(media)],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([message]);
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.pdf")),
        );
        let error = ResponsesRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("file input must be rejected when the flag is off");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn audio_input_is_rejected_when_the_model_flag_is_off() {
        use finstack_ai_kernel::BlobRef;

        let blob = BlobRef::try_new("blob-1", "audio/wav", 4, None, None::<&str>).expect("blob");
        let media = MediaRef::new(blob);
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::Audio(media)],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([message]);
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Bytes {
                media_type: Arc::from("audio/wav"),
                bytes: Arc::from(b"abcd".as_slice()),
            },
        );
        let error = ResponsesRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("audio input must be rejected when the flag is off");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn media_without_resolver_fails_closed() {
        use finstack_ai_kernel::BlobRef;

        let blob = BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>).expect("blob");
        let media = MediaRef::new(blob);
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::Image(media)],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([message]);

        let error = ResponsesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect_err("media without resolution must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn audio_url_resolution_is_rejected() {
        use finstack_ai_kernel::BlobRef;

        let blob = BlobRef::try_new("blob-1", "audio/mpeg", 4, None, None::<&str>).expect("blob");
        let media = MediaRef::new(blob);
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::Audio(media)],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([message]);

        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.mp3")),
        );

        let error = ResponsesRequest::try_from_draft(
            &draft,
            &model().with_input_audio(true),
            None,
            &resolved,
        )
        .expect_err("audio URL resolution must be rejected");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn audio_bytes_map_to_input_audio_with_base64_data_and_format() {
        use finstack_ai_kernel::BlobRef;

        let blob = BlobRef::try_new("blob-1", "audio/wav", 4, None, None::<&str>).expect("blob");
        let media = MediaRef::new(blob);
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::Audio(media)],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([message]);

        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Bytes {
                media_type: Arc::from("audio/wav"),
                bytes: Arc::from(b"abcd".as_slice()),
            },
        );

        let request = ResponsesRequest::try_from_draft(
            &draft,
            &model().with_input_audio(true),
            None,
            &resolved,
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["input"][0]["content"][0]["type"], "input_audio");
        assert_eq!(
            value["input"][0]["content"][0]["input_audio"]["data"],
            base64::engine::general_purpose::STANDARD.encode(b"abcd")
        );
        assert_eq!(
            value["input"][0]["content"][0]["input_audio"]["format"],
            "wav"
        );
    }

    fn model() -> OpenAiModelConfig {
        OpenAiModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
            .expect("model")
    }

    fn text_message(role: MessageRole, text: &str) -> Message {
        Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            role,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    const STRICT_SCHEMA: &[u8] =
        br#"{"additionalProperties":false,"properties":{"query":{"type":"string"}},"required":["query"],"type":"object"}"#;

    fn tool(name: &str, schema: &[u8]) -> ToolSpec {
        ToolSpec {
            id: ToolId::parse("finstack.tools.fixture").expect("tool id"),
            model_name: Arc::from(name),
            title: Arc::from("Fixture tool"),
            description: Arc::from("A deterministic fixture tool."),
            input_schema: RawJson::parse(schema).expect("schema"),
            output_schema: None,
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 1_024,
            metadata: Metadata::empty(),
            deferral: finstack_ai_runtime::ToolDeferralSupport::Never,
        }
    }

    fn draft(settings: &[u8]) -> ModelRequestDraft {
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from([text_message(MessageRole::User, "hello")]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(settings).expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        }
    }
}
