//! Ollama `/api/chat` NDJSON event assembly.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use finstack_ai_kernel::{ContentBlock, JsonBlock, ProviderIds, RawJson, TextBlock, Usage};
use finstack_ai_runtime::ports::model::{
    ModelResponse, ModelStreamItem, ModelToolCall, ReasoningDelta, TextDelta, ToolCallDelta,
    UsageDelta,
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
///
/// Serialized inside the `assistant_replay` array of the Ollama continuation
/// envelope; both fields default when absent so an older envelope still parses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct OllamaReplayEntry {
    /// Thinking text from a prior assistant turn.
    #[serde(default)]
    pub thinking: String,
    /// Optional content digest from a prior assistant turn.
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Deserialize)]
struct ContinuationEnvelope {
    provider: String,
    version: u32,
    assistant_replay: Vec<OllamaReplayEntry>,
}

/// Decode an Ollama continuation envelope into its replay entries.
///
/// Returns `None` when the state is not a well-formed envelope or was issued
/// by a different provider or envelope version, so a leaf silently starts a
/// fresh conversation instead of replaying foreign state. Matching the
/// entries against the outgoing assistant turns stays with the leaf.
///
/// ```
/// use finstack_ai_kernel::RawJson;
/// use finstack_ai_provider_wire::parse_ollama_continuation;
///
/// let state = RawJson::parse(
///     r#"{"provider":"ollama.api_chat","version":1,"assistant_replay":[{"thinking":"t"}]}"#,
/// )
/// .expect("state");
/// let entries = parse_ollama_continuation(&state).expect("envelope");
/// assert_eq!(entries[0].thinking, "t");
/// assert_eq!(entries[0].digest, None);
///
/// let foreign = RawJson::parse(r#"{"provider":"other","version":1,"assistant_replay":[]}"#)
///     .expect("state");
/// assert!(parse_ollama_continuation(&foreign).is_none());
/// ```
#[must_use]
pub fn parse_ollama_continuation(state: &RawJson) -> Option<Vec<OllamaReplayEntry>> {
    let envelope: ContinuationEnvelope = serde_json::from_slice(state.as_bytes()).ok()?;
    if envelope.provider != CONTINUATION_PROVIDER || envelope.version != CONTINUATION_VERSION {
        return None;
    }
    Some(envelope.assistant_replay)
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
                TextBlock::try_new(self.text.as_str()).map_err(|_| {
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
                b"{}".as_slice()
            } else {
                tool.arguments.as_bytes()
            };
            let arguments = RawJson::parse(arguments)
                .map_err(|_| StreamNormError::response("tool-call arguments are invalid JSON"))?;
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
            digest: Some(ollama_content_digest(&self.text, &digest_tools)?),
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
        let delta: Arc<str> = match self.tools.get(&index) {
            None => Arc::from(arguments.as_str()),
            Some(existing) if existing.name != name => {
                return Err(StreamNormError::response(
                    "tool-call name changed within one stream",
                ));
            }
            Some(existing) if existing.arguments == arguments => return Ok(Vec::new()),
            Some(existing) => match arguments.strip_prefix(existing.arguments.as_str()) {
                Some(suffix) => Arc::from(suffix),
                None => {
                    return Err(StreamNormError::response(
                        "tool-call arguments changed within one stream",
                    ));
                }
            },
        };
        let item = ModelStreamItem::ToolCallDelta(ToolCallDelta {
            index,
            name: Some(Arc::from(name.as_str())),
            arguments_delta: delta,
            provider_call_id: None,
        });
        self.tools.insert(index, ToolAssembly { name, arguments });
        Ok(vec![item])
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

/// Digest of one assistant turn's visible content for continuation matching.
///
/// `text` is the concatenated text and JSON block content; each tool call is
/// its `(name, canonical arguments JSON)` pair in emission order. The digest
/// is the kernel digest of the canonical JSON
/// `{"text":..., "tool_calls":[{"arguments":..., "name":...}, ...]}`, so the
/// assembler and the request translator compute byte-identical values.
///
/// # Errors
///
/// Returns a [`StreamNormKind::Response`](crate::StreamNormKind::Response)
/// failure when the digest document cannot be encoded as canonical JSON.
pub fn ollama_content_digest(
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
    fn ollama_replay_entry_roundtrips() {
        let entries = vec![
            OllamaReplayEntry {
                thinking: "first".to_owned(),
                digest: Some("abc".to_owned()),
            },
            OllamaReplayEntry {
                thinking: String::new(),
                digest: None,
            },
        ];
        let state = encode_continuation(&entries).unwrap().unwrap();
        assert_eq!(parse_ollama_continuation(&state).unwrap(), entries);

        // Absent fields default like the leaf's `ReplayEntry`.
        let sparse =
            RawJson::parse(r#"{"provider":"ollama.api_chat","version":1,"assistant_replay":[{}]}"#)
                .unwrap();
        assert_eq!(
            parse_ollama_continuation(&sparse).unwrap(),
            vec![OllamaReplayEntry::default()]
        );

        // Provider and version mismatches are rejected rather than replayed.
        for state in [
            r#"{"provider":"other","version":1,"assistant_replay":[]}"#,
            r#"{"provider":"ollama.api_chat","version":2,"assistant_replay":[]}"#,
            r#"{"provider":"ollama.api_chat","version":1}"#,
            "[]",
        ] {
            assert!(parse_ollama_continuation(&RawJson::parse(state).unwrap()).is_none());
        }
    }

    #[test]
    fn ollama_content_digest_matches_response_digest() {
        // The leaf's `content_digest` form: tool calls as `Value` objects.
        fn leaf_content_digest(text: &str, tool_calls: &[Value]) -> String {
            let value = json!({
                "text": text,
                "tool_calls": tool_calls,
            });
            let bytes = serde_json::to_vec(&value).unwrap();
            RawJson::parse(&bytes).unwrap().digest().to_hex()
        }

        let text = "Here is the \"result\": \u{e9}";
        let tool_calls = [
            (
                "lookup".to_owned(),
                r#"{"query":"weather in Paris","units":"metric"}"#.to_owned(),
            ),
            ("noop".to_owned(), "{}".to_owned()),
        ];
        let leaf_calls = tool_calls
            .iter()
            .map(|(name, arguments)| json!({ "name": name, "arguments": arguments }))
            .collect::<Vec<_>>();

        let shared = ollama_content_digest(text, &tool_calls).unwrap();
        assert_eq!(shared, leaf_content_digest(text, &leaf_calls));
        assert_eq!(shared, ollama_content_digest(text, &tool_calls).unwrap());

        let empty = ollama_content_digest("", &[]).unwrap();
        assert_eq!(empty, leaf_content_digest("", &[]));
        assert_ne!(empty, shared);
    }

    #[test]
    fn finish_records_the_shared_digest_in_the_continuation() {
        let mut assembly = OllamaChatAssembly::new("request-1".to_owned(), false, None);
        assembly
            .consume(r#"{"message":{"content":"hi","thinking":"why","tool_calls":[{"function":{"name":"lookup","arguments":{"q":"x"}}}]},"done":true}"#)
            .unwrap();
        let response = assembly.finish().unwrap();
        let state = response.continuation_state.unwrap();
        let entries = parse_ollama_continuation(&state).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].thinking, "why");
        let expected =
            ollama_content_digest("hi", &[("lookup".to_owned(), r#"{"q":"x"}"#.to_owned())])
                .unwrap();
        assert_eq!(entries[0].digest.as_deref(), Some(expected.as_str()));
    }

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
