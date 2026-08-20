//! Gemini `generateContent` SSE chunk assembly.
//!
//! Each SSE frame carries an unnamed `GenerateContentResponse` chunk. Only
//! `candidates[0]` is read because every request pins `candidateCount: 1`.
//! Thought signatures and other opaque continuation material are accumulated
//! verbatim into the continuation envelope and never surface in stream deltas,
//! assistant content, or `Debug` output (SEC-INV-005).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    ContentBlock, JsonBlock, LimitKey, ModelResponse, ModelStreamItem, ModelToolCall, OpaqueBlock,
    OpaquePayload, ProviderIds, RawJson, ReasoningDelta, TextBlock, TextDelta, ToolCallDelta,
    Usage, UsageDelta,
};

use super::StreamNormError;

/// Continuation-envelope provider tag for Gemini `generateContent` streams.
pub const GEMINI_CONTINUATION_PROVIDER: &str = "gemini.generate-content";
/// Opaque media type carrying a candidate's `groundingMetadata`.
pub const GEMINI_GROUNDING_MEDIA_TYPE: &str = "application/vnd.finstack.gemini.grounding-metadata";
/// Opaque media type carrying an `executableCode` part.
pub const GEMINI_EXECUTABLE_CODE_MEDIA_TYPE: &str =
    "application/vnd.finstack.gemini.executable-code";
/// Opaque media type carrying a `codeExecutionResult` part.
pub const GEMINI_CODE_RESULT_MEDIA_TYPE: &str =
    "application/vnd.finstack.gemini.code-execution-result";
/// Usage extension counter for `usageMetadata.thoughtsTokenCount`.
pub const GEMINI_THOUGHTS_TOKENS_KEY: &str = "gemini.thoughts_token_count";
/// Usage extension counter for `usageMetadata.cachedContentTokenCount`.
pub const GEMINI_CACHED_TOKENS_KEY: &str = "gemini.cached_content_token_count";

const CONTINUATION_VERSION: u32 = 1;
const MODEL_ROLE: &str = "model";

#[derive(Deserialize)]
struct WireChunk {
    #[serde(default)]
    candidates: Vec<WireCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<WireUsage>,
    #[serde(default, rename = "promptFeedback")]
    prompt_feedback: Option<WirePromptFeedback>,
    #[serde(default, rename = "responseId")]
    response_id: Option<String>,
}

#[derive(Deserialize)]
struct WireCandidate {
    #[serde(default)]
    content: Option<WireContent>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
    #[serde(default, rename = "groundingMetadata")]
    grounding_metadata: Option<Value>,
}

#[derive(Deserialize)]
struct WireContent {
    #[serde(default)]
    parts: Vec<Value>,
}

#[derive(Deserialize)]
struct WirePromptFeedback {
    #[serde(default, rename = "blockReason")]
    block_reason: Option<String>,
}

#[derive(Deserialize)]
#[allow(
    clippy::struct_field_names,
    reason = "private wire fields match the Gemini usageMetadata object"
)]
struct WireUsage {
    #[serde(default, rename = "promptTokenCount")]
    prompt_token_count: Option<u64>,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_token_count: Option<u64>,
    #[serde(default, rename = "totalTokenCount")]
    total_token_count: Option<u64>,
    #[serde(default, rename = "thoughtsTokenCount")]
    thoughts_token_count: Option<u64>,
    #[serde(default, rename = "cachedContentTokenCount")]
    cached_content_token_count: Option<u64>,
}

struct ToolAssembly {
    call_id: Option<String>,
    name: String,
    arguments: String,
}

/// Incremental Gemini `generateContent` assembler.
///
/// Deliberately does not implement [`Debug`]: the replay accumulator holds
/// verbatim `thoughtSignature` material which must never reach logs.
pub struct GeminiGenerateContentAssembly {
    request_id: String,
    structured: bool,
    completion_id: Option<String>,
    text: String,
    tools: Vec<ToolAssembly>,
    opaque: Vec<(&'static str, Value)>,
    replay_parts: Vec<Value>,
    usage: Usage,
    completed: bool,
}

impl GeminiGenerateContentAssembly {
    /// Start a new `generateContent` assembly for one committed request.
    #[must_use]
    pub fn new(request_id: String, structured: bool) -> Self {
        Self {
            request_id,
            structured,
            completion_id: None,
            text: String::new(),
            tools: Vec::new(),
            opaque: Vec::new(),
            replay_parts: Vec::new(),
            usage: Usage::empty(),
            completed: false,
        }
    }

    /// True once a [`ModelStreamItem::Completed`] item has been emitted.
    #[must_use]
    pub const fn completed(&self) -> bool {
        self.completed
    }

    /// Consume one SSE `data:` payload (a `GenerateContentResponse` chunk).
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when the chunk is malformed, the prompt was
    /// blocked, or the candidate reported a non-terminal finish reason.
    pub fn consume(&mut self, data: &str) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let chunk: WireChunk = serde_json::from_str(data)
            .map_err(|_| StreamNormError::stream("SSE chunk is invalid JSON"))?;
        if let Some(id) = chunk.response_id.filter(|value| !value.is_empty()) {
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
        if chunk.candidates.is_empty() {
            if chunk
                .prompt_feedback
                .and_then(|feedback| feedback.block_reason)
                .is_some_and(|reason| !reason.is_empty())
            {
                return Err(StreamNormError::response("prompt was blocked"));
            }
            return self.apply_usage(chunk.usage_metadata.as_ref());
        }
        let Some(candidate) = chunk.candidates.into_iter().next() else {
            return Ok(Vec::new());
        };
        let mut items = Vec::new();
        if let Some(content) = candidate.content {
            for part in &content.parts {
                items.extend(self.consume_part(part)?);
            }
        }
        if let Some(metadata) = candidate.grounding_metadata {
            self.opaque.push((GEMINI_GROUNDING_MEDIA_TYPE, metadata));
        }
        items.extend(self.apply_usage(chunk.usage_metadata.as_ref())?);
        if let Some(reason) = candidate.finish_reason.filter(|value| !value.is_empty()) {
            items.extend(self.consume_finish_reason(&reason)?);
        }
        Ok(items)
    }

    /// Assert the stream reached a terminal chunk before EOF.
    ///
    /// # Errors
    ///
    /// Returns [`StreamNormError`] when no `finishReason` chunk arrived.
    pub fn finish(&mut self) -> Result<(), StreamNormError> {
        if self.completed {
            return Ok(());
        }
        Err(StreamNormError::stream(
            "SSE stream ended before a finishReason",
        ))
    }

    fn consume_finish_reason(
        &mut self,
        reason: &str,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        if self.completed {
            return Ok(Vec::new());
        }
        match reason {
            "STOP" | "MAX_TOKENS" => {
                let response = self.build_completed()?;
                self.completed = true;
                Ok(vec![ModelStreamItem::Completed(response)])
            }
            _ => Err(StreamNormError::response(
                "candidate reported a non-terminal finish reason",
            )),
        }
    }

    fn consume_part(&mut self, part: &Value) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        self.replay_parts.push(part.clone());
        if let Some(call) = part.get("functionCall") {
            return self.consume_function_call(call).map(|item| vec![item]);
        }
        if let Some(code) = part.get("executableCode") {
            self.opaque
                .push((GEMINI_EXECUTABLE_CODE_MEDIA_TYPE, code.clone()));
            return Ok(Vec::new());
        }
        if let Some(result) = part.get("codeExecutionResult") {
            self.opaque
                .push((GEMINI_CODE_RESULT_MEDIA_TYPE, result.clone()));
            return Ok(Vec::new());
        }
        let Some(text) = part.get("text").and_then(Value::as_str) else {
            return Ok(Vec::new());
        };
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            return Ok(vec![ModelStreamItem::ReasoningDelta(ReasoningDelta {
                text: Arc::from(text),
            })]);
        }
        self.text.push_str(text);
        if self.structured {
            Ok(Vec::new())
        } else {
            Ok(vec![ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            })])
        }
    }

    fn consume_function_call(&mut self, call: &Value) -> Result<ModelStreamItem, StreamNormError> {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| StreamNormError::response("functionCall omitted its name"))?;
        let call_id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let arguments = match call.get("args") {
            None | Some(Value::Null) => "{}".to_owned(),
            Some(value) => serde_json::to_string(value)
                .map_err(|_| StreamNormError::response("tool-call arguments are invalid JSON"))?,
        };
        let index = u32::try_from(self.tools.len())
            .map_err(|_| StreamNormError::response("tool-call index overflowed"))?;
        self.tools.push(ToolAssembly {
            call_id: call_id.clone(),
            name: name.to_owned(),
            arguments: arguments.clone(),
        });
        Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
            index,
            name: Some(Arc::from(name)),
            arguments_delta: Arc::from(arguments.as_str()),
            provider_call_id: call_id.as_deref().map(Arc::from),
        }))
    }

    fn apply_usage(
        &mut self,
        usage: Option<&WireUsage>,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        let Some(usage) = usage else {
            return Ok(Vec::new());
        };
        let mut counters = BTreeMap::new();
        if let Some(value) = usage.thoughts_token_count.filter(|value| *value > 0) {
            counters.insert(LimitKey::from_static(GEMINI_THOUGHTS_TOKENS_KEY), value);
        }
        if let Some(value) = usage.cached_content_token_count.filter(|value| *value > 0) {
            counters.insert(LimitKey::from_static(GEMINI_CACHED_TOKENS_KEY), value);
        }
        for (key, value) in self.usage.extension_counters() {
            counters.entry(key.clone()).or_insert(*value);
        }
        let input_tokens = usage.prompt_token_count.or(self.usage.input_tokens());
        let output_tokens = usage.candidates_token_count.or(self.usage.output_tokens());
        let total_tokens =
            usage
                .total_token_count
                .or_else(|| match (input_tokens, output_tokens) {
                    (Some(input), Some(output)) => input.checked_add(output),
                    _ => None,
                });
        self.usage = Usage::try_new(input_tokens, output_tokens, total_tokens, None, counters)
            .map_err(|_| StreamNormError::response("usage is invalid"))?;
        Ok(vec![ModelStreamItem::Usage(UsageDelta {
            usage: self.usage.clone(),
        })])
    }

    fn build_completed(&self) -> Result<ModelResponse, StreamNormError> {
        let completion_id = self
            .completion_id
            .clone()
            .unwrap_or_else(|| self.request_id.clone());
        let mut assistant = Vec::with_capacity(self.opaque.len().saturating_add(1));
        if self.structured {
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
        for (media_type, value) in &self.opaque {
            assistant.push(ContentBlock::Opaque(opaque_block(media_type, value)?));
        }
        let mut tool_calls = Vec::with_capacity(self.tools.len());
        for tool in &self.tools {
            let arguments = RawJson::parse(tool.arguments.as_bytes())
                .map_err(|_| StreamNormError::response("tool-call arguments are invalid JSON"))?;
            tool_calls.push(ModelToolCall {
                name: Arc::from(tool.name.as_str()),
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
            assistant_content: assistant.into(),
            tool_calls: tool_calls.into(),
            usage: self.usage.clone(),
            provider_ids,
            completion_id: Arc::from(completion_id),
            continuation_state: Some(self.continuation_envelope()?),
        })
    }

    fn continuation_envelope(&self) -> Result<RawJson, StreamNormError> {
        let envelope = json!({
            "provider": GEMINI_CONTINUATION_PROVIDER,
            "version": CONTINUATION_VERSION,
            "replay_contents": [{
                "role": MODEL_ROLE,
                "parts": self.replay_parts,
            }],
        });
        let bytes = serde_json::to_vec(&envelope)
            .map_err(|_| StreamNormError::response("provider continuation could not be encoded"))?;
        RawJson::parse(bytes.as_slice())
            .map_err(|_| StreamNormError::response("provider continuation is not canonical JSON"))
    }
}

fn opaque_block(media_type: &str, value: &Value) -> Result<OpaqueBlock, StreamNormError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| StreamNormError::response("opaque payload could not be encoded"))?;
    let raw = RawJson::parse(bytes.as_slice())
        .map_err(|_| StreamNormError::response("opaque payload is not canonical JSON"))?;
    OpaqueBlock::try_new(media_type, OpaquePayload::json(raw))
        .map_err(|_| StreamNormError::response("opaque media type is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK_TEXT_ONE: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello "}]},"index":0}],"responseId":"resp-1","modelVersion":"gemini-2.5-pro"}"#;
    const CHUNK_TEXT_TWO: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"world"}]},"index":0}],"responseId":"resp-1"}"#;
    const CHUNK_STOP: &str = r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15},"responseId":"resp-1"}"#;

    #[test]
    fn text_stream_assembles_completed_response() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        let first = assembly.consume(CHUNK_TEXT_ONE).unwrap();
        assert!(matches!(first[0], ModelStreamItem::TextDelta(_)));
        assert_eq!(first.len(), 1);
        let second = assembly.consume(CHUNK_TEXT_TWO).unwrap();
        assert!(matches!(second[0], ModelStreamItem::TextDelta(_)));
        assert!(!assembly.completed());

        let last = assembly.consume(CHUNK_STOP).unwrap();
        assert!(matches!(last[0], ModelStreamItem::Usage(_)));
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        assert!(assembly.completed());
        assert!(assembly.finish().is_ok());

        let ContentBlock::Text(text) = &response.assistant_content[0] else {
            panic!("expected assistant text");
        };
        assert_eq!(text.text(), "Hello world");
        assert_eq!(response.completion_id.as_ref(), "resp-1");
        assert_eq!(response.usage.input_tokens(), Some(10));
        assert_eq!(response.usage.output_tokens(), Some(5));
        assert_eq!(response.usage.total_tokens(), Some(15));
        let continuation = response
            .continuation_state
            .as_ref()
            .expect("continuation state");
        assert!(continuation.as_str().contains(GEMINI_CONTINUATION_PROVIDER));
        assert!(continuation.as_str().contains("replay_contents"));
    }

    #[test]
    fn thought_parts_emit_reasoning_deltas_and_are_not_persisted() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        let items = assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"pondering","thought":true}]},"index":0}],"responseId":"resp-1"}"#,
            )
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::ReasoningDelta(_)));
        assert_eq!(items.len(), 1);
        let items = assembly.consume(CHUNK_TEXT_TWO).unwrap();
        assert!(matches!(items[0], ModelStreamItem::TextDelta(_)));

        let last = assembly.consume(CHUNK_STOP).unwrap();
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        let ContentBlock::Text(text) = &response.assistant_content[0] else {
            panic!("expected assistant text");
        };
        assert_eq!(text.text(), "world");
        assert!(!text.text().contains("pondering"));
    }

    #[test]
    fn thought_signature_lands_in_continuation_not_content() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        let items = assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"visible","thoughtSignature":"c2ln-fixture-001"}]},"index":0}],"responseId":"resp-1"}"#,
            )
            .unwrap();
        let ModelStreamItem::TextDelta(delta) = &items[0] else {
            panic!("expected text delta");
        };
        assert_eq!(delta.text.as_ref(), "visible");
        assert!(!delta.text.contains("c2ln-fixture-001"));

        let last = assembly.consume(CHUNK_STOP).unwrap();
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        let ContentBlock::Text(text) = &response.assistant_content[0] else {
            panic!("expected assistant text");
        };
        assert!(!text.text().contains("c2ln-fixture-001"));
        let continuation = response
            .continuation_state
            .as_ref()
            .expect("continuation state");
        assert!(continuation.as_str().contains("c2ln-fixture-001"));
    }

    #[test]
    fn function_call_maps_to_tool_call_with_optional_id() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        let items = assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"id":"fc-1","name":"lookup","args":{"x":1}}}]},"index":0}],"responseId":"resp-1"}"#,
            )
            .unwrap();
        let ModelStreamItem::ToolCallDelta(delta) = &items[0] else {
            panic!("expected tool-call delta");
        };
        assert_eq!(delta.index, 0);
        assert_eq!(delta.name.as_deref(), Some("lookup"));
        assert_eq!(delta.provider_call_id.as_deref(), Some("fc-1"));
        let last = assembly.consume(CHUNK_STOP).unwrap();
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        assert_eq!(response.tool_calls[0].name.as_ref(), "lookup");
        assert_eq!(response.tool_calls[0].arguments.as_str(), r#"{"x":1}"#);
        assert_eq!(
            response.tool_calls[0].provider_call_id.as_deref(),
            Some("fc-1")
        );

        let mut assembly = GeminiGenerateContentAssembly::new("request-2".to_owned(), false);
        assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"lookup","args":{}}}]},"index":0}],"responseId":"resp-1"}"#,
            )
            .unwrap();
        let last = assembly.consume(CHUNK_STOP).unwrap();
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        assert_eq!(response.tool_calls[0].arguments.as_str(), "{}");
        assert!(response.tool_calls[0].provider_call_id.is_none());
    }

    #[test]
    fn grounding_metadata_becomes_opaque_block() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        assembly.consume(CHUNK_TEXT_ONE).unwrap();
        let last = assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP","index":0,"groundingMetadata":{"webSearchQueries":["rust"]}}],"responseId":"resp-1"}"#,
            )
            .unwrap();
        let ModelStreamItem::Completed(response) = last.last().expect("items") else {
            panic!("expected completed item");
        };
        let opaque = response
            .assistant_content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Opaque(block) => Some(block),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(opaque.len(), 1);
        assert_eq!(opaque[0].media_type(), GEMINI_GROUNDING_MEDIA_TYPE);
        assert!(matches!(opaque[0].payload(), OpaquePayload::Json(_)));
    }

    #[test]
    fn executable_code_and_result_parts_become_opaque_blocks() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[{"executableCode":{"language":"PYTHON","code":"print(1)"}},{"codeExecutionResult":{"outcome":"OUTCOME_OK","output":"1"}}]},"index":0}],"responseId":"resp-1"}"#,
            )
            .unwrap();
        let last = assembly.consume(CHUNK_STOP).unwrap();
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        let media = response
            .assistant_content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Opaque(block) => Some(block.media_type().to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            media,
            vec![
                GEMINI_EXECUTABLE_CODE_MEDIA_TYPE.to_owned(),
                GEMINI_CODE_RESULT_MEDIA_TYPE.to_owned(),
            ]
        );
    }

    #[test]
    fn usage_extension_counters_map_and_suppress_zeros() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        assembly.consume(CHUNK_TEXT_ONE).unwrap();
        let last = assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"totalTokenCount":15,"thoughtsTokenCount":7,"cachedContentTokenCount":0},"responseId":"resp-1"}"#,
            )
            .unwrap();
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        let counters = response.usage.extension_counters();
        assert_eq!(counters.len(), 1);
        assert_eq!(
            counters.get(&LimitKey::from_static(GEMINI_THOUGHTS_TOKENS_KEY)),
            Some(&7)
        );
        assert!(
            counters
                .get(&LimitKey::from_static(GEMINI_CACHED_TOKENS_KEY))
                .is_none()
        );
    }

    #[test]
    fn safety_finish_reason_is_an_error() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        assembly.consume(CHUNK_TEXT_ONE).unwrap();
        let error = assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"SAFETY","index":0}],"responseId":"resp-1"}"#,
            )
            .expect_err("safety finish reason");
        assert_eq!(error.kind, super::super::StreamNormKind::Response);
        assert!(!assembly.completed());
    }

    #[test]
    fn prompt_feedback_block_reason_without_candidates_is_an_error() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        let error = assembly
            .consume(r#"{"promptFeedback":{"blockReason":"SAFETY"},"responseId":"resp-1"}"#)
            .expect_err("block reason");
        assert_eq!(error.kind, super::super::StreamNormKind::Response);
    }

    #[test]
    fn eof_without_finish_reason_is_an_error() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), false);
        assembly.consume(CHUNK_TEXT_ONE).unwrap();
        assert!(!assembly.completed());
        assert!(assembly.finish().is_err());
    }

    #[test]
    fn structured_terminal_requires_json_text() {
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), true);
        let items = assembly
            .consume(
                r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"{\"ok\":true}"}]},"index":0}],"responseId":"resp-1"}"#,
            )
            .unwrap();
        assert!(items.is_empty());
        let last = assembly.consume(CHUNK_STOP).unwrap();
        let ModelStreamItem::Completed(response) = &last[1] else {
            panic!("expected completed item");
        };
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Json(_)
        ));

        let mut assembly = GeminiGenerateContentAssembly::new("request-2".to_owned(), true);
        assert!(assembly.consume(CHUNK_STOP).is_err());
    }
}
