//! Private Ollama `/api/chat` request translation.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use finstack_ai_kernel::{
    ContentBlock, Message, MessageRole, OutputSpec, RawJson, SUBMIT_FINAL_OUTPUT_TOOL,
};
use finstack_ai_runtime::{ModelError, ModelRequestDraft, ResolvedMedia};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::OllamaModelConfig;
use crate::error::request_error;

const RESERVED_SETTINGS: &[&str] = &[
    "model", "messages", "tools", "stream", "format", "think", "options",
];
const CONTINUATION_PROVIDER: &str = "ollama.api_chat";
const CONTINUATION_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub(crate) struct ChatRequest {
    model: String,
    messages: Vec<WireMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
    options: ChatOptions,
    #[serde(flatten)]
    settings: BTreeMap<String, Value>,
}

#[derive(Debug)]
pub(crate) struct PreparedChat {
    pub(crate) request: ChatRequest,
    pub(crate) matched_replay: Option<Vec<ReplayEntry>>,
}

#[derive(Debug, Serialize)]
struct ChatOptions {
    num_predict: u64,
}

#[derive(Debug, Serialize)]
struct WireMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<WireToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
}

#[derive(Debug, Serialize)]
struct WireToolCall {
    function: WireFunctionCall,
}

#[derive(Debug, Serialize)]
struct WireFunctionCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunction,
}

#[derive(Debug, Serialize)]
struct WireFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ContinuationEnvelope {
    provider: String,
    version: u32,
    assistant_replay: Vec<ReplayEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ReplayEntry {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) thinking: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) digest: Option<String>,
}

impl ChatRequest {
    pub(crate) fn try_from_draft(
        draft: &ModelRequestDraft,
        model: &OllamaModelConfig,
        continuation: Option<&RawJson>,
        resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    ) -> Result<PreparedChat, ModelError> {
        draft.validate()?;
        if draft.model != model.name {
            return Err(request_error("requested model is not configured"));
        }
        let matched_replay = resolve_replay(&draft.messages, continuation);
        let messages = map_messages(&draft.messages, matched_replay.as_deref(), resolved, model)?;
        let mut settings = parse_settings(&draft.settings.values)?;
        for reserved in RESERVED_SETTINGS {
            if settings.remove(*reserved).is_some() {
                return Err(request_error("provider settings contain a reserved field"));
            }
        }

        let mut format = None;
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
                format = Some(raw_value(&tool.input_schema)?);
                continue;
            }
            tools.push(WireTool {
                kind: "function",
                function: WireFunction {
                    name: tool.model_name.to_string(),
                    description: tool.description.to_string(),
                    parameters: raw_value(&tool.input_schema)?,
                },
            });
        }
        if matches!(draft.output, OutputSpec::JsonSchema { .. }) && format.is_none() {
            return Err(request_error(
                "structured output requires the matching framework schema tool",
            ));
        }

        Ok(PreparedChat {
            request: Self {
                model: draft.model.as_str().to_owned(),
                messages,
                stream: true,
                tools,
                format,
                think: model.reasoning.then_some(true),
                options: ChatOptions {
                    num_predict: draft.limits.max_output_tokens.min(model.max_output_tokens),
                },
                settings,
            },
            matched_replay,
        })
    }
}

fn parse_settings(settings: &RawJson) -> Result<BTreeMap<String, Value>, ModelError> {
    let value = raw_value(settings)?;
    let Value::Object(values) = value else {
        return Err(request_error("provider settings must be a JSON object"));
    };
    Ok(values.into_iter().collect())
}

fn raw_value(value: &RawJson) -> Result<Value, ModelError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| request_error("canonical provider JSON could not be decoded"))
}

fn resolve_replay(
    messages: &[Message],
    continuation: Option<&RawJson>,
) -> Option<Vec<ReplayEntry>> {
    let continuation = continuation?;
    let envelope: ContinuationEnvelope = serde_json::from_slice(continuation.as_bytes()).ok()?;
    if envelope.provider != CONTINUATION_PROVIDER || envelope.version != CONTINUATION_VERSION {
        return None;
    }
    let assistants = messages
        .iter()
        .filter(|message| message.role() == MessageRole::Assistant)
        .collect::<Vec<_>>();
    if envelope.assistant_replay.len() != assistants.len() {
        return None;
    }
    for (entry, message) in envelope.assistant_replay.iter().zip(assistants) {
        let digest = assistant_message_digest(message).ok()?;
        if entry
            .digest
            .as_deref()
            .is_some_and(|expected| expected != digest)
        {
            return None;
        }
    }
    Some(envelope.assistant_replay)
}

fn map_messages(
    messages: &[Message],
    replay: Option<&[ReplayEntry]>,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &OllamaModelConfig,
) -> Result<Vec<WireMessage>, ModelError> {
    let mut mapped = Vec::new();
    let mut assistant_index = 0_usize;
    let tool_names = index_tool_names(messages);
    for message in messages {
        if message.role() == MessageRole::Tool {
            mapped.extend(map_tool_results(&tool_names, message)?);
            continue;
        }
        let thinking = if message.role() == MessageRole::Assistant {
            let thinking = replay
                .and_then(|entries| entries.get(assistant_index))
                .map(|entry| entry.thinking.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            assistant_index = assistant_index.saturating_add(1);
            thinking
        } else {
            None
        };
        mapped.push(map_conversation_message(
            message, thinking, resolved, model,
        )?);
    }
    Ok(mapped)
}

fn map_conversation_message(
    message: &Message,
    thinking: Option<String>,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &OllamaModelConfig,
) -> Result<WireMessage, ModelError> {
    let role = match message.role() {
        MessageRole::System | MessageRole::Developer => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => {
            return Err(request_error(
                "tool messages must be mapped as native tool results",
            ));
        }
    };
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut images = Vec::new();
    for block in message.content() {
        match block {
            ContentBlock::Text(value) => text.push_str(value.text()),
            ContentBlock::Json(value) => text.push_str(value.value().as_str()),
            ContentBlock::ToolCall(call) if message.role() == MessageRole::Assistant => {
                tool_calls.push(WireToolCall {
                    function: WireFunctionCall {
                        name: call.tool_name().to_owned(),
                        arguments: raw_value(call.arguments())?,
                    },
                });
            }
            ContentBlock::Image(media) if message.role() == MessageRole::User => {
                if !model.input_images {
                    return Err(request_error("model is not configured for image input"));
                }
                images.push(resolved_image(media, resolved)?);
            }
            ContentBlock::Opaque(_) => {}
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    Ok(WireMessage {
        role,
        content: (!text.is_empty()).then_some(text),
        thinking,
        tool_calls,
        tool_name: None,
        images,
    })
}

fn resolved_image(
    media: &finstack_ai_kernel::MediaRef,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
) -> Result<String, ModelError> {
    match resolved.get(media.blob().id()) {
        Some(ResolvedMedia::Bytes { bytes, .. }) => {
            Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
        Some(ResolvedMedia::Url(_)) => Err(request_error(
            "ollama image input requires resolved bytes, not a URL",
        )),
        None => Err(request_error(
            "media content requires a configured media resolver",
        )),
    }
}

fn map_tool_results(
    tool_names: &BTreeMap<finstack_ai_kernel::ToolCallId, String>,
    message: &Message,
) -> Result<Vec<WireMessage>, ModelError> {
    message
        .content()
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult(result) => Ok(WireMessage {
                role: "tool",
                content: Some(render_text(result.content())?),
                thinking: None,
                tool_calls: Vec::new(),
                tool_name: Some(tool_name_for(tool_names, result.tool_call_id())?),
                images: Vec::new(),
            }),
            _ => Err(request_error("tool messages contain unsupported content")),
        })
        .collect()
}

fn tool_name_for(
    tool_names: &BTreeMap<finstack_ai_kernel::ToolCallId, String>,
    tool_call_id: &finstack_ai_kernel::ToolCallId,
) -> Result<String, ModelError> {
    tool_names
        .get(tool_call_id)
        .cloned()
        .ok_or_else(|| request_error("tool result has no matching assistant tool call"))
}

fn index_tool_names(messages: &[Message]) -> BTreeMap<finstack_ai_kernel::ToolCallId, String> {
    let mut tool_names = BTreeMap::new();
    for message in messages {
        if message.role() != MessageRole::Assistant {
            continue;
        }
        for block in message.content() {
            if let ContentBlock::ToolCall(call) = block {
                tool_names
                    .entry(*call.tool_call_id())
                    .or_insert_with(|| call.tool_name().to_owned());
            }
        }
    }
    tool_names
}

fn render_text(content: &[ContentBlock]) -> Result<String, ModelError> {
    let mut rendered = String::new();
    for block in content {
        match block {
            ContentBlock::Text(value) => rendered.push_str(value.text()),
            ContentBlock::Json(value) => rendered.push_str(value.value().as_str()),
            _ => {
                return Err(request_error(
                    "tool result contains unsupported provider content",
                ));
            }
        }
    }
    Ok(rendered)
}

fn assistant_message_digest(message: &Message) -> Result<String, ModelError> {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in message.content() {
        match block {
            ContentBlock::Text(value) => text.push_str(value.text()),
            ContentBlock::Json(value) => text.push_str(value.value().as_str()),
            ContentBlock::ToolCall(call) => tool_calls.push(json!({
                "name": call.tool_name(),
                "arguments": call.arguments().as_str(),
            })),
            _ => {}
        }
    }
    content_digest(&text, &tool_calls)
}

fn content_digest(text: &str, tool_calls: &[Value]) -> Result<String, ModelError> {
    let value = json!({
        "text": text,
        "tool_calls": tool_calls,
    });
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| request_error("assistant content digest could not be encoded"))?;
    let raw = RawJson::parse(&bytes)
        .map_err(|_| request_error("assistant content digest is not canonical JSON"))?;
    Ok(raw.digest().to_hex())
}

#[allow(
    dead_code,
    reason = "request-side continuation helpers stay with the vendor crate"
)]
pub(crate) fn response_content_digest(
    text: &str,
    tool_calls: &[(String, String)],
) -> Result<String, ModelError> {
    let values = tool_calls
        .iter()
        .map(|(name, arguments)| json!({ "name": name, "arguments": arguments }))
        .collect::<Vec<_>>();
    content_digest(text, &values)
}

#[allow(
    dead_code,
    reason = "request-side continuation helpers stay with the vendor crate"
)]
pub(crate) fn encode_continuation(
    entries: Vec<ReplayEntry>,
) -> Result<Option<RawJson>, ModelError> {
    if entries.iter().all(|entry| entry.thinking.is_empty()) {
        return Ok(None);
    }
    let envelope = ContinuationEnvelope {
        provider: CONTINUATION_PROVIDER.to_owned(),
        version: CONTINUATION_VERSION,
        assistant_replay: entries,
    };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|_| request_error("provider continuation could not be encoded"))?;
    RawJson::parse(&bytes)
        .map(Some)
        .map_err(|_| request_error("provider continuation is not canonical JSON"))
}

pub(crate) fn serialize_request(request: &ChatRequest) -> Result<Vec<u8>, ModelError> {
    serde_json::to_vec(request).map_err(|_| request_error("provider request could not be encoded"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use finstack_ai_kernel::{
        JsonSchemaDraft, MessageId, Metadata, ProviderIds, SchemaRef, TextBlock, Timestamp,
        ToolCallBlock, ToolCallId, ToolResultBlock,
    };
    use finstack_ai_runtime::{ModelName, ModelRequestLimits, ModelSettings};

    #[test]
    fn maps_native_messages_tools_limits_and_reasoning() {
        let draft = draft_with(
            vec![
                text_message(MessageRole::System, "Stay concise."),
                text_message(MessageRole::User, "hello"),
            ],
            br#"{"temperature":0}"#,
            OutputSpec::PlainText,
            vec![tool("lookup", br#"{"type":"object"}"#)],
        );
        let request = ChatRequest::try_from_draft(
            &draft,
            &model().with_reasoning(true),
            None,
            &BTreeMap::new(),
        )
        .expect("request")
        .request;
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][0]["content"], "Stay concise.");
        assert_eq!(value["messages"][1]["role"], "user");
        assert_eq!(value["messages"][1]["content"], "hello");
        assert_eq!(value["stream"], true);
        assert_eq!(value["think"], true);
        assert_eq!(value["options"]["num_predict"], 1_024);
        assert_eq!(value["temperature"], 0);
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(value["tools"][0]["function"]["name"], "lookup");
        assert!(value.get("format").is_none());
    }

    #[test]
    fn structured_output_uses_top_level_format_and_omits_schema_tool() {
        let schema =
            RawJson::parse(br#"{"type":"object","properties":{"answer":{"type":"integer"}}}"#)
                .expect("schema");
        let draft = draft_with(
            vec![text_message(MessageRole::User, "hello")],
            b"{}",
            OutputSpec::JsonSchema {
                schema: SchemaRef {
                    draft: JsonSchemaDraft::Draft202012,
                    schema_version: 1,
                    schema_digest: schema.digest(),
                },
            },
            vec![tool(SUBMIT_FINAL_OUTPUT_TOOL, schema.as_bytes())],
        );
        let request = ChatRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect("request")
            .request;
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["format"]["type"], "object");
        assert!(value.get("tools").is_none());
    }

    #[test]
    fn tool_results_use_native_role_and_tool_name() {
        let call_id = ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
        let assistant = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("message id"),
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(
                ToolCallBlock::try_new(
                    call_id,
                    "lookup",
                    RawJson::parse(br#"{"city":"Toronto"}"#).expect("args"),
                )
                .expect("call"),
            )],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("assistant");
        let tool_result = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("message id"),
            MessageRole::Tool,
            vec![ContentBlock::ToolResult(
                ToolResultBlock::try_new(
                    call_id,
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
        let draft = draft_with(
            vec![
                text_message(MessageRole::User, "weather?"),
                assistant,
                tool_result,
            ],
            b"{}",
            OutputSpec::PlainText,
            Vec::new(),
        );
        let request = ChatRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect("request")
            .request;
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["messages"][1]["role"], "assistant");
        assert_eq!(
            value["messages"][1]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        assert_eq!(value["messages"][2]["role"], "tool");
        assert_eq!(value["messages"][2]["tool_name"], "lookup");
        assert_eq!(value["messages"][2]["content"], "ok");
    }

    #[test]
    fn continuation_replays_thinking_and_omits_it_on_digest_mismatch() {
        let assistant = text_message(MessageRole::Assistant, "hello");
        let digest = assistant_message_digest(&assistant).expect("digest");
        let draft = draft_with(
            vec![
                text_message(MessageRole::User, "hi"),
                assistant,
                text_message(MessageRole::User, "again"),
            ],
            b"{}",
            OutputSpec::PlainText,
            Vec::new(),
        );
        let matched = RawJson::parse(
            serde_json::to_vec(&json!({
                "provider": CONTINUATION_PROVIDER,
                "version": CONTINUATION_VERSION,
                "assistant_replay": [{ "thinking": "consider", "digest": digest }]
            }))
            .expect("json")
            .as_slice(),
        )
        .expect("continuation");
        let request =
            ChatRequest::try_from_draft(&draft, &model(), Some(&matched), &BTreeMap::new())
                .expect("request")
                .request;
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["messages"][1]["thinking"], "consider");

        let stale = RawJson::parse(
            serde_json::to_vec(&json!({
                "provider": CONTINUATION_PROVIDER,
                "version": CONTINUATION_VERSION,
                "assistant_replay": [{ "thinking": "stale", "digest": "00" }]
            }))
            .expect("json")
            .as_slice(),
        )
        .expect("stale");
        let rebuilt = ChatRequest::try_from_draft(&draft, &model(), Some(&stale), &BTreeMap::new())
            .expect("rebuild")
            .request;
        let value: Value = serde_json::from_slice(&serialize_request(&rebuilt).unwrap()).unwrap();
        assert!(value["messages"][1].get("thinking").is_none());

        let compacted = RawJson::parse(
            serde_json::to_vec(&json!({
                "provider": CONTINUATION_PROVIDER,
                "version": CONTINUATION_VERSION,
                "assistant_replay": [
                    { "thinking": "old" },
                    { "thinking": "extra" }
                ]
            }))
            .expect("json")
            .as_slice(),
        )
        .expect("compacted");
        let rebuilt =
            ChatRequest::try_from_draft(&draft, &model(), Some(&compacted), &BTreeMap::new())
                .expect("compaction rebuild")
                .request;
        let value: Value = serde_json::from_slice(&serialize_request(&rebuilt).unwrap()).unwrap();
        assert!(value["messages"][1].get("thinking").is_none());
    }

    #[test]
    fn rejects_reserved_settings() {
        let draft = draft_with(
            vec![text_message(MessageRole::User, "hello")],
            br#"{"model":"shadow"}"#,
            OutputSpec::PlainText,
            Vec::new(),
        );
        let error = ChatRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect_err("reserved setting must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn image_bytes_map_to_the_images_array() {
        let draft = draft_with(
            vec![media_message(ContentBlock::Image(media_ref("blob-1")))],
            b"{}",
            OutputSpec::PlainText,
            Vec::new(),
        );
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Bytes {
                media_type: Arc::from("image/png"),
                bytes: Arc::from(b"pngbytes".as_slice()),
            },
        );
        let request =
            ChatRequest::try_from_draft(&draft, &model().with_input_images(true), None, &resolved)
                .expect("request")
                .request;
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        let expected = base64::engine::general_purpose::STANDARD.encode(b"pngbytes");
        assert_eq!(value["messages"][0]["images"][0], expected);
    }

    #[test]
    fn image_url_resolution_is_rejected() {
        let draft = draft_with(
            vec![media_message(ContentBlock::Image(media_ref("blob-1")))],
            b"{}",
            OutputSpec::PlainText,
            Vec::new(),
        );
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );
        let error =
            ChatRequest::try_from_draft(&draft, &model().with_input_images(true), None, &resolved)
                .expect_err("url resolution must be rejected");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn image_input_is_rejected_when_the_model_flag_is_off() {
        let draft = draft_with(
            vec![media_message(ContentBlock::Image(media_ref("blob-1")))],
            b"{}",
            OutputSpec::PlainText,
            Vec::new(),
        );
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Bytes {
                media_type: Arc::from("image/png"),
                bytes: Arc::from(b"pngbytes".as_slice()),
            },
        );
        let error = ChatRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("image input must be rejected when the flag is off");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn file_and_audio_blocks_stay_rejected() {
        for block in [
            ContentBlock::File(media_ref("blob-1")),
            ContentBlock::Audio(media_ref("blob-1")),
        ] {
            let draft = draft_with(
                vec![media_message(block)],
                b"{}",
                OutputSpec::PlainText,
                Vec::new(),
            );
            let error = ChatRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
                .expect_err("file/audio blocks must be rejected");
            assert_eq!(error.code(), crate::error::REQUEST_INVALID);
        }
    }

    #[test]
    fn media_without_resolver_fails_closed() {
        let draft = draft_with(
            vec![media_message(ContentBlock::Image(media_ref("blob-1")))],
            b"{}",
            OutputSpec::PlainText,
            Vec::new(),
        );
        let error = ChatRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &BTreeMap::new(),
        )
        .expect_err("missing resolution must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    fn media_ref(id: &str) -> finstack_ai_kernel::MediaRef {
        finstack_ai_kernel::MediaRef::new(
            finstack_ai_kernel::BlobRef::try_new(id, "image/png", 4, None, None::<&str>)
                .expect("blob"),
        )
    }

    fn media_message(block: ContentBlock) -> Message {
        Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![block],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn model() -> OllamaModelConfig {
        OllamaModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
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

    fn tool(name: &str, schema: &[u8]) -> finstack_ai_runtime::ToolSpec {
        finstack_ai_runtime::ToolSpec {
            id: finstack_ai_kernel::ToolId::parse("finstack.tools.fixture").expect("tool id"),
            model_name: Arc::from(name),
            title: Arc::from("Fixture tool"),
            description: Arc::from("A deterministic fixture tool."),
            input_schema: RawJson::parse(schema).expect("schema"),
            output_schema: None,
            execution: finstack_ai_kernel::ToolExecutionMode::Sequential,
            side_effect: finstack_ai_runtime::SideEffectClass::ReadOnly,
            retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
            approval: finstack_ai_runtime::ApprovalMetadata {
                requirement: finstack_ai_runtime::ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 1_024,
            metadata: Metadata::empty(),
            deferral: finstack_ai_runtime::ToolDeferralSupport::Never,
        }
    }

    fn draft_with(
        messages: Vec<Message>,
        settings: &[u8],
        output: OutputSpec,
        tools: Vec<finstack_ai_runtime::ToolSpec>,
    ) -> ModelRequestDraft {
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from(messages),
            tools: Arc::from(tools),
            output,
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
