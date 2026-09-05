//! Anthropic Messages SSE event assembly.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use finstack_ai_kernel::{
    ContentBlock, JsonBlock, LimitKey, OpaqueBlock, OpaquePayload, ProviderIds, RawJson, TextBlock,
    Usage,
};
use finstack_ai_runtime::ports::model::{
    ModelResponse, ModelStreamItem, ModelToolCall, ReasoningDelta, TextDelta, ToolCallDelta,
    UsageDelta,
};

use super::StreamNormError;

const THINKING_SIGNATURE_MEDIA_TYPE: &str = "application/vnd.finstack.anthropic.thinking-signature";
const CACHE_CREATION_KEY: &str = "anthropic.cache_creation_input_tokens";
const CACHE_READ_KEY: &str = "anthropic.cache_read_input_tokens";

#[derive(Deserialize)]
struct WireEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: Option<WireMessageStart>,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    content_block: Option<WireContentBlock>,
    #[serde(default)]
    delta: Option<WireDelta>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireMessageStart {
    id: Option<String>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
}

#[derive(Deserialize)]
struct WireDelta {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
}

#[derive(Deserialize)]
#[allow(
    clippy::struct_field_names,
    reason = "private wire fields match the Anthropic usage object"
)]
struct WireUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

enum OpenBlock {
    Text,
    Thinking,
    Tool { stream_index: u32 },
    Redacted,
}

struct ToolAssembly {
    name: String,
    arguments: String,
}

/// Incremental Anthropic Messages assembler.
pub struct AnthropicMessagesAssembly {
    request_id: String,
    completion_id: Option<String>,
    text: String,
    thinking_signature: Option<String>,
    tools: BTreeMap<u32, ToolAssembly>,
    next_tool_index: u32,
    blocks: BTreeMap<u32, OpenBlock>,
    usage: Usage,
    structured: bool,
}

impl AnthropicMessagesAssembly {
    /// Start a new Messages assembly for one committed request.
    #[must_use]
    pub fn new(request_id: String, structured: bool) -> Self {
        Self {
            request_id,
            completion_id: None,
            text: String::new(),
            thinking_signature: None,
            tools: BTreeMap::new(),
            next_tool_index: 0,
            blocks: BTreeMap::new(),
            usage: Usage::empty(),
            structured,
        }
    }

    /// Consume one named SSE event.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when the event name or payload is invalid.
    pub fn consume(
        &mut self,
        name: &str,
        data: &str,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        match name {
            "ping" => Ok(Vec::new()),
            "error" => Err(StreamNormError::stream("stream reported an error")),
            "message_stop" => self
                .finish()
                .map(|response| vec![ModelStreamItem::Completed(response)]),
            "message_start"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
            | "message_delta" => self.consume_named(name, data),
            _ => Err(StreamNormError::stream("SSE event name is unknown")),
        }
    }

    fn consume_named(
        &mut self,
        name: &str,
        data: &str,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let event: WireEvent = serde_json::from_str(data)
            .map_err(|_| StreamNormError::stream("SSE event is invalid JSON"))?;
        if event.kind != name {
            return Err(StreamNormError::stream(
                "SSE event name does not match its payload",
            ));
        }
        match name {
            "message_start" => self.consume_message_start(event),
            "content_block_start" => self.consume_block_start(event),
            "content_block_delta" => self.consume_block_delta(event),
            "content_block_stop" => self.consume_block_stop(&event),
            "message_delta" => self.consume_message_delta(event),
            _ => Err(StreamNormError::stream("SSE event name is unknown")),
        }
    }

    fn consume_message_start(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let message = event
            .message
            .ok_or_else(|| StreamNormError::response("message_start omitted its message"))?;
        let id = message
            .id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StreamNormError::response("completion ID is empty"))?;
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
        if let Some(usage) = message.usage {
            return self.apply_usage(&usage);
        }
        Ok(Vec::new())
    }

    fn consume_block_start(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let index = event
            .index
            .ok_or_else(|| StreamNormError::response("content block omitted its index"))?;
        let block = event
            .content_block
            .ok_or_else(|| StreamNormError::response("content_block_start omitted its block"))?;
        let open = match block.kind.as_str() {
            "text" => OpenBlock::Text,
            "thinking" => OpenBlock::Thinking,
            "redacted_thinking" => OpenBlock::Redacted,
            "tool_use" => {
                let name = block
                    .name
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| StreamNormError::response("tool_use omitted its name"))?;
                if block.id.is_none_or(|value| value.is_empty()) {
                    return Err(StreamNormError::response("tool_use omitted its id"));
                }
                let arguments = block.input.map_or_else(String::new, |value| {
                    if value == Value::Object(serde_json::Map::new()) {
                        String::new()
                    } else {
                        value.to_string()
                    }
                });
                let stream_index = self.next_tool_index;
                self.next_tool_index = self
                    .next_tool_index
                    .checked_add(1)
                    .ok_or_else(|| StreamNormError::response("tool-call index overflowed"))?;
                let item = ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: stream_index,
                    name: Some(Arc::from(name.as_str())),
                    arguments_delta: Arc::from(arguments.as_str()),
                    provider_call_id: None,
                });
                self.tools
                    .insert(stream_index, ToolAssembly { name, arguments });
                self.blocks.insert(index, OpenBlock::Tool { stream_index });
                return Ok(vec![item]);
            }
            _ => return Err(StreamNormError::response("content block type is unknown")),
        };
        if self.blocks.insert(index, open).is_some() {
            return Err(StreamNormError::response("content block index was reused"));
        }
        Ok(Vec::new())
    }

    fn consume_block_delta(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let index = event
            .index
            .ok_or_else(|| StreamNormError::response("content block omitted its index"))?;
        let delta = event
            .delta
            .ok_or_else(|| StreamNormError::response("content_block_delta omitted its delta"))?;
        let kind = delta
            .kind
            .as_deref()
            .ok_or_else(|| StreamNormError::response("content_block_delta omitted its type"))?;
        if !self.blocks.contains_key(&index) {
            return Err(StreamNormError::response(
                "content_block_delta referenced no block",
            ));
        }
        match kind {
            "text_delta" => {
                if !matches!(self.blocks.get(&index), Some(OpenBlock::Text)) {
                    return Err(StreamNormError::response("content_block_delta is invalid"));
                }
                let text = delta.text.unwrap_or_default();
                if text.is_empty() {
                    return Ok(Vec::new());
                }
                self.text.push_str(&text);
                if self.structured {
                    Ok(Vec::new())
                } else {
                    Ok(vec![ModelStreamItem::TextDelta(TextDelta {
                        text: Arc::from(text.as_str()),
                    })])
                }
            }
            "thinking_delta" => {
                if !matches!(self.blocks.get(&index), Some(OpenBlock::Thinking)) {
                    return Err(StreamNormError::response("content_block_delta is invalid"));
                }
                let thinking = delta.thinking.unwrap_or_default();
                if thinking.is_empty() {
                    return Ok(Vec::new());
                }
                Ok(vec![ModelStreamItem::ReasoningDelta(ReasoningDelta {
                    text: Arc::from(thinking.as_str()),
                })])
            }
            "signature_delta" => {
                if !matches!(self.blocks.get(&index), Some(OpenBlock::Thinking)) {
                    return Err(StreamNormError::response("content_block_delta is invalid"));
                }
                if let Some(signature) = delta.signature.filter(|value| !value.is_empty()) {
                    self.thinking_signature = Some(signature);
                }
                Ok(Vec::new())
            }
            "input_json_delta" => {
                let fragment = delta.partial_json.unwrap_or_default();
                let Some(OpenBlock::Tool { stream_index }) = self.blocks.get(&index) else {
                    return Err(StreamNormError::response("content_block_delta is invalid"));
                };
                let Some(tool) = self.tools.get_mut(stream_index) else {
                    return Err(StreamNormError::response("content_block_delta is invalid"));
                };
                tool.arguments.push_str(&fragment);
                Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: *stream_index,
                    name: Some(Arc::from(tool.name.as_str())),
                    arguments_delta: Arc::from(fragment.as_str()),
                    provider_call_id: None,
                })])
            }
            _ => Err(StreamNormError::response("content_block_delta is invalid")),
        }
    }

    fn consume_block_stop(
        &mut self,
        event: &WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let index = event
            .index
            .ok_or_else(|| StreamNormError::response("content block omitted its index"))?;
        self.blocks
            .remove(&index)
            .ok_or_else(|| StreamNormError::response("content_block_stop referenced no block"))?;
        Ok(Vec::new())
    }

    fn consume_message_delta(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        if let Some(usage) = event.usage {
            return self.apply_usage(&usage);
        }
        Ok(Vec::new())
    }

    fn apply_usage(&mut self, usage: &WireUsage) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let mut counters = BTreeMap::new();
        if let Some(value) = usage.cache_creation_input_tokens.filter(|value| *value > 0) {
            counters.insert(
                finstack_ai_kernel::static_key!(LimitKey, CACHE_CREATION_KEY),
                value,
            );
        }
        if let Some(value) = usage.cache_read_input_tokens.filter(|value| *value > 0) {
            counters.insert(
                finstack_ai_kernel::static_key!(LimitKey, CACHE_READ_KEY),
                value,
            );
        }
        let input_tokens = usage.input_tokens.or(self.usage.input_tokens());
        let output_tokens = usage.output_tokens.or(self.usage.output_tokens());
        let total_tokens = match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => input.checked_add(output),
            _ => None,
        };
        if counters.is_empty() {
            counters.clone_from(self.usage.extension_counters());
        } else {
            for (key, value) in self.usage.extension_counters() {
                counters.entry(key.clone()).or_insert(*value);
            }
        }
        self.usage = Usage::try_new(input_tokens, output_tokens, total_tokens, None, counters)
            .map_err(|_| StreamNormError::response("usage is invalid"))?;
        Ok(vec![ModelStreamItem::Usage(UsageDelta {
            usage: self.usage.clone(),
        })])
    }

    fn finish(&self) -> Result<ModelResponse, StreamNormError> {
        let completion_id = self
            .completion_id
            .clone()
            .ok_or_else(|| StreamNormError::response("stream omitted its completion ID"))?;
        let mut assistant = Vec::new();
        if let Some(signature) = &self.thinking_signature {
            let payload = OpaquePayload::json(
                RawJson::parse(
                    serde_json::to_vec(&serde_json::json!({ "signature": signature }))
                        .map_err(|_| StreamNormError::response("thinking signature is invalid"))?
                        .as_slice(),
                )
                .map_err(|_| StreamNormError::response("thinking signature is invalid"))?,
            );
            assistant.push(ContentBlock::Opaque(
                OpaqueBlock::try_new(THINKING_SIGNATURE_MEDIA_TYPE, payload)
                    .map_err(|_| StreamNormError::response("thinking signature is invalid"))?,
            ));
        }
        if self.structured && !self.text.is_empty() {
            let value = RawJson::parse(self.text.as_bytes())
                .map_err(|_| StreamNormError::response("structured output is not valid JSON"))?;
            assistant.push(ContentBlock::Json(JsonBlock::new(value)));
        } else if !self.text.is_empty() {
            assistant.push(ContentBlock::Text(
                TextBlock::try_new(self.text.as_str()).map_err(|_| {
                    StreamNormError::response("assistant text exceeds the kernel bound")
                })?,
            ));
        }
        let mut tool_calls = Vec::with_capacity(self.tools.len());
        for (expected, (index, tool)) in (0_u32..).zip(&self.tools) {
            if expected != *index {
                return Err(StreamNormError::response(
                    "tool-call indices are not contiguous",
                ));
            }
            let arguments = if tool.arguments.is_empty() {
                b"{}".as_slice()
            } else {
                tool.arguments.as_bytes()
            };
            let arguments = RawJson::parse(arguments)
                .map_err(|_| StreamNormError::response("tool-call arguments are invalid JSON"))?;
            tool_calls.push(ModelToolCall {
                name: Arc::from(tool.name.as_str()),
                arguments,
                provider_call_id: None,
            });
        }
        let provider_ids = ProviderIds::try_new(
            Some(self.request_id.as_str()),
            Some(completion_id.as_str()),
            None::<&str>,
        )
        .map_err(|_| StreamNormError::response("provider identifiers are invalid"))?;
        Ok(ModelResponse {
            assistant_content: assistant.into(),
            tool_calls: tool_calls.into(),
            usage: self.usage.clone(),
            provider_ids,
            completion_id: Arc::from(completion_id),
            continuation_state: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_and_usage() {
        let mut assembly = AnthropicMessagesAssembly::new("request-1".to_owned(), false);
        assembly
            .consume(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg-1","usage":{"input_tokens":4,"output_tokens":1}}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap();
        let items = assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
            )
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::TextDelta(_)));
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            )
            .unwrap();
        let items = assembly
            .consume("message_stop", r#"{"type":"message_stop"}"#)
            .unwrap();
        let ModelStreamItem::Completed(response) = &items[0] else {
            panic!("expected completed item");
        };
        assert_eq!(response.completion_id.as_ref(), "msg-1");
    }
}
