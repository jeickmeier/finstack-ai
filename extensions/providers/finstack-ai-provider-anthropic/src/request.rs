//! Private Anthropic Messages request translation.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine;
use finstack_ai_kernel::{
    ContentBlock, MediaRef, Message, MessageRole, OutputSpec, RawJson, SUBMIT_FINAL_OUTPUT_TOOL,
};
use finstack_ai_runtime::ports::model::{
    ModelError, ModelRequestDraft, ResolvedMedia, thinking_level_budget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AnthropicModelConfig;
use crate::error::request_error;

/// Messages-API keys the request owns; settings may not shadow them.
///
/// `thinking` is deliberately absent: `take_thinking` consumes it from the
/// settings before they are flattened, so it is a passthrough rather than a
/// shadowing key. `metadata` is reserved even though the request never sets
/// it today, so a later owner can decide how it is populated.
const RESERVED_SETTINGS: &[&str] = &[
    "model",
    "messages",
    "tools",
    "stream",
    "system",
    "max_tokens",
    "metadata",
];

/// Provider tag of the continuation envelope produced by the wire assembler.
const CONTINUATION_PROVIDER: &str = "anthropic.messages";
const CONTINUATION_VERSION: u64 = 1;

/// Opaque continuation state emitted by `AnthropicMessagesAssembly`.
#[derive(Deserialize)]
struct ContinuationEnvelope {
    provider: String,
    version: u64,
    replay_blocks: Vec<ReplayBlock>,
}

/// One thinking-family block replayed verbatim at the head of the last
/// assistant turn. Only the two shapes the API accepts back are decodable,
/// so a tampered envelope fails closed as a request error.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum ReplayBlock {
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
}
#[derive(Debug, Serialize)]
pub(crate) struct MessagesRequest {
    model: String,
    max_tokens: u64,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<SystemBlock>,
    messages: Vec<WireMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<WireTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Value>,
    #[serde(flatten)]
    settings: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct SystemBlock {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<WireContent>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum WireContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    #[serde(rename = "image")]
    Image { source: Value },
    #[serde(rename = "document")]
    Document { source: Value },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
}

impl From<ReplayBlock> for WireContent {
    fn from(block: ReplayBlock) -> Self {
        match block {
            ReplayBlock::Thinking {
                thinking,
                signature,
            } => Self::Thinking {
                thinking,
                signature,
            },
            ReplayBlock::RedactedThinking { data } => Self::RedactedThinking { data },
        }
    }
}

#[derive(Debug, Serialize)]
struct WireTool {
    name: String,
    description: String,
    input_schema: Value,
}

impl MessagesRequest {
    /// Translate a committed draft into one Messages request.
    ///
    /// `continuation` is the opaque envelope from the previous completion.
    /// Its provider and version are always checked; its thinking blocks are
    /// replayed at the head of the last assistant turn only when the outgoing
    /// request has thinking enabled, because the API rejects thinking blocks
    /// on a request without `thinking`.
    pub(crate) fn try_from_draft(
        draft: &ModelRequestDraft,
        model: &AnthropicModelConfig,
        continuation: Option<&RawJson>,
        resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    ) -> Result<Self, ModelError> {
        draft.validate()?;
        if draft.model != model.name {
            return Err(request_error("requested model is not configured"));
        }
        let mut settings = parse_settings(&draft.settings.values)?;
        for reserved in RESERVED_SETTINGS {
            if settings.remove(*reserved).is_some() {
                return Err(request_error("provider settings contain a reserved field"));
            }
        }
        let max_tokens = draft.limits.max_output_tokens.min(model.max_output_tokens);
        let thinking = take_thinking(&mut settings, model, max_tokens)?;
        let replay = match continuation {
            Some(state) => parse_continuation(state)?.replay_blocks,
            None => Vec::new(),
        };
        let replay = if thinking_enabled(thinking.as_ref()) {
            replay
        } else {
            Vec::new()
        };
        let (system, messages) = map_messages(&draft.messages, &replay, resolved, model)?;
        let mut tools = Vec::with_capacity(draft.tools.len());
        for tool in draft.tools.iter() {
            if let OutputSpec::JsonSchema { schema } = &draft.output
                && tool.model_name.as_ref() == SUBMIT_FINAL_OUTPUT_TOOL
                && tool.input_schema.digest() != schema.schema_digest
            {
                return Err(request_error(
                    "structured-output schema does not match the committed schema reference",
                ));
            }
            tools.push(WireTool {
                name: tool.model_name.to_string(),
                description: tool.description.to_string(),
                input_schema: raw_value(&tool.input_schema)?,
            });
        }
        if matches!(draft.output, OutputSpec::JsonSchema { .. })
            && !tools
                .iter()
                .any(|tool| tool.name == SUBMIT_FINAL_OUTPUT_TOOL)
        {
            return Err(request_error(
                "structured output requires the matching framework schema tool",
            ));
        }

        Ok(Self {
            model: draft.model.as_str().to_owned(),
            max_tokens,
            stream: true,
            system,
            messages,
            tools,
            thinking,
            settings,
        })
    }
}

fn take_thinking(
    settings: &mut BTreeMap<String, Value>,
    model: &AnthropicModelConfig,
    max_tokens: u64,
) -> Result<Option<Value>, ModelError> {
    if let Some(value) = settings.remove("thinking") {
        return Ok(Some(value));
    }
    if let Some(value) = settings.remove("anthropic.thinking") {
        return Ok(Some(value));
    }
    if let Some(level) = settings.remove("thinking_level") {
        let budget = level
            .as_str()
            .and_then(thinking_level_budget)
            .ok_or_else(|| request_error("thinking_level is not an allowlisted value"))?;
        if budget >= max_tokens {
            return Err(request_error("thinking budget exceeds max_tokens"));
        }
        return Ok(Some(json!({
            "type": "enabled",
            "budget_tokens": budget
        })));
    }
    if model.thinking {
        if model.thinking_budget_tokens >= max_tokens {
            return Err(request_error("thinking budget exceeds max_tokens"));
        }
        return Ok(Some(json!({
            "type": "enabled",
            "budget_tokens": model.thinking_budget_tokens
        })));
    }
    Ok(None)
}

/// Whether the outgoing `thinking` parameter turns thinking on.
fn thinking_enabled(thinking: Option<&Value>) -> bool {
    thinking.is_some_and(|value| value.get("type").and_then(Value::as_str) != Some("disabled"))
}

fn parse_continuation(state: &RawJson) -> Result<ContinuationEnvelope, ModelError> {
    let envelope: ContinuationEnvelope = serde_json::from_slice(state.as_bytes())
        .map_err(|_| request_error("continuation state is invalid"))?;
    if envelope.provider != CONTINUATION_PROVIDER || envelope.version != CONTINUATION_VERSION {
        return Err(request_error("continuation state is invalid"));
    }
    Ok(envelope)
}

fn parse_settings(
    settings: &finstack_ai_kernel::RawJson,
) -> Result<BTreeMap<String, Value>, ModelError> {
    let value = raw_value(settings)?;
    let Value::Object(values) = value else {
        return Err(request_error("provider settings must be a JSON object"));
    };
    Ok(values.into_iter().collect())
}

fn raw_value(value: &finstack_ai_kernel::RawJson) -> Result<Value, ModelError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| request_error("canonical provider JSON could not be decoded"))
}

fn map_messages(
    messages: &[Message],
    replay: &[ReplayBlock],
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &AnthropicModelConfig,
) -> Result<(Vec<SystemBlock>, Vec<WireMessage>), ModelError> {
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
    let mut system = Vec::new();
    for message in &messages[..prefix_len] {
        system.push(SystemBlock {
            kind: "text",
            text: render_text(message.content())?,
            cache_control: None,
        });
    }
    if model.cache_breakpoints
        && let Some(last) = system.last_mut()
    {
        last.cache_control = Some(CacheControl { kind: "ephemeral" });
    }
    // Replayed thinking belongs to the latest assistant turn only; with no
    // assistant turn there is nothing to continue and the replay is dropped.
    let last_assistant = messages
        .iter()
        .rposition(|message| message.role() == MessageRole::Assistant);
    let mut mapped = Vec::new();
    for (index, message) in messages.iter().enumerate().skip(prefix_len) {
        let replay = if Some(index) == last_assistant {
            replay
        } else {
            &[]
        };
        mapped.extend(map_conversation_message(message, replay, resolved, model)?);
    }
    Ok((system, mapped))
}

fn map_conversation_message(
    message: &Message,
    replay: &[ReplayBlock],
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &AnthropicModelConfig,
) -> Result<Vec<WireMessage>, ModelError> {
    match message.role() {
        MessageRole::User => Ok(vec![WireMessage {
            role: "user",
            content: map_user_content(message.content(), resolved, model)?,
        }]),
        MessageRole::Assistant => Ok(vec![WireMessage {
            role: "assistant",
            content: map_assistant_content(message.content(), replay)?,
        }]),
        MessageRole::Tool => Ok(vec![WireMessage {
            role: "user",
            content: map_tool_results(message.content())?,
        }]),
        MessageRole::System | MessageRole::Developer => Err(request_error(
            "system instructions must remain a stable leading prefix",
        )),
    }
}

fn map_user_content(
    content: &[ContentBlock],
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &AnthropicModelConfig,
) -> Result<Vec<WireContent>, ModelError> {
    let mut wire = Vec::new();
    let mut text = String::new();
    let flush = |wire: &mut Vec<WireContent>, text: &mut String| {
        if !text.is_empty() {
            wire.push(WireContent::Text {
                text: std::mem::take(text),
            });
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
                flush(&mut wire, &mut text);
                wire.push(WireContent::Image {
                    source: media_source(media, resolved)?,
                });
            }
            ContentBlock::File(media) => {
                if !model.input_files {
                    return Err(request_error("model is not configured for file input"));
                }
                flush(&mut wire, &mut text);
                wire.push(WireContent::Document {
                    source: media_source(media, resolved)?,
                });
            }
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    flush(&mut wire, &mut text);
    Ok(wire)
}

fn media_source(
    media: &MediaRef,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
) -> Result<Value, ModelError> {
    match resolved.get(media.blob().id()) {
        Some(ResolvedMedia::Url(url)) => Ok(json!({ "type": "url", "url": url.as_ref() })),
        Some(ResolvedMedia::Bytes { media_type, bytes }) => Ok(json!({
            "type": "base64",
            "media_type": media_type.as_ref(),
            "data": base64::engine::general_purpose::STANDARD.encode(bytes)
        })),
        None => Err(request_error(
            "media content requires a configured media resolver",
        )),
    }
}

/// Map one assistant turn, prepending any replayed thinking blocks so they
/// precede the turn's text and `tool_use` blocks as the API requires.
fn map_assistant_content(
    content: &[ContentBlock],
    replay: &[ReplayBlock],
) -> Result<Vec<WireContent>, ModelError> {
    let mut blocks = Vec::with_capacity(replay.len() + content.len());
    blocks.extend(replay.iter().cloned().map(WireContent::from));
    for block in content {
        match block {
            ContentBlock::Text(value) => blocks.push(WireContent::Text {
                text: value.text().to_owned(),
            }),
            ContentBlock::Json(value) => blocks.push(WireContent::Text {
                text: value.value().as_str().to_owned(),
            }),
            ContentBlock::ToolCall(call) => {
                let input = raw_value(call.arguments())?;
                blocks.push(WireContent::ToolUse {
                    id: call.tool_call_id().to_string(),
                    name: call.tool_name().to_owned(),
                    input,
                });
            }
            ContentBlock::Opaque(_) => {}
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    Ok(blocks)
}

fn map_tool_results(content: &[ContentBlock]) -> Result<Vec<WireContent>, ModelError> {
    content
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult(result) => Ok(WireContent::ToolResult {
                tool_use_id: result.tool_call_id().to_string(),
                content: render_text(result.content())?,
            }),
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

pub(crate) fn serialize_request(request: &MessagesRequest) -> Result<Vec<u8>, ModelError> {
    serde_json::to_vec(request).map_err(|_| request_error("provider request could not be encoded"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use finstack_ai_kernel::{MessageId, Metadata, ProviderIds, TextBlock, Timestamp};
    use finstack_ai_runtime::ports::model::{ModelName, ModelRequestLimits, ModelSettings};

    #[test]
    fn rejects_reserved_settings_and_keeps_system_prefix_stable() {
        let mut draft = draft(br#"{"model":"shadow"}"#);
        let error = MessagesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect_err("reserved setting must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);

        draft.settings.values =
            finstack_ai_kernel::RawJson::parse(br#"{"temperature":0}"#).expect("settings");
        let request = MessagesRequest::try_from_draft(&draft, &model(), None, &BTreeMap::new())
            .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(value["temperature"], 0);
        assert_eq!(value["stream"], true);
        assert!(value.get("system").is_none());
    }

    #[test]
    fn bare_thinking_setting_passes_through_and_api_keys_stay_reserved() {
        let passthrough = draft(br#"{"thinking":{"type":"enabled","budget_tokens":256}}"#);
        let request =
            MessagesRequest::try_from_draft(&passthrough, &model(), None, &BTreeMap::new())
                .expect("bare thinking is a passthrough");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["thinking"]["type"], "enabled");
        assert_eq!(value["thinking"]["budget_tokens"], 256);

        for reserved in RESERVED_SETTINGS {
            let shadowing = draft(format!(r#"{{"{reserved}":1}}"#).as_bytes());
            let error =
                MessagesRequest::try_from_draft(&shadowing, &model(), None, &BTreeMap::new())
                    .expect_err("reserved key must fail");
            assert_eq!(error.code(), crate::error::REQUEST_INVALID, "{reserved}");
        }
        // Keys that belong to other vendors' APIs are no longer reserved.
        let foreign = draft(br#"{"stream_options":{"include_usage":true}}"#);
        assert!(
            MessagesRequest::try_from_draft(&foreign, &model(), None, &BTreeMap::new()).is_ok()
        );
    }

    #[test]
    fn thinking_budget_must_fit_the_effective_request_limit() {
        let mut configured = draft(b"{}");
        configured.limits.max_output_tokens = 512;
        let thinking_model = model().with_thinking(true, 1_024).expect("thinking model");
        let error =
            MessagesRequest::try_from_draft(&configured, &thinking_model, None, &BTreeMap::new())
                .expect_err("configured thinking must fit the request limit");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);

        let mut requested = draft(br#"{"thinking_level":"low"}"#);
        requested.limits.max_output_tokens = 512;
        let error = MessagesRequest::try_from_draft(&requested, &model(), None, &BTreeMap::new())
            .expect_err("requested thinking must fit the request limit");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn cache_control_marks_the_last_system_block_without_reordering() {
        let system_a = text_message(MessageRole::System, "Stable prefix.");
        let system_b = text_message(MessageRole::System, "Always instruction.");
        let user = text_message(MessageRole::User, "hello");
        let draft = ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from([system_a, system_b, user]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: finstack_ai_kernel::RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        };
        let model = model().with_cache_breakpoints(true);
        let request = MessagesRequest::try_from_draft(&draft, &model, None, &BTreeMap::new())
            .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["system"][0]["text"], "Stable prefix.");
        assert!(value["system"][0].get("cache_control").is_none());
        assert_eq!(value["system"][1]["text"], "Always instruction.");
        assert_eq!(value["system"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(value["messages"][0]["content"][0]["text"], "hello");
    }

    #[test]
    fn cache_control_is_omitted_when_the_tool_catalog_changes() {
        let system = text_message(MessageRole::System, "Stable prefix.");
        let user = text_message(MessageRole::User, "hello");
        let draft = ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from([system, user]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: finstack_ai_kernel::RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        };
        let cached = MessagesRequest::try_from_draft(
            &draft,
            &model().with_cache_breakpoints(true),
            None,
            &BTreeMap::new(),
        )
        .expect("cached");
        let busted = MessagesRequest::try_from_draft(
            &draft,
            &model().with_cache_breakpoints(false),
            None,
            &BTreeMap::new(),
        )
        .expect("busted");
        let cached: Value = serde_json::from_slice(&serialize_request(&cached).unwrap()).unwrap();
        let busted: Value = serde_json::from_slice(&serialize_request(&busted).unwrap()).unwrap();
        assert_eq!(cached["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(busted["system"][0].get("cache_control").is_none());
    }

    #[test]
    fn image_url_resolution_maps_to_an_image_block_with_a_url_source() {
        let draft = media_draft(ContentBlock::Image(media_ref()));
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &resolved,
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        let content = value["messages"][0]["content"].as_array().unwrap();
        assert_eq!(
            content.len(),
            1,
            "media-only message must omit an empty text block"
        );
        let block = &content[0];
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "url");
        assert_eq!(block["source"]["url"], "https://cdn.example/a.png");
    }

    #[test]
    fn image_bytes_resolution_maps_to_a_base64_source() {
        let draft = media_draft(ContentBlock::Image(media_ref()));
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Bytes {
                media_type: Arc::from("image/png"),
                bytes: Arc::from(&b"pngdata"[..]),
            },
        );
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &resolved,
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        let block = &value["messages"][0]["content"][0];
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(
            block["source"]["data"],
            base64::engine::general_purpose::STANDARD.encode(b"pngdata")
        );
    }

    #[test]
    fn file_blocks_map_to_document_blocks() {
        let draft = media_draft(ContentBlock::File(media_ref()));
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.pdf")),
        );
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model().with_input_files(true),
            None,
            &resolved,
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        let block = &value["messages"][0]["content"][0];
        assert_eq!(block["type"], "document");
        assert_eq!(block["source"]["type"], "url");
    }

    #[test]
    fn caption_text_precedes_the_image_block_and_no_extra_blocks_are_added() {
        let draft = text_and_media_draft("describe this", ContentBlock::Image(media_ref()));
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &resolved,
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        let content = value["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "describe this");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "url");
    }

    #[test]
    fn text_image_text_interleaving_is_preserved_not_coalesced() {
        let draft = content_draft(vec![
            ContentBlock::Text(TextBlock::try_new("before").expect("text")),
            ContentBlock::Image(media_ref()),
            ContentBlock::Text(TextBlock::try_new("after").expect("text")),
        ]);
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &resolved,
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        let content = value["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3, "text must not be coalesced across media");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "before");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[2]["type"], "text");
        assert_eq!(content[2]["text"], "after");
    }

    #[test]
    fn image_input_is_rejected_when_the_model_flag_is_off() {
        let draft = media_draft(ContentBlock::Image(media_ref()));
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );
        let error = MessagesRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("image input must be rejected when the flag is off");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn file_input_is_rejected_when_the_model_flag_is_off() {
        let draft = media_draft(ContentBlock::File(media_ref()));
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.pdf")),
        );
        let error = MessagesRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("file input must be rejected when the flag is off");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn media_without_resolver_fails_closed() {
        let draft = media_draft(ContentBlock::Image(media_ref()));
        let error = MessagesRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &BTreeMap::new(),
        )
        .expect_err("unresolved media must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn audio_blocks_stay_rejected() {
        let draft = media_draft(ContentBlock::Audio(media_ref()));
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.mp3")),
        );
        let error = MessagesRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("audio input must stay rejected");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);
    }

    #[test]
    fn continuation_replays_thinking_blocks_at_head_of_last_assistant_turn() {
        let draft = tool_loop_draft();
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model().with_thinking(true, 512).expect("thinking"),
            Some(&continuation()),
            &BTreeMap::new(),
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        let messages = value["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        // The earlier assistant turn is untouched.
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "text");
        assert_eq!(messages[1]["content"].as_array().unwrap().len(), 1);
        // The last assistant turn leads with the replayed blocks, in order.
        let last = messages[3]["content"].as_array().unwrap();
        assert_eq!(messages[3]["role"], "assistant");
        assert_eq!(last.len(), 4);
        assert_eq!(last[0]["type"], "thinking");
        assert_eq!(last[0]["thinking"], "consider");
        assert_eq!(last[0]["signature"], "sig-1");
        assert_eq!(last[1]["type"], "redacted_thinking");
        assert_eq!(last[1]["data"], "opaque");
        assert_eq!(last[2]["type"], "text");
        assert_eq!(last[3]["type"], "tool_use");
        assert_eq!(value["thinking"]["type"], "enabled");
    }

    #[test]
    fn continuation_ignored_when_thinking_disabled() {
        let draft = tool_loop_draft();
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model(),
            Some(&continuation()),
            &BTreeMap::new(),
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert!(value.get("thinking").is_none());
        let rendered = value.to_string();
        assert!(!rendered.contains("redacted_thinking"));
        assert!(!rendered.contains("sig-1"));
        assert!(!rendered.contains("\"type\":\"thinking\""));
        let last = value["messages"][3]["content"].as_array().unwrap();
        assert_eq!(last.len(), 2);
        assert_eq!(last[0]["type"], "text");
        assert_eq!(last[1]["type"], "tool_use");

        // An explicit `thinking: {"type":"disabled"}` override also ignores it.
        let mut disabled = tool_loop_draft();
        disabled.settings.values =
            RawJson::parse(br#"{"anthropic.thinking":{"type":"disabled"}}"#).expect("settings");
        let request = MessagesRequest::try_from_draft(
            &disabled,
            &model().with_thinking(true, 512).expect("thinking"),
            Some(&continuation()),
            &BTreeMap::new(),
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert!(!value.to_string().contains("sig-1"));
    }

    #[test]
    fn continuation_provider_mismatch_is_rejected() {
        let draft = tool_loop_draft();
        for state in [
            br#"{"provider":"openai.responses","version":1,"replay_blocks":[]}"#.as_slice(),
            br#"{"provider":"anthropic.messages","version":2,"replay_blocks":[]}"#.as_slice(),
            br#"{"provider":"anthropic.messages","version":1,"replay_blocks":[{"type":"text","text":"smuggled"}]}"#.as_slice(),
            br#"{"provider":"anthropic.messages","version":1,"replay_blocks":[{"type":"thinking","thinking":"unsigned"}]}"#.as_slice(),
            br"[]".as_slice(),
        ] {
            let state = RawJson::parse(state).expect("state");
            // Rejected whether or not thinking is enabled: a foreign envelope
            // is a routing fault, not a thinking-mode question.
            for thinking in [true, false] {
                let model = if thinking {
                    model().with_thinking(true, 512).expect("thinking")
                } else {
                    model()
                };
                let error = MessagesRequest::try_from_draft(
                    &draft,
                    &model,
                    Some(&state),
                    &BTreeMap::new(),
                )
                .expect_err("foreign continuation must fail");
                assert_eq!(error.code(), crate::error::REQUEST_INVALID);
                assert_eq!(error.message(), "continuation state is invalid");
            }
        }
    }

    #[test]
    fn continuation_without_an_assistant_turn_is_dropped() {
        let draft = draft(b"{}");
        let request = MessagesRequest::try_from_draft(
            &draft,
            &model().with_thinking(true, 512).expect("thinking"),
            Some(&continuation()),
            &BTreeMap::new(),
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["messages"].as_array().unwrap().len(), 1);
        assert!(!value.to_string().contains("sig-1"));
    }

    fn continuation() -> RawJson {
        RawJson::parse(
            br#"{"provider":"anthropic.messages","version":1,"replay_blocks":[{"type":"thinking","thinking":"consider","signature":"sig-1"},{"type":"redacted_thinking","data":"opaque"}]}"#,
        )
        .expect("continuation")
    }

    /// `user -> assistant(text) -> user -> assistant(text + tool_use)`: the
    /// replay must land on the last assistant turn only.
    fn tool_loop_draft() -> ModelRequestDraft {
        let call = finstack_ai_kernel::ToolCallBlock::try_new(
            finstack_ai_kernel::ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789cc")
                .expect("call id"),
            "lookup",
            RawJson::parse(br#"{"x":1}"#).expect("arguments"),
        )
        .expect("tool call");
        let last_assistant = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("message id"),
            MessageRole::Assistant,
            vec![
                ContentBlock::Text(TextBlock::try_new("looking").expect("text")),
                ContentBlock::ToolCall(call),
            ],
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from([
                text_message(MessageRole::User, "hello"),
                text_message(MessageRole::Assistant, "hi"),
                text_message(MessageRole::User, "look it up"),
                last_assistant,
            ]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        }
    }

    fn media_ref() -> finstack_ai_kernel::MediaRef {
        finstack_ai_kernel::MediaRef::new(
            finstack_ai_kernel::BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>)
                .expect("blob"),
        )
    }

    fn media_draft(block: ContentBlock) -> ModelRequestDraft {
        content_draft(vec![block])
    }

    fn text_and_media_draft(text: &str, block: ContentBlock) -> ModelRequestDraft {
        content_draft(vec![
            ContentBlock::Text(TextBlock::try_new(text).expect("text")),
            block,
        ])
    }

    fn content_draft(content: Vec<ContentBlock>) -> ModelRequestDraft {
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            content,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from([message]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: finstack_ai_kernel::RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        }
    }

    fn model() -> AnthropicModelConfig {
        AnthropicModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
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

    fn draft(settings: &[u8]) -> ModelRequestDraft {
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from([text_message(MessageRole::User, "hello")]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: finstack_ai_kernel::RawJson::parse(settings).expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        }
    }
}
