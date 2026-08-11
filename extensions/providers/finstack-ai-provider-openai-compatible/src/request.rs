//! Private Chat Completions request translation.

use std::collections::BTreeMap;

use finstack_ai_runtime::{
    ContentBlock, Message, MessageRole, ModelError, ModelRequestDraft, OutputSpec,
    SUBMIT_FINAL_OUTPUT_TOOL,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::request_error;
use crate::{EndpointQuirks, OpenAiModelConfig};

const RESERVED_SETTINGS: &[&str] = &[
    "model",
    "messages",
    "tools",
    "stream",
    "stream_options",
    "max_completion_tokens",
    "response_format",
    "parallel_tool_calls",
];

#[derive(Debug, Serialize)]
pub(crate) struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    max_completion_tokens: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ChatTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
    #[serde(flatten)]
    settings: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<ChatToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatToolCall {
    id: String,
    r#type: &'static str,
    function: ChatFunctionCall,
}

#[derive(Debug, Serialize)]
struct ChatFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct ChatTool {
    r#type: &'static str,
    function: ChatFunction,
}

#[derive(Debug, Serialize)]
struct ChatFunction {
    name: String,
    description: String,
    parameters: Value,
    strict: bool,
}

impl ChatCompletionRequest {
    pub(crate) fn try_from_draft(
        draft: &ModelRequestDraft,
        model: &OpenAiModelConfig,
        quirks: EndpointQuirks,
    ) -> Result<Self, ModelError> {
        draft.validate()?;
        if draft.model != model.name {
            return Err(request_error("requested model is not configured"));
        }
        let messages = draft
            .messages
            .iter()
            .map(|message| map_message(message, quirks))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        let mut settings = parse_settings(&draft.settings.values)?;
        for reserved in RESERVED_SETTINGS {
            if settings.remove(*reserved).is_some() {
                return Err(request_error("provider settings contain a reserved field"));
            }
        }

        let mut response_format = None;
        let mut tools = Vec::with_capacity(draft.tools.len());
        for tool in draft.tools.iter() {
            if tool.model_name.as_ref() == SUBMIT_FINAL_OUTPUT_TOOL
                && quirks.native_structured_output
                && let OutputSpec::JsonSchema { schema } = &draft.output
            {
                if tool.input_schema.digest() != schema.schema_digest {
                    return Err(request_error(
                        "structured-output schema does not match the committed schema reference",
                    ));
                }
                let schema_value = raw_value(&tool.input_schema)?;
                response_format = Some(json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": "finstack_final_output",
                        "strict": true,
                        "schema": schema_value
                    }
                }));
                continue;
            }
            tools.push(ChatTool {
                r#type: "function",
                function: ChatFunction {
                    name: tool.model_name.to_string(),
                    description: tool.description.to_string(),
                    parameters: raw_value(&tool.input_schema)?,
                    strict: true,
                },
            });
        }
        if matches!(draft.output, OutputSpec::JsonSchema { .. })
            && quirks.native_structured_output
            && response_format.is_none()
        {
            return Err(request_error(
                "structured output requires the matching framework schema tool",
            ));
        }

        Ok(Self {
            model: draft.model.as_str().to_owned(),
            messages,
            stream: true,
            stream_options: quirks.stream_usage.then_some(StreamOptions {
                include_usage: true,
            }),
            max_completion_tokens: draft.limits.max_output_tokens.min(model.max_output_tokens),
            parallel_tool_calls: (!tools.is_empty()).then_some(model.parallel_tool_calls),
            tools,
            response_format,
            settings,
        })
    }
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

fn map_message(message: &Message, quirks: EndpointQuirks) -> Result<Vec<ChatMessage>, ModelError> {
    if message.role() == MessageRole::Tool {
        return message
            .content()
            .iter()
            .map(|block| match block {
                ContentBlock::ToolResult(result) => Ok(ChatMessage {
                    role: "tool",
                    content: Some(render_content(result.content())?),
                    tool_calls: Vec::new(),
                    tool_call_id: Some(result.tool_call_id().to_string()),
                }),
                _ => Err(request_error("tool messages contain unsupported content")),
            })
            .collect();
    }

    let role = match message.role() {
        MessageRole::Developer if quirks.developer_role => "developer",
        MessageRole::System | MessageRole::Developer => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => unreachable!("tool messages return above"),
    };
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in message.content() {
        match block {
            ContentBlock::Text(value) => text.push_str(value.text()),
            ContentBlock::Json(value) => text.push_str(value.value().as_str()),
            ContentBlock::ToolCall(call) if message.role() == MessageRole::Assistant => {
                tool_calls.push(ChatToolCall {
                    id: call.tool_call_id().to_string(),
                    r#type: "function",
                    function: ChatFunctionCall {
                        name: call.tool_name().to_owned(),
                        arguments: call.arguments().as_str().to_owned(),
                    },
                });
            }
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    Ok(vec![ChatMessage {
        role,
        content: (!text.is_empty()).then_some(text),
        tool_calls,
        tool_call_id: None,
    }])
}

fn render_content(content: &[ContentBlock]) -> Result<String, ModelError> {
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

pub(crate) fn serialize_request(request: &ChatCompletionRequest) -> Result<Vec<u8>, ModelError> {
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
    fn rejects_reserved_settings() {
        let mut draft = draft(br#"{"model":"shadow"}"#);
        let error = ChatCompletionRequest::try_from_draft(
            &draft,
            &model(),
            EndpointQuirks::for_kind(crate::EndpointKind::OpenAi),
        )
        .expect_err("reserved setting must fail");
        assert_eq!(error.code(), crate::error::REQUEST_INVALID);

        draft.settings.values =
            finstack_ai_runtime::RawJson::parse(br#"{"temperature":0}"#).expect("settings");
        let request = ChatCompletionRequest::try_from_draft(
            &draft,
            &model(),
            EndpointQuirks::for_kind(crate::EndpointKind::OpenAi),
        )
        .expect("request");
        let value: Value = serde_json::from_slice(&serialize_request(&request).unwrap()).unwrap();
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"], "hello");
        assert_eq!(value["temperature"], 0);
        assert_eq!(value["stream_options"]["include_usage"], true);
    }

    fn model() -> OpenAiModelConfig {
        OpenAiModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
            .expect("model")
    }

    fn draft(settings: &[u8]) -> ModelRequestDraft {
        let message = Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            MessageRole::User,
            vec![ContentBlock::Text(
                TextBlock::try_new("hello").expect("text"),
            )],
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
