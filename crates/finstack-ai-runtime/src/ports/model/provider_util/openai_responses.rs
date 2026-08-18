//! Official `OpenAI` Responses SSE event assembly.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ContentBlock, JsonBlock, ModelResponse, ModelStreamItem, ModelToolCall, ProviderIds, RawJson,
    ReasoningDelta, TextBlock, TextDelta, ToolCallDelta, Usage, UsageDelta,
};

use super::StreamNormError;

const CONTINUATION_PROVIDER: &str = "openai.responses";
const CONTINUATION_VERSION: u64 = 1;

#[derive(Deserialize)]
struct WireEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    sequence_number: Option<u64>,
    #[serde(default)]
    delta: Option<Value>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    output_index: Option<u32>,
    #[serde(default)]
    item: Option<WireOutputItem>,
    #[serde(default)]
    response: Option<WireResponse>,
}

#[derive(Deserialize)]
struct WireOutputItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct WireResponse {
    id: Option<String>,
    #[serde(default)]
    output: Vec<Value>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
#[allow(
    clippy::struct_field_names,
    reason = "private wire fields match the OpenAI Responses usage object"
)]
struct WireUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

struct ToolAssembly {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// Incremental Responses-stream assembler.
pub struct OpenAiResponsesAssembly {
    request_id: String,
    structured: bool,
    last_sequence: Option<u64>,
    completion_id: Option<String>,
    text: String,
    tools: BTreeMap<u32, ToolAssembly>,
    output_to_stream: BTreeMap<u32, u32>,
    next_stream_index: u32,
    usage: Usage,
}

impl OpenAiResponsesAssembly {
    /// Start a new Responses assembly for one committed request.
    #[must_use]
    pub fn new(request_id: String, structured: bool) -> Self {
        Self {
            request_id,
            structured,
            last_sequence: None,
            completion_id: None,
            text: String::new(),
            tools: BTreeMap::new(),
            output_to_stream: BTreeMap::new(),
            next_stream_index: 0,
            usage: Usage::empty(),
        }
    }

    /// Consume one JSON event payload and emit normalized stream items.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when the event is malformed or incomplete.
    pub fn consume(&mut self, data: &str) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let event: WireEvent = serde_json::from_str(data)
            .map_err(|_| StreamNormError::stream("SSE event is invalid JSON"))?;
        self.observe_sequence(event.sequence_number)?;
        match event.kind.as_str() {
            "error" => Err(StreamNormError::stream("stream reported an error")),
            "response.failed" => Err(StreamNormError::response("response failed")),
            "response.incomplete" => Err(StreamNormError::incomplete("response was incomplete")),
            "response.completed" => self.consume_completed(event),
            "response.output_text.delta" => Ok(self.consume_text_delta(&event)),
            "response.function_call_arguments.delta" => self.consume_arguments_delta(&event),
            "response.function_call_arguments.done" => self.consume_arguments_done(event),
            "response.output_item.added" => self.consume_output_item_added(event),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                Ok(reasoning_delta(&event))
            }
            _ => Ok(Vec::new()),
        }
    }

    fn observe_sequence(&mut self, sequence_number: Option<u64>) -> Result<(), StreamNormError> {
        let Some(sequence) = sequence_number else {
            return Ok(());
        };
        if self.last_sequence.is_some_and(|last| sequence <= last) {
            return Err(StreamNormError::stream(
                "SSE sequence_number is not monotonic",
            ));
        }
        self.last_sequence = Some(sequence);
        Ok(())
    }

    fn consume_text_delta(&mut self, event: &WireEvent) -> Vec<ModelStreamItem> {
        let Some(text) = string_delta(event.delta.as_ref()) else {
            return Vec::new();
        };
        if text.is_empty() {
            return Vec::new();
        }
        self.text.push_str(text);
        if self.structured {
            Vec::new()
        } else {
            vec![ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            })]
        }
    }

    fn consume_output_item_added(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let Some(item) = event.item else {
            return Ok(Vec::new());
        };
        if item.kind.as_deref() != Some("function_call") {
            return Ok(Vec::new());
        }
        let output_index = event.output_index.unwrap_or(self.next_stream_index);
        let stream_index = self.allocate_stream_index(output_index)?;
        let tool = self
            .tools
            .entry(stream_index)
            .or_insert_with(|| ToolAssembly {
                call_id: None,
                name: None,
                arguments: String::new(),
            });
        if let Some(call_id) = item.call_id.filter(|value| !value.is_empty()) {
            tool.call_id = Some(call_id);
        }
        if let Some(name) = item.name.filter(|value| !value.is_empty()) {
            tool.name = Some(name);
        }
        let arguments = item.arguments.unwrap_or_default();
        if !arguments.is_empty() {
            tool.arguments.push_str(&arguments);
        }
        Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
            index: stream_index,
            name: tool.name.as_deref().map(Arc::from),
            arguments_delta: Arc::from(arguments.as_str()),
            provider_call_id: tool.call_id.as_deref().map(Arc::from),
        })])
    }

    fn consume_arguments_delta(
        &mut self,
        event: &WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let fragment = string_delta(event.delta.as_ref()).unwrap_or("");
        let stream_index = self.stream_index_for(event.output_index)?;
        let tool = self.tool_mut(stream_index);
        tool.arguments.push_str(fragment);
        Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
            index: stream_index,
            name: tool.name.as_deref().map(Arc::from),
            arguments_delta: Arc::from(fragment),
            provider_call_id: tool.call_id.as_deref().map(Arc::from),
        })])
    }

    fn consume_arguments_done(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let stream_index = self.stream_index_for(event.output_index)?;
        let tool = self.tool_mut(stream_index);
        if tool.arguments.is_empty() {
            let arguments = event.arguments.unwrap_or_else(|| "{}".to_owned());
            tool.arguments.clone_from(&arguments);
            return Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: stream_index,
                name: tool.name.as_deref().map(Arc::from),
                arguments_delta: Arc::from(arguments.as_str()),
                provider_call_id: tool.call_id.as_deref().map(Arc::from),
            })]);
        }
        Ok(Vec::new())
    }

    fn consume_completed(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let response = event
            .response
            .ok_or_else(|| StreamNormError::response("completed event omitted its response"))?;
        let completion_id = response
            .id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StreamNormError::response("completion ID is empty"))?;
        if self
            .completion_id
            .as_deref()
            .is_some_and(|known| known != completion_id)
        {
            return Err(StreamNormError::response(
                "completion ID changed within one stream",
            ));
        }
        self.completion_id = Some(completion_id.clone());
        let mut items = Vec::new();
        if let Some(usage) = response.usage {
            self.usage = Usage::try_new(
                usage.input_tokens,
                usage.output_tokens,
                usage.total_tokens,
                None,
                BTreeMap::new(),
            )
            .map_err(|_| StreamNormError::response("usage is invalid"))?;
            items.push(ModelStreamItem::Usage(UsageDelta {
                usage: self.usage.clone(),
            }));
        }
        let continuation_state = continuation_envelope(&response.output)?;
        let completed = self.finish(completion_id, continuation_state)?;
        items.push(ModelStreamItem::Completed(completed));
        Ok(items)
    }

    fn allocate_stream_index(&mut self, output_index: u32) -> Result<u32, StreamNormError> {
        if let Some(index) = self.output_to_stream.get(&output_index) {
            return Ok(*index);
        }
        let stream_index = self.next_stream_index;
        self.next_stream_index = self
            .next_stream_index
            .checked_add(1)
            .ok_or_else(|| StreamNormError::response("tool-call index overflowed"))?;
        self.output_to_stream.insert(output_index, stream_index);
        Ok(stream_index)
    }

    fn stream_index_for(&mut self, output_index: Option<u32>) -> Result<u32, StreamNormError> {
        let output_index = output_index.unwrap_or(0);
        if let Some(index) = self.output_to_stream.get(&output_index) {
            return Ok(*index);
        }
        self.allocate_stream_index(output_index)
    }

    fn tool_mut(&mut self, stream_index: u32) -> &mut ToolAssembly {
        self.tools
            .entry(stream_index)
            .or_insert_with(|| ToolAssembly {
                call_id: None,
                name: None,
                arguments: String::new(),
            })
    }

    fn finish(
        &self,
        completion_id: String,
        continuation_state: Option<RawJson>,
    ) -> Result<ModelResponse, StreamNormError> {
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
        Ok(ModelResponse {
            assistant_content,
            tool_calls: tool_calls.into(),
            usage: self.usage.clone(),
            provider_ids,
            completion_id: Arc::from(completion_id),
            continuation_state,
        })
    }
}

fn string_delta(delta: Option<&Value>) -> Option<&str> {
    delta.and_then(Value::as_str)
}

fn reasoning_delta(event: &WireEvent) -> Vec<ModelStreamItem> {
    let Some(text) = string_delta(event.delta.as_ref()) else {
        return Vec::new();
    };
    if text.is_empty() {
        return Vec::new();
    }
    vec![ModelStreamItem::ReasoningDelta(ReasoningDelta {
        text: Arc::from(text),
    })]
}

fn continuation_envelope(output: &[Value]) -> Result<Option<RawJson>, StreamNormError> {
    let envelope = json!({
        "provider": CONTINUATION_PROVIDER,
        "version": CONTINUATION_VERSION,
        "replay_items": output
    });
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|_| StreamNormError::response("continuation state is invalid"))?;
    RawJson::parse(bytes.as_slice())
        .map(Some)
        .map_err(|_| StreamNormError::response("continuation state is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_and_function_call_id_on_completed() {
        let mut assembly = OpenAiResponsesAssembly::new("request-1".to_owned(), false);
        let text_items = assembly
            .consume(r#"{"type":"response.output_text.delta","sequence_number":1,"delta":"hello"}"#)
            .unwrap();
        assert!(matches!(text_items[0], ModelStreamItem::TextDelta(_)));
        assembly
            .consume(
                r#"{"type":"response.output_item.added","sequence_number":2,"output_index":1,"item":{"type":"function_call","call_id":"call_abc","name":"lookup","arguments":""}}"#,
            )
            .unwrap();
        assembly
            .consume(
                r#"{"type":"response.function_call_arguments.delta","sequence_number":3,"output_index":1,"delta":"{\"x\":1}"}"#,
            )
            .unwrap();
        let items = assembly
            .consume(
                r#"{"type":"response.completed","sequence_number":4,"response":{"id":"resp-1","output":[{"type":"message"},{"arguments":"{\"x\":1}","call_id":"call_abc","name":"lookup","type":"function_call"}],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}}"#,
            )
            .unwrap();
        let ModelStreamItem::Completed(response) = &items[1] else {
            panic!("expected completed item");
        };
        assert_eq!(response.completion_id.as_ref(), "resp-1");
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Text(_)
        ));
        assert_eq!(response.tool_calls[0].name.as_ref(), "lookup");
        assert_eq!(response.tool_calls[0].arguments.as_str(), r#"{"x":1}"#);
        assert_eq!(
            response.tool_calls[0].provider_call_id.as_deref(),
            Some("call_abc")
        );
        assert_eq!(response.usage.total_tokens(), Some(5));
    }

    #[test]
    fn incomplete_and_failed_are_errors() {
        let mut assembly = OpenAiResponsesAssembly::new("request-1".to_owned(), false);
        assert_eq!(
            assembly
                .consume(r#"{"type":"response.incomplete","sequence_number":1,"response":{"id":"resp-1","incomplete_details":{"reason":"max_output_tokens"}}}"#)
                .expect_err("incomplete")
                .kind,
            super::super::StreamNormKind::Incomplete
        );
        let mut assembly = OpenAiResponsesAssembly::new("request-2".to_owned(), false);
        assert_eq!(
            assembly
                .consume(
                    r#"{"type":"response.failed","sequence_number":1,"response":{"id":"resp-2"}}"#
                )
                .expect_err("failed")
                .kind,
            super::super::StreamNormKind::Response
        );
    }

    #[test]
    fn done_sentinel_is_not_success() {
        let mut assembly = OpenAiResponsesAssembly::new("request-1".to_owned(), false);
        assert!(assembly.consume("[DONE]").is_err());
    }
}
