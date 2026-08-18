//! `OpenAI` Chat Completions SSE event assembly.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::{
    ContentBlock, JsonBlock, ModelResponse, ModelStreamItem, ModelToolCall, ProviderIds, RawJson,
    ReasoningDelta, TextBlock, TextDelta, ToolCallDelta, Usage, UsageDelta,
};

use super::StreamNormError;

#[derive(Deserialize)]
struct WireChunk {
    id: Option<String>,
    #[serde(default)]
    choices: Vec<WireChoice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    #[serde(default)]
    delta: Option<WireDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCallDelta>,
}

#[derive(Deserialize)]
struct WireToolCallDelta {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireFunctionDelta>,
}

#[derive(Deserialize)]
struct WireFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
#[allow(
    clippy::struct_field_names,
    reason = "private wire fields match the OpenAI Chat Completions usage object"
)]
struct WireUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

struct ToolAssembly {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// Incremental Chat Completions assembler.
pub struct OpenAiChatAssembly {
    request_id: String,
    structured: bool,
    completion_id: Option<String>,
    text: String,
    tools: BTreeMap<u32, ToolAssembly>,
    usage: Usage,
    finished: bool,
}

impl OpenAiChatAssembly {
    /// Start a new Chat Completions assembly for one committed request.
    #[must_use]
    pub fn new(request_id: String, structured: bool) -> Self {
        Self {
            request_id,
            structured,
            completion_id: None,
            text: String::new(),
            tools: BTreeMap::new(),
            usage: Usage::empty(),
            finished: false,
        }
    }

    /// Consume one JSON event payload or the `[DONE]` sentinel.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when the event is malformed.
    pub fn consume(&mut self, data: &str) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        if data == "[DONE]" {
            return self.finish_done();
        }
        let chunk: WireChunk = serde_json::from_str(data)
            .map_err(|_| StreamNormError::stream("SSE event is invalid JSON"))?;
        if let Some(id) = chunk.id.filter(|value| !value.is_empty()) {
            if self
                .completion_id
                .as_deref()
                .is_some_and(|known| known != id)
            {
                return Err(StreamNormError::response(
                    "completion ID changed within one stream",
                ));
            }
            self.completion_id = Some(id);
        }
        let mut items = Vec::new();
        for choice in chunk.choices {
            if let Some(delta) = choice.delta {
                items.extend(self.consume_delta(delta));
            }
            if choice.finish_reason.as_deref() == Some("length") {
                return Err(StreamNormError::incomplete("response was incomplete"));
            }
        }
        if let Some(usage) = chunk.usage {
            self.usage = Usage::try_new(
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
                None,
                BTreeMap::new(),
            )
            .map_err(|_| StreamNormError::response("usage is invalid"))?;
            items.push(ModelStreamItem::Usage(UsageDelta {
                usage: self.usage.clone(),
            }));
        }
        Ok(items)
    }

    /// Finish after the HTTP body ends without a `[DONE]` sentinel.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when the stream never completed.
    pub fn finish_after_body(mut self) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finish_done()
    }

    fn consume_delta(&mut self, delta: WireDelta) -> Vec<ModelStreamItem> {
        let mut items = Vec::new();
        if let Some(text) = delta.content.filter(|value| !value.is_empty()) {
            self.text.push_str(&text);
            if !self.structured {
                items.push(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from(text.as_str()),
                }));
            }
        }
        if let Some(thinking) = delta.reasoning_content.filter(|value| !value.is_empty()) {
            items.push(ModelStreamItem::ReasoningDelta(ReasoningDelta {
                text: Arc::from(thinking.as_str()),
            }));
        }
        for tool in delta.tool_calls {
            items.extend(self.consume_tool(tool));
        }
        items
    }

    fn consume_tool(&mut self, tool: WireToolCallDelta) -> Vec<ModelStreamItem> {
        let index = tool.index.unwrap_or(0);
        let entry = self.tools.entry(index).or_insert_with(|| ToolAssembly {
            call_id: None,
            name: None,
            arguments: String::new(),
        });
        if let Some(id) = tool.id.filter(|value| !value.is_empty()) {
            entry.call_id = Some(id);
        }
        let function = tool.function.unwrap_or(WireFunctionDelta {
            name: None,
            arguments: None,
        });
        if let Some(name) = function.name.filter(|value| !value.is_empty()) {
            entry.name = Some(name);
        }
        let fragment = function.arguments.unwrap_or_default();
        if !fragment.is_empty() {
            entry.arguments.push_str(&fragment);
        }
        vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
            index,
            name: entry.name.as_deref().map(Arc::from),
            arguments_delta: Arc::from(fragment.as_str()),
            provider_call_id: entry.call_id.as_deref().map(Arc::from),
        })]
    }

    fn finish_done(&mut self) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let completion_id = self
            .completion_id
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StreamNormError::response("completion ID is empty"))?;
        let assistant_content: Arc<[ContentBlock]> = if self.structured {
            let value = RawJson::parse(self.text.as_bytes())
                .map_err(|_| StreamNormError::response("structured output is not valid JSON"))?;
            Arc::from([ContentBlock::Json(JsonBlock::new(value))])
        } else if self.text.is_empty() {
            Arc::from([])
        } else {
            Arc::from([ContentBlock::Text(
                TextBlock::try_new(self.text.clone()).map_err(|_| {
                    StreamNormError::response("assistant text exceeds the kernel bound")
                })?,
            )])
        };
        let mut tool_calls = Vec::with_capacity(self.tools.len());
        for (expected, (index, tool)) in (0_u32..).zip(&self.tools) {
            if expected != *index {
                return Err(StreamNormError::response(
                    "tool-call indices are not contiguous",
                ));
            }
            let name = tool
                .name
                .as_deref()
                .ok_or_else(|| StreamNormError::response("tool call omitted its name"))?;
            let arguments = if tool.arguments.is_empty() {
                RawJson::parse(b"{}").map_err(|_| {
                    StreamNormError::response("tool-call arguments are invalid JSON")
                })?
            } else {
                RawJson::parse(tool.arguments.as_bytes()).map_err(|_| {
                    StreamNormError::response("tool-call arguments are invalid JSON")
                })?
            };
            tool_calls.push(ModelToolCall {
                name: Arc::from(name),
                arguments,
                provider_call_id: tool.call_id.as_deref().map(Arc::from),
            });
        }
        let provider_ids = ProviderIds::try_new(
            Some(self.request_id.as_str()),
            Some(completion_id.as_str()),
            None::<&str>,
        )
        .map_err(|_| StreamNormError::response("provider identifiers are invalid"))?;
        Ok(vec![ModelStreamItem::Completed(ModelResponse {
            assistant_content,
            tool_calls: tool_calls.into(),
            usage: self.usage.clone(),
            provider_ids,
            completion_id: Arc::from(completion_id),
            continuation_state: None,
        })])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_usage_and_done() {
        let mut assembly = OpenAiChatAssembly::new("request-1".to_owned(), false);
        let items = assembly
            .consume(
                r#"{"id":"chat-text-1","choices":[{"index":0,"delta":{"content":"hello "},"finish_reason":null}],"usage":null}"#,
            )
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::TextDelta(_)));
        assembly
            .consume(
                r#"{"id":"chat-text-1","choices":[{"index":0,"delta":{"content":"world"},"finish_reason":"stop"}],"usage":null}"#,
            )
            .unwrap();
        let items = assembly
            .consume(
                r#"{"id":"chat-text-1","choices":[],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#,
            )
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::Usage(_)));
        let items = assembly.consume("[DONE]").unwrap();
        let ModelStreamItem::Completed(response) = &items[0] else {
            panic!("expected completed item");
        };
        assert_eq!(response.completion_id.as_ref(), "chat-text-1");
        assert_eq!(response.usage.total_tokens(), Some(6));
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Text(_)
        ));
    }
}
