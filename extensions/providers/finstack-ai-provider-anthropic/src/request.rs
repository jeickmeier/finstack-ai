//! Private Anthropic Messages request translation.

use std::collections::BTreeMap;

use finstack_ai_runtime::{
    ContentBlock, Message, MessageRole, ModelError, ModelRequestDraft, OutputSpec,
    SUBMIT_FINAL_OUTPUT_TOOL,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::AnthropicModelConfig;
use crate::error::request_error;

const RESERVED_SETTINGS: &[&str] = &[
    "model",
    "messages",
    "tools",
    "stream",
    "system",
    "max_tokens",
    "max_completion_tokens",
    "response_format",
    "parallel_tool_calls",
    "stream_options",
];
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
}

#[derive(Debug, Serialize)]
struct WireTool {
    name: String,
    description: String,
    input_schema: Value,
}

impl MessagesRequest {
    pub(crate) fn try_from_draft(
        draft: &ModelRequestDraft,
        model: &AnthropicModelConfig,
    ) -> Result<Self, ModelError> {
        Self::try_from_draft_with_cache(draft, model, model.cache_breakpoints)
    }

    pub(crate) fn try_from_draft_with_cache(
        draft: &ModelRequestDraft,
        model: &AnthropicModelConfig,
        cache_breakpoints: bool,
    ) -> Result<Self, ModelError> {
        draft.validate()?;
        if draft.model != model.name {
            return Err(request_error("requested model is not configured"));
        }
        let (system, messages) = map_messages(&draft.messages, cache_breakpoints)?;
        let mut settings = parse_settings(&draft.settings.values)?;
        for reserved in RESERVED_SETTINGS {
            if settings.remove(*reserved).is_some() {
                return Err(request_error("provider settings contain a reserved field"));
            }
        }
        let thinking = take_thinking(&mut settings, model)?;
        let mut tools = Vec::with_capacity(draft.tools.len());
        for tool in draft.tools.iter() {
            if matches!(draft.output, OutputSpec::JsonSchema { .. })
                && tool.model_name.as_ref() == SUBMIT_FINAL_OUTPUT_TOOL
            {
                let OutputSpec::JsonSchema { schema } = &draft.output else {
                    unreachable!("JsonSchema matched above");
                };
                if tool.input_schema.digest() != schema.schema_digest {
                    return Err(request_error(
                        "structured-output schema does not match the committed schema reference",
                    ));
                }
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
            max_tokens: draft.limits.max_output_tokens.min(model.max_output_tokens),
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
) -> Result<Option<Value>, ModelError> {
    if let Some(value) = settings.remove("thinking") {
        return Ok(Some(value));
    }
    if let Some(value) = settings.remove("anthropic.thinking") {
        return Ok(Some(value));
    }
    if let Some(level) = settings.remove("thinking_level") {
        let budget = match level.as_str() {
            Some("low") => 1_024,
            Some("medium") => 4_096,
            Some("high") => 8_192,
            _ => return Err(request_error("thinking_level is not an allowlisted value")),
        };
        if budget >= model.max_output_tokens {
            return Err(request_error("thinking budget exceeds max_tokens"));
        }
        return Ok(Some(json!({
            "type": "enabled",
            "budget_tokens": budget
        })));
    }
    if model.thinking {
        if model.thinking_budget_tokens >= model.max_output_tokens {
            return Err(request_error("thinking budget exceeds max_tokens"));
        }
        return Ok(Some(json!({
            "type": "enabled",
            "budget_tokens": model.thinking_budget_tokens
        })));
    }
    Ok(None)
}

fn parse_settings(
    settings: &finstack_ai_runtime::RawJson,
) -> Result<BTreeMap<String, Value>, ModelError> {
    let value = raw_value(settings)?;
    let Value::Object(values) = value else {
        return Err(request_error("provider settings must be a JSON object"));
    };
    Ok(values.into_iter().collect())
}

fn raw_value(value: &finstack_ai_runtime::RawJson) -> Result<Value, ModelError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| request_error("canonical provider JSON could not be decoded"))
}

fn map_messages(
    messages: &[Message],
    cache_breakpoints: bool,
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
    if cache_breakpoints && let Some(last) = system.last_mut() {
        last.cache_control = Some(CacheControl { kind: "ephemeral" });
    }
    let mut mapped = Vec::new();
    for message in &messages[prefix_len..] {
        mapped.extend(map_conversation_message(message)?);
    }
    Ok((system, mapped))
}

fn map_conversation_message(message: &Message) -> Result<Vec<WireMessage>, ModelError> {
    match message.role() {
        MessageRole::User => Ok(vec![WireMessage {
            role: "user",
            content: vec![WireContent::Text {
                text: render_text(message.content())?,
            }],
        }]),
        MessageRole::Assistant => Ok(vec![WireMessage {
            role: "assistant",
            content: map_assistant_content(message.content())?,
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

fn map_assistant_content(content: &[ContentBlock]) -> Result<Vec<WireContent>, ModelError> {
    let mut blocks = Vec::new();
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
    use finstack_ai_runtime::{
        MessageId, Metadata, ModelName, ModelRequestLimits, ModelSettings, ProviderIds, TextBlock,
        Timestamp,
    };

    #[test]
    fn rejects_reserved_settings_and_keeps_system_prefix_stable() {
        let mut draft = draft(br#"{"model":"shadow"}"#);
        let error = MessagesRequest::try_from_draft(&draft, &model())
            .expect_err("reserved setting must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);

        draft.settings.values =
            finstack_ai_runtime::RawJson::parse(br#"{"temperature":0}"#).expect("settings");
        let request = MessagesRequest::try_from_draft(&draft, &model()).expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(value["temperature"], 0);
        assert_eq!(value["stream"], true);
        assert!(value.get("system").is_none());
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
                values: finstack_ai_runtime::RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        };
        let model = model().with_cache_breakpoints(true);
        let request = MessagesRequest::try_from_draft(&draft, &model).expect("request");
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
                values: finstack_ai_runtime::RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        };
        let model = model().with_cache_breakpoints(true);
        let cached =
            MessagesRequest::try_from_draft_with_cache(&draft, &model, true).expect("cached");
        let busted =
            MessagesRequest::try_from_draft_with_cache(&draft, &model, false).expect("busted");
        let cached: Value = serde_json::from_slice(&serialize_request(&cached).unwrap()).unwrap();
        let busted: Value = serde_json::from_slice(&serialize_request(&busted).unwrap()).unwrap();
        assert_eq!(cached["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(busted["system"][0].get("cache_control").is_none());
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
                values: finstack_ai_runtime::RawJson::parse(settings).expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 1_024,
            },
        }
    }
}
