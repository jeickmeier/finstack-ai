//! Ollama `/api/chat` NDJSON event assembly.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ContentBlock, JsonBlock, ModelResponse, ModelStreamItem, ModelToolCall, ProviderIds, RawJson,
    ReasoningDelta, TextBlock, TextDelta, ToolCallDelta, Usage, UsageDelta,
};

use super::StreamNormError;

const CONTINUATION_PROVIDER: &str = "ollama.api_chat";
const CONTINUATION_VERSION: u32 = 1;

#[derive(Deserialize)]
struct WireChunk {
    #[serde(default)]
    message: Option<WireMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Deserialize)]
struct WireToolCall {
    function: Option<WireFunctionCall>,
}

#[derive(Deserialize)]
struct WireFunctionCall {
    name: Option<String>,
    arguments: Option<Value>,
}

struct ToolAssembly {
    name: String,
    arguments: String,
}

/// Prior assistant replay used to preserve thinking across turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OllamaReplayEntry {
    /// Thinking text from a prior assistant turn.
    pub thinking: String,
    /// Optional content digest from a prior assistant turn.
    pub digest: Option<String>,
}

/// Incremental Ollama chat assembler.
pub struct OllamaChatAssembly {
    request_id: String,
    text: String,
    thinking: String,
    tools: BTreeMap<u32, ToolAssembly>,
    usage: Usage,
    structured: bool,
    /// Whether `done: true` has been observed.
    pub done: bool,
    matched_replay: Option<Vec<OllamaReplayEntry>>,
}

impl OllamaChatAssembly {
    /// Start a new Ollama assembly for one committed request.
    #[must_use]
    pub fn new(
        request_id: String,
        structured: bool,
        matched_replay: Option<Vec<OllamaReplayEntry>>,
    ) -> Self {
        Self {
            request_id,
            text: String::new(),
            thinking: String::new(),
            tools: BTreeMap::new(),
            usage: Usage::empty(),
            structured,
            done: false,
            matched_replay,
        }
    }

    /// Consume one NDJSON object line.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when the line is malformed.
    pub fn consume(&mut self, line: &str) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let chunk: WireChunk = serde_json::from_str(line)
            .map_err(|_| StreamNormError::stream("NDJSON line is invalid JSON"))?;
        let mut items = Vec::new();
        if let Some(message) = chunk.message {
            if let Some(text) = message.content.filter(|value| !value.is_empty()) {
                self.text.push_str(&text);
                if !self.structured {
                    items.push(ModelStreamItem::TextDelta(TextDelta {
                        text: Arc::from(text.as_str()),
                    }));
                }
            }
            if let Some(thinking) = message.thinking.filter(|value| !value.is_empty()) {
                self.thinking.push_str(&thinking);
                items.push(ModelStreamItem::ReasoningDelta(ReasoningDelta {
                    text: Arc::from(thinking.as_str()),
                }));
            }
            for (index, tool) in (0_u32..).zip(message.tool_calls) {
                items.extend(self.consume_tool(index, tool)?);
            }
        }
        if chunk.prompt_eval_count.is_some() || chunk.eval_count.is_some() {
            items.extend(self.apply_usage(chunk.prompt_eval_count, chunk.eval_count)?);
        }
        if chunk.done {
            self.done = true;
        }
        Ok(items)
    }

    /// Complete the response after `done: true`.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when the stream never completed or the payload is invalid.
    pub fn finish(self) -> Result<ModelResponse, StreamNormError> {
        if !self.done {
            return Err(StreamNormError::stream(
                "NDJSON stream ended before done:true",
            ));
        }
        let assistant_content: Arc<[ContentBlock]> = if self.structured && !self.text.is_empty() {
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
        let mut digest_tools = Vec::with_capacity(self.tools.len());
        for (expected, (index, tool)) in (0_u32..).zip(self.tools) {
            if expected != index {
                return Err(StreamNormError::response(
                    "tool-call indices are not contiguous",
                ));
            }
            let arguments = if tool.arguments.is_empty() {
                RawJson::parse(b"{}").map_err(|_| {
                    StreamNormError::response("tool-call arguments are invalid JSON")
                })?
            } else {
                RawJson::parse(tool.arguments.as_bytes()).map_err(|_| {
                    StreamNormError::response("tool-call arguments are invalid JSON")
                })?
            };
            digest_tools.push((tool.name.clone(), arguments.as_str().to_owned()));
            tool_calls.push(ModelToolCall {
                name: Arc::from(tool.name),
                arguments,
                provider_call_id: None,
            });
        }
        let mut replay = self.matched_replay.unwrap_or_default();
        replay.push(OllamaReplayEntry {
            thinking: self.thinking,
            digest: Some(response_content_digest(&self.text, &digest_tools)?),
        });
        let continuation_state = encode_continuation(&replay)?;
        let provider_ids =
            ProviderIds::try_new(Some(self.request_id.as_str()), None::<&str>, None::<&str>)
                .map_err(|_| StreamNormError::response("provider identifiers are invalid"))?;
        Ok(ModelResponse {
            assistant_content,
            tool_calls: tool_calls.into(),
            usage: self.usage,
            provider_ids,
            completion_id: Arc::from(self.request_id),
            continuation_state,
        })
    }

    fn consume_tool(
        &mut self,
        index: u32,
        tool: WireToolCall,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let function = tool
            .function
            .ok_or_else(|| StreamNormError::response("tool call omitted its function"))?;
        let name = function
            .name
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StreamNormError::response("tool call omitted its name"))?;
        let arguments = encode_arguments(function.arguments)?;
        let previous = self
            .tools
            .get(&index)
            .map(|existing| (existing.name.clone(), existing.arguments.clone()));
        match previous {
            Some((existing_name, existing_args))
                if existing_name == name && existing_args == arguments =>
            {
                return Ok(Vec::new());
            }
            Some((existing_name, _)) if existing_name != name => {
                return Err(StreamNormError::response(
                    "tool-call name changed within one stream",
                ));
            }
            Some((_, existing_args)) if arguments.starts_with(&existing_args) => {
                let suffix = arguments[existing_args.len()..].to_owned();
                self.tools.insert(
                    index,
                    ToolAssembly {
                        name: name.clone(),
                        arguments,
                    },
                );
                return Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index,
                    name: Some(Arc::from(name.as_str())),
                    arguments_delta: Arc::from(suffix.as_str()),
                    provider_call_id: None,
                })]);
            }
            Some(_) => {
                return Err(StreamNormError::response(
                    "tool-call arguments changed within one stream",
                ));
            }
            None => {}
        }
        self.tools.insert(
            index,
            ToolAssembly {
                name: name.clone(),
                arguments: arguments.clone(),
            },
        );
        Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
            index,
            name: Some(Arc::from(name.as_str())),
            arguments_delta: Arc::from(arguments.as_str()),
            provider_call_id: None,
        })])
    }

    fn apply_usage(
        &mut self,
        prompt_eval_count: Option<u64>,
        eval_count: Option<u64>,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let input_tokens = prompt_eval_count.or(self.usage.input_tokens());
        let output_tokens = eval_count.or(self.usage.output_tokens());
        let total_tokens = match (input_tokens, output_tokens) {
            (Some(input), Some(output)) => input.checked_add(output),
            _ => None,
        };
        self.usage = Usage::try_new(
            input_tokens,
            output_tokens,
            total_tokens,
            None,
            BTreeMap::new(),
        )
        .map_err(|_| StreamNormError::response("usage is invalid"))?;
        Ok(vec![ModelStreamItem::Usage(UsageDelta {
            usage: self.usage.clone(),
        })])
    }
}

fn encode_arguments(arguments: Option<Value>) -> Result<String, StreamNormError> {
    match arguments {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value),
        Some(value) => serde_json::to_string(&value)
            .map_err(|_| StreamNormError::response("tool-call arguments are invalid JSON")),
    }
}

fn response_content_digest(
    text: &str,
    tool_calls: &[(String, String)],
) -> Result<String, StreamNormError> {
    let values = tool_calls
        .iter()
        .map(|(name, arguments)| json!({ "name": name, "arguments": arguments }))
        .collect::<Vec<_>>();
    let value = json!({
        "text": text,
        "tool_calls": values,
    });
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| StreamNormError::response("assistant content digest could not be encoded"))?;
    let raw = RawJson::parse(&bytes)
        .map_err(|_| StreamNormError::response("assistant content digest is not canonical JSON"))?;
    Ok(raw.digest().to_hex())
}

fn encode_continuation(entries: &[OllamaReplayEntry]) -> Result<Option<RawJson>, StreamNormError> {
    if entries.iter().all(|entry| entry.thinking.is_empty()) {
        return Ok(None);
    }
    let envelope = json!({
        "provider": CONTINUATION_PROVIDER,
        "version": CONTINUATION_VERSION,
        "assistant_replay": entries.iter().map(|entry| {
            json!({
                "thinking": entry.thinking,
                "digest": entry.digest,
            })
        }).collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|_| StreamNormError::response("provider continuation could not be encoded"))?;
    RawJson::parse(&bytes)
        .map(Some)
        .map_err(|_| StreamNormError::response("provider continuation is not canonical JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_and_done() {
        let mut assembly = OllamaChatAssembly::new("request-1".to_owned(), false, None);
        let items = assembly
            .consume(r#"{"message":{"content":"hello "},"done":false}"#)
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::TextDelta(_)));
        assembly
            .consume(r#"{"message":{"content":"world"},"done":false}"#)
            .unwrap();
        let items = assembly
            .consume(r#"{"message":{},"done":true,"prompt_eval_count":4,"eval_count":2}"#)
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::Usage(_)));
        let response = assembly.finish().unwrap();
        assert_eq!(response.completion_id.as_ref(), "request-1");
        assert_eq!(response.usage.total_tokens(), Some(6));
    }
}
