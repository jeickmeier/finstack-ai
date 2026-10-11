//! Anthropic Messages SSE event assembly.
//!
//! The assembler consumes named Messages SSE events and produces
//! provider-neutral stream items. Thinking content never lands in the
//! assistant content: signed `thinking` blocks and `redacted_thinking` blocks
//! are collected in content-block order and emitted as the opaque
//! continuation envelope
//! `{"provider":"anthropic.messages","version":1,"replay_blocks":[..]}` so the
//! leaf can replay them at the head of the last assistant turn. A thinking
//! block that never received a signature cannot be replayed and is dropped.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use finstack_ai_kernel::{
    ContentBlock, JsonBlock, LimitKey, ProviderIds, RawJson, TextBlock, Usage,
};
use finstack_ai_runtime::ports::model::{
    ModelResponse, ModelStreamItem, ModelToolCall, ReasoningDelta, TextDelta, ToolCallDelta,
    UsageDelta,
};

use super::StreamNormError;

const CONTINUATION_PROVIDER: &str = "anthropic.messages";
const CONTINUATION_VERSION: u64 = 1;
const CACHE_CREATION_KEY: &str = "anthropic.cache_creation_input_tokens";
const CACHE_READ_KEY: &str = "anthropic.cache_read_input_tokens";

#[derive(Deserialize)]
struct WireEvent<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    #[serde(default)]
    message: Option<WireMessageStart>,
    #[serde(default)]
    index: Option<u32>,
    #[serde(default, borrow)]
    content_block: Option<WireContentBlock<'a>>,
    #[serde(default, borrow)]
    delta: Option<WireDelta<'a>>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireMessageStart {
    id: Option<String>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireContentBlock<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    #[serde(default, borrow)]
    id: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    name: Option<Cow<'a, str>>,
    #[serde(default)]
    input: Option<Value>,
    /// Opaque payload of a `redacted_thinking` block.
    #[serde(default, borrow)]
    data: Option<Cow<'a, str>>,
}

/// Payload of both `content_block_delta.delta` and `message_delta.delta`.
///
/// The latter carries no `type`, only `stop_reason`, so `kind` stays optional.
#[derive(Deserialize)]
struct WireDelta<'a> {
    #[serde(rename = "type", default, borrow)]
    kind: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    text: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    thinking: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    signature: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    partial_json: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    stop_reason: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct WireErrorEvent<'a> {
    #[serde(default, borrow)]
    error: Option<WireErrorBody<'a>>,
}

#[derive(Deserialize)]
struct WireErrorBody<'a> {
    #[serde(rename = "type", default, borrow)]
    kind: Option<Cow<'a, str>>,
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
    Tool {
        stream_index: u32,
    },
    Redacted,
    /// A block type this assembler does not understand; its deltas are dropped.
    Ignored,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayKind {
    Thinking,
    Redacted,
}

/// One thinking-family block captured for replay, keyed by content-block index.
struct ReplayThinking {
    kind: ReplayKind,
    text: String,
    signature: Option<String>,
    data: Option<String>,
}

impl ReplayThinking {
    fn replay_block(&self) -> Option<Value> {
        match self.kind {
            ReplayKind::Thinking => self.signature.as_deref().map(|signature| {
                json!({
                    "type": "thinking",
                    "thinking": self.text,
                    "signature": signature,
                })
            }),
            ReplayKind::Redacted => self
                .data
                .as_deref()
                .map(|data| json!({ "type": "redacted_thinking", "data": data })),
        }
    }
}

struct ToolAssembly {
    name: Arc<str>,
    arguments: String,
}

/// Incremental Anthropic Messages assembler.
///
/// Unknown event names, unknown content-block types (and every delta on
/// them), and unknown delta kinds on known blocks are ignored so the adapter
/// keeps working when the API adds event vocabulary. Malformed payloads,
/// known delta kinds on the wrong block, index reuse, and `error` events
/// still fail the stream.
pub struct AnthropicMessagesAssembly {
    request_id: String,
    completion_id: Option<String>,
    text: String,
    replay: BTreeMap<u32, ReplayThinking>,
    tools: BTreeMap<u32, ToolAssembly>,
    next_tool_index: u32,
    blocks: BTreeMap<u32, OpenBlock>,
    usage: Usage,
    stop_reason: Option<String>,
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
            replay: BTreeMap::new(),
            tools: BTreeMap::new(),
            next_tool_index: 0,
            blocks: BTreeMap::new(),
            usage: Usage::empty(),
            stop_reason: None,
            structured,
        }
    }

    /// Consume one named SSE event.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when a known event's payload is invalid,
    /// the upstream service reports an `error` event, or the generation
    /// stopped at `max_tokens`.
    pub fn consume(
        &mut self,
        name: &str,
        data: &str,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        match name {
            "error" => Err(error_event(data)),
            "message_stop" => self
                .finish()
                .map(|response| vec![ModelStreamItem::Completed(response)]),
            "message_start"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
            | "message_delta" => self.consume_named(name, data),
            // `ping` and any event name this assembler does not know.
            _ => Ok(Vec::new()),
        }
    }

    fn consume_named(
        &mut self,
        name: &str,
        data: &str,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let event: WireEvent<'_> = serde_json::from_str(data)
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
            _ => Ok(Vec::new()),
        }
    }

    fn consume_message_start(
        &mut self,
        event: WireEvent<'_>,
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
        event: WireEvent<'_>,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let index = event
            .index
            .ok_or_else(|| StreamNormError::response("content block omitted its index"))?;
        let block = event
            .content_block
            .ok_or_else(|| StreamNormError::response("content_block_start omitted its block"))?;
        if self.blocks.contains_key(&index) {
            return Err(StreamNormError::response("content block index was reused"));
        }
        let mut items = Vec::new();
        let open = match block.kind.as_ref() {
            "text" => OpenBlock::Text,
            "thinking" => {
                self.replay.insert(
                    index,
                    ReplayThinking {
                        kind: ReplayKind::Thinking,
                        text: String::new(),
                        signature: None,
                        data: None,
                    },
                );
                OpenBlock::Thinking
            }
            "redacted_thinking" => {
                let data = block
                    .data
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        StreamNormError::response("redacted_thinking omitted its data")
                    })?;
                self.replay.insert(
                    index,
                    ReplayThinking {
                        kind: ReplayKind::Redacted,
                        text: String::new(),
                        signature: None,
                        data: Some(data.into_owned()),
                    },
                );
                OpenBlock::Redacted
            }
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
                let name: Arc<str> = Arc::from(name.as_ref());
                items.push(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: stream_index,
                    name: Some(Arc::clone(&name)),
                    arguments_delta: Arc::from(arguments.as_str()),
                    provider_call_id: None,
                }));
                self.tools
                    .insert(stream_index, ToolAssembly { name, arguments });
                OpenBlock::Tool { stream_index }
            }
            _ => OpenBlock::Ignored,
        };
        self.blocks.insert(index, open);
        Ok(items)
    }

    fn consume_block_delta(
        &mut self,
        event: WireEvent<'_>,
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
        let Some(block) = self.blocks.get(&index) else {
            return Err(StreamNormError::response(
                "content_block_delta referenced no block",
            ));
        };
        match (kind, block) {
            (_, OpenBlock::Ignored) => Ok(Vec::new()),
            ("text_delta", OpenBlock::Text) => {
                let text = delta.text.unwrap_or_default();
                if text.is_empty() {
                    return Ok(Vec::new());
                }
                self.text.push_str(&text);
                if self.structured {
                    Ok(Vec::new())
                } else {
                    Ok(vec![ModelStreamItem::TextDelta(TextDelta {
                        text: Arc::from(text.as_ref()),
                    })])
                }
            }
            ("thinking_delta", OpenBlock::Thinking) => {
                let thinking = delta.thinking.unwrap_or_default();
                if thinking.is_empty() {
                    return Ok(Vec::new());
                }
                if let Some(entry) = self.replay.get_mut(&index) {
                    entry.text.push_str(&thinking);
                }
                Ok(vec![ModelStreamItem::ReasoningDelta(ReasoningDelta {
                    text: Arc::from(thinking.as_ref()),
                })])
            }
            ("signature_delta", OpenBlock::Thinking) => {
                if let Some(signature) = delta.signature.filter(|value| !value.is_empty())
                    && let Some(entry) = self.replay.get_mut(&index)
                {
                    entry.signature = Some(signature.into_owned());
                }
                Ok(Vec::new())
            }
            ("input_json_delta", OpenBlock::Tool { stream_index }) => {
                let fragment = delta.partial_json.unwrap_or_default();
                let Some(tool) = self.tools.get_mut(stream_index) else {
                    return Err(StreamNormError::response("content_block_delta is invalid"));
                };
                tool.arguments.push_str(&fragment);
                Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: *stream_index,
                    name: Some(Arc::clone(&tool.name)),
                    arguments_delta: Arc::from(fragment.as_ref()),
                    provider_call_id: None,
                })])
            }
            // A known delta kind on the wrong block is a protocol violation.
            ("text_delta" | "thinking_delta" | "signature_delta" | "input_json_delta", _) => {
                Err(StreamNormError::response("content_block_delta is invalid"))
            }
            // An unknown delta kind on a known block is new vocabulary; drop it.
            _ => Ok(Vec::new()),
        }
    }

    fn consume_block_stop(
        &mut self,
        event: &WireEvent<'_>,
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
        event: WireEvent<'_>,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        if let Some(stop_reason) = event
            .delta
            .and_then(|delta| delta.stop_reason)
            .filter(|value| !value.is_empty())
        {
            self.stop_reason = Some(stop_reason.into_owned());
        }
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
        if self.stop_reason.as_deref() == Some("max_tokens") {
            return Err(StreamNormError::incomplete(
                "Anthropic generation stopped at max_tokens",
            ));
        }
        let mut assistant = Vec::new();
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
                name: Arc::clone(&tool.name),
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
            continuation_state: continuation_envelope(&self.replay)?,
        })
    }
}

/// Classify an `error` event by its `error.type` without forwarding vendor text.
fn error_event(data: &str) -> StreamNormError {
    let Ok(event) = serde_json::from_str::<WireErrorEvent<'_>>(data) else {
        return StreamNormError::stream("SSE event is invalid JSON");
    };
    let kind = event.error.and_then(|body| body.kind);
    match kind.as_deref() {
        Some("overloaded_error" | "api_error" | "rate_limit_error") => {
            StreamNormError::upstream("stream reported a retryable upstream error", true)
        }
        _ => StreamNormError::upstream("stream reported an upstream error", false),
    }
}

/// Build the replay envelope from every replayable thinking-family block.
///
/// Returns `None` when no block can be replayed (no signed thinking and no
/// redacted block), so a plain text or tool turn carries no continuation.
fn continuation_envelope(
    replay: &BTreeMap<u32, ReplayThinking>,
) -> Result<Option<RawJson>, StreamNormError> {
    let replay_blocks = replay
        .values()
        .filter_map(ReplayThinking::replay_block)
        .collect::<Vec<_>>();
    if replay_blocks.is_empty() {
        return Ok(None);
    }
    let envelope = json!({
        "provider": CONTINUATION_PROVIDER,
        "version": CONTINUATION_VERSION,
        "replay_blocks": replay_blocks,
    });
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|_| StreamNormError::response("continuation state is invalid"))?;
    RawJson::parse(bytes.as_slice())
        .map(Some)
        .map_err(|_| StreamNormError::response("continuation state is invalid"))
}

#[cfg(test)]
mod tests {
    use super::super::StreamNormKind;
    use super::*;

    fn started(request_id: &str, completion_id: &str) -> AnthropicMessagesAssembly {
        let mut assembly = AnthropicMessagesAssembly::new(request_id.to_owned(), false);
        assembly
            .consume(
                "message_start",
                &format!(
                    r#"{{"type":"message_start","message":{{"id":"{completion_id}","usage":{{"input_tokens":4,"output_tokens":1}}}}}}"#
                ),
            )
            .unwrap();
        assembly
    }

    fn complete(assembly: &mut AnthropicMessagesAssembly) -> ModelResponse {
        let items = assembly
            .consume("message_stop", r#"{"type":"message_stop"}"#)
            .unwrap();
        let Some(ModelStreamItem::Completed(response)) = items.into_iter().next() else {
            panic!("expected completed item");
        };
        response
    }

    #[test]
    fn assembles_text_and_usage() {
        let mut assembly = started("request-1", "msg-1");
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
        let response = complete(&mut assembly);
        assert_eq!(response.completion_id.as_ref(), "msg-1");
        assert!(response.continuation_state.is_none());
    }

    #[test]
    fn thinking_blocks_land_in_continuation_not_content() {
        let mut assembly = started("request-1", "msg-1");
        for (index, text, signature) in [(0, "first", "sig-a"), (1, "second", "sig-b")] {
            assembly
                .consume(
                    "content_block_start",
                    &format!(
                        r#"{{"type":"content_block_start","index":{index},"content_block":{{"type":"thinking","thinking":""}}}}"#
                    ),
                )
                .unwrap();
            let items = assembly
                .consume(
                    "content_block_delta",
                    &format!(
                        r#"{{"type":"content_block_delta","index":{index},"delta":{{"type":"thinking_delta","thinking":"{text}"}}}}"#
                    ),
                )
                .unwrap();
            assert!(matches!(items[0], ModelStreamItem::ReasoningDelta(_)));
            assembly
                .consume(
                    "content_block_delta",
                    &format!(
                        r#"{{"type":"content_block_delta","index":{index},"delta":{{"type":"signature_delta","signature":"{signature}"}}}}"#
                    ),
                )
                .unwrap();
            assembly
                .consume(
                    "content_block_stop",
                    &format!(r#"{{"type":"content_block_stop","index":{index}}}"#),
                )
                .unwrap();
        }
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"redacted_thinking","data":"opaque-bytes"}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":2}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":"hello"}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":3}"#,
            )
            .unwrap();
        let response = complete(&mut assembly);

        assert_eq!(response.assistant_content.len(), 1);
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Text(_)
        ));
        assert!(
            !response
                .assistant_content
                .iter()
                .any(|block| matches!(block, ContentBlock::Opaque(_)))
        );
        assert!(!format!("{:?}", response.assistant_content).contains("sig-a"));

        let continuation = response.continuation_state.expect("continuation");
        let envelope: Value = serde_json::from_str(continuation.as_str()).unwrap();
        assert_eq!(envelope["provider"], "anthropic.messages");
        assert_eq!(envelope["version"], 1);
        let blocks = envelope["replay_blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "first");
        assert_eq!(blocks[0]["signature"], "sig-a");
        assert_eq!(blocks[1]["type"], "thinking");
        assert_eq!(blocks[1]["thinking"], "second");
        assert_eq!(blocks[1]["signature"], "sig-b");
        assert_eq!(blocks[2]["type"], "redacted_thinking");
        assert_eq!(blocks[2]["data"], "opaque-bytes");
    }

    #[test]
    fn unsigned_thinking_block_yields_no_continuation() {
        let mut assembly = started("request-1", "msg-1");
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"unsigned"}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            )
            .unwrap();
        let response = complete(&mut assembly);
        assert!(response.continuation_state.is_none());
        assert!(response.assistant_content.is_empty());
    }

    #[test]
    fn unknown_event_name_is_ignored() {
        let mut assembly = started("request-1", "msg-1");
        assert!(
            assembly
                .consume("message_annotation", r#"{"type":"message_annotation"}"#)
                .unwrap()
                .is_empty()
        );
        assert!(
            assembly
                .consume("something_new", "not even json")
                .unwrap()
                .is_empty()
        );
        let response = complete(&mut assembly);
        assert_eq!(response.completion_id.as_ref(), "msg-1");
    }

    #[test]
    fn unknown_block_type_and_its_deltas_are_ignored() {
        let mut assembly = started("request-1", "msg-1");
        assert!(
            assembly
                .consume(
                    "content_block_start",
                    r#"{"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srv-1","name":"web_search"}}"#,
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            assembly
                .consume(
                    "content_block_delta",
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":1}"}}"#,
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            assembly
                .consume(
                    "content_block_delta",
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":{}}}"#,
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            assembly
                .consume(
                    "content_block_stop",
                    r#"{"type":"content_block_stop","index":0}"#,
                )
                .unwrap()
                .is_empty()
        );
        // An unknown delta kind on a known block is also dropped.
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap();
        assert!(
            assembly
                .consume(
                    "content_block_delta",
                    r#"{"type":"content_block_delta","index":1,"delta":{"type":"citations_delta","citation":{}}}"#,
                )
                .unwrap()
                .is_empty()
        );
        let response = complete(&mut assembly);
        assert!(response.tool_calls.is_empty());
        assert!(response.assistant_content.is_empty());
    }

    #[test]
    fn text_delta_on_tool_block_is_still_rejected() {
        let mut assembly = started("request-1", "msg-1");
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}"#,
            )
            .unwrap();
        let error = assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"nope"}}"#,
            )
            .unwrap_err();
        assert_eq!(error.kind, StreamNormKind::Response);
        assert_eq!(error.message, "content_block_delta is invalid");
    }

    #[test]
    fn malformed_known_event_is_still_rejected() {
        let mut assembly = started("request-1", "msg-1");
        let error = assembly
            .consume("content_block_start", "{not json")
            .unwrap_err();
        assert_eq!(error.kind, StreamNormKind::Stream);
        assert!(!error.retryable);
    }

    #[test]
    fn overloaded_error_event_is_retryable() {
        let mut assembly = started("request-1", "msg-1");
        for kind in ["overloaded_error", "api_error", "rate_limit_error"] {
            let error = assembly
                .consume(
                    "error",
                    &format!(
                        r#"{{"type":"error","error":{{"type":"{kind}","message":"vendor-secret-canary"}}}}"#
                    ),
                )
                .unwrap_err();
            assert_eq!(error.kind, StreamNormKind::Stream);
            assert!(error.retryable, "{kind} must be retryable");
            assert_eq!(error.message, "stream reported a retryable upstream error");
            assert!(!error.message.contains("vendor-secret-canary"));
        }
    }

    #[test]
    fn invalid_request_error_event_is_not_retryable() {
        let mut assembly = started("request-1", "msg-1");
        let error = assembly
            .consume(
                "error",
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"vendor-secret-canary"}}"#,
            )
            .unwrap_err();
        assert_eq!(error.kind, StreamNormKind::Stream);
        assert!(!error.retryable);
        assert_eq!(error.message, "stream reported an upstream error");

        let error = assembly
            .consume("error", r#"{"type":"error"}"#)
            .unwrap_err();
        assert!(!error.retryable);
        assert_eq!(error.message, "stream reported an upstream error");

        let error = assembly.consume("error", "{broken").unwrap_err();
        assert!(!error.retryable);
        assert_eq!(error.message, "SSE event is invalid JSON");
    }

    #[test]
    fn max_tokens_stop_reason_is_incomplete() {
        let mut assembly = started("request-1", "msg-1");
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            )
            .unwrap();
        let items = assembly
            .consume(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens","stop_sequence":null},"usage":{"output_tokens":128}}"#,
            )
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::Usage(_)));
        let error = assembly
            .consume("message_stop", r#"{"type":"message_stop"}"#)
            .unwrap_err();
        assert_eq!(error.kind, StreamNormKind::Incomplete);
        assert!(!error.retryable);
    }

    #[test]
    fn end_turn_stop_reason_completes() {
        let mut assembly = started("request-1", "msg-1");
        assembly
            .consume(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}"#,
            )
            .unwrap();
        let response = complete(&mut assembly);
        assert_eq!(response.usage.output_tokens(), Some(2));
    }

    #[test]
    fn tool_use_index_reuse_is_rejected() {
        let mut assembly = started("request-1", "msg-1");
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap();
        let error = assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}"#,
            )
            .unwrap_err();
        assert_eq!(error.kind, StreamNormKind::Response);
        assert_eq!(error.message, "content block index was reused");
        // The rejected block must not have reserved a tool-call slot.
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            )
            .unwrap();
        let response = complete(&mut assembly);
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn tool_name_is_shared_across_fragments() {
        let mut assembly = started("request-1", "msg-1");
        let items = assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}"#,
            )
            .unwrap();
        let ModelStreamItem::ToolCallDelta(first) = &items[0] else {
            panic!("expected tool delta");
        };
        let items = assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"x\":1}"}}"#,
            )
            .unwrap();
        let ModelStreamItem::ToolCallDelta(second) = &items[0] else {
            panic!("expected tool delta");
        };
        let (Some(first), Some(second)) = (&first.name, &second.name) else {
            panic!("tool deltas carry the name");
        };
        assert!(Arc::ptr_eq(first, second));
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            )
            .unwrap();
        let response = complete(&mut assembly);
        assert_eq!(response.tool_calls[0].arguments.as_str(), r#"{"x":1}"#);
    }
}
