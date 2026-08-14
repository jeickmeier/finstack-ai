//! Native OpenAI-compatible provider implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_runtime::{
    ContentBlock, ErrorCategory, JsonBlock, Metadata, Model, ModelCapabilities, ModelDescriptor,
    ModelError, ModelEventStream, ModelName, ModelReconcileResult, ModelRequest, ModelResponse,
    ModelStreamItem, ModelTokenEstimate, ModelToolCall, OutputSpec, PendingModelEffect,
    ProviderIds, RawJson, ReasoningDelta, ReconcileContext, TextBlock, TextDelta, ToolCallDelta,
    Usage, UsageDelta,
};
use futures_core::Stream;
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::config::estimator_ref;
use crate::error::{
    CANCELLED, HTTP_ERROR, RESPONSE_INVALID, TIMEOUT, TRANSPORT_ERROR, error, response_error,
    stream_error,
};
use crate::request::{ChatCompletionRequest, serialize_request};
use crate::sse::{SseEvent, SseParser};
use crate::{EndpointQuirks, OpenAiCompatibleConfig, OpenAiModelConfig};

const STREAM_CHANNEL_CAPACITY: usize = 32;

/// Reusable native OpenAI-compatible Chat Completions provider.
pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    config: OpenAiCompatibleConfig,
    models: BTreeMap<ModelName, OpenAiModelConfig>,
    descriptor: ModelDescriptor,
    quirks: EndpointQuirks,
}

impl fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleProvider")
            .field("config", &self.config)
            .field("models", &self.models.keys().collect::<Vec<_>>())
            .field("quirks", &self.quirks)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatibleProvider {
    /// Construct one provider and its reusable pooled HTTP client.
    ///
    /// # Errors
    ///
    /// Rejects an empty/duplicate model catalog or invalid transport configuration.
    pub fn try_new(
        config: OpenAiCompatibleConfig,
        models: Vec<OpenAiModelConfig>,
    ) -> Result<Self, ModelError> {
        let endpoint = config.endpoint_url()?;
        let headers = config.header_map()?;
        let mut by_name = BTreeMap::new();
        for model in models {
            if by_name.insert(model.name.clone(), model).is_some() {
                return Err(crate::error::config_error(
                    "provider contains a duplicate model name",
                ));
            }
        }
        if by_name.is_empty() {
            return Err(crate::error::config_error(
                "provider requires at least one model",
            ));
        }
        let descriptor = ModelDescriptor {
            provider: Arc::from("openai-compatible"),
            models: by_name.keys().cloned().collect::<Vec<_>>().into(),
            metadata: Metadata::empty(),
        };
        descriptor.validate()?;
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(Policy::none())
            .build()
            .map_err(|_| crate::error::config_error("provider HTTP client could not be built"))?;
        let quirks = EndpointQuirks::for_kind(config.endpoint());
        Ok(Self {
            client,
            endpoint,
            config,
            models: by_name,
            descriptor,
            quirks,
        })
    }

    /// Exact checked-in endpoint compatibility facts used by this provider.
    #[must_use]
    pub const fn quirks(&self) -> EndpointQuirks {
        self.quirks
    }

    fn model_config(&self, name: &ModelName) -> Result<&OpenAiModelConfig, ModelError> {
        self.models
            .get(name)
            .ok_or_else(|| crate::error::request_error("requested model is not configured"))
    }
}

impl Model for OpenAiCompatibleProvider {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor.clone()
    }

    fn capabilities(&self, model: &ModelName) -> ModelCapabilities {
        self.models.get(model).map_or_else(
            || ModelCapabilities {
                input: finstack_ai_runtime::InputCapabilities {
                    text: false,
                    json: false,
                    images: false,
                    audio: false,
                    files: false,
                },
                context_profile: self
                    .models
                    .values()
                    .next()
                    .expect("provider model catalog is non-empty")
                    .capabilities(self.config.endpoint())
                    .context_profile,
                native_tool_calls: false,
                parallel_tool_calls: false,
                structured_output: finstack_ai_runtime::StructuredOutputCapability::Unsupported,
                reasoning: false,
                prompt_cache: false,
                resumable_stream: false,
                idempotent_requests: false,
                native_capabilities: BTreeSet::new(),
            },
            |configured| configured.capabilities(self.config.endpoint()),
        )
    }

    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        self.model_config(model)?;
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len()).map_err(|_| {
                error(
                    RESPONSE_INVALID,
                    ErrorCategory::Limit,
                    false,
                    "OpenAI-compatible request length overflowed the estimator",
                )
            })?,
            estimator: estimator_ref(),
        })
    }

    fn request(
        &self,
        request: ModelRequest,
    ) -> finstack_ai_runtime::PortFuture<Result<ModelEventStream, ModelError>> {
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let model = self.model_config(&request.draft.model).cloned();
        let quirks = self.quirks;
        let timeout = self.config.request_timeout();
        let max_event_bytes = self.config.max_event_bytes();
        let max_stream_bytes = self.config.max_stream_bytes();
        Box::pin(async move {
            let model = model?;
            let wire = ChatCompletionRequest::try_from_draft(&request.draft, &model, quirks)?;
            let payload = serialize_request(&wire)?;
            let request_id = request.call.request_id.to_string();
            let cancellation = request.call.run.cancellation;
            let send = client
                .post(endpoint)
                .header("x-client-request-id", &request_id)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(timeout)
                .body(payload)
                .send();
            let response = tokio::select! {
                () = cancellation.cancelled() => return Err(cancelled_error()),
                response = send => response.map_err(|source| transport_error(&source))?,
            };
            if !response.status().is_success() {
                let retryable = matches!(response.status().as_u16(), 408 | 409 | 429 | 500..=599);
                return Err(error(
                    HTTP_ERROR,
                    ErrorCategory::Model,
                    retryable,
                    "OpenAI-compatible endpoint returned an unsuccessful status",
                ));
            }
            let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
            let structured = matches!(request.draft.output, OutputSpec::JsonSchema { .. });
            let task = tokio::spawn(drive_response(
                response,
                sender,
                cancellation,
                request_id,
                structured,
                max_event_bytes,
                max_stream_bytes,
            ));
            Ok(Box::pin(ReceiverModelStream { receiver, task }) as ModelEventStream)
        })
    }

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingModelEffect,
    ) -> finstack_ai_runtime::PortFuture<Result<ModelReconcileResult, ModelError>> {
        Box::pin(async { Ok(ModelReconcileResult::Unknown) })
    }
}

struct ReceiverModelStream {
    receiver: mpsc::Receiver<Result<ModelStreamItem, ModelError>>,
    task: tokio::task::JoinHandle<()>,
}

impl Stream for ReceiverModelStream {
    type Item = Result<ModelStreamItem, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(context)
    }
}

impl Drop for ReceiverModelStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn drive_response(
    response: reqwest::Response,
    sender: mpsc::Sender<Result<ModelStreamItem, ModelError>>,
    cancellation: finstack_ai_runtime::CancellationSignal,
    request_id: String,
    structured: bool,
    max_event_bytes: usize,
    max_stream_bytes: usize,
) {
    let mut body = response.bytes_stream();
    let mut parser = SseParser::new(max_event_bytes, max_stream_bytes);
    let mut assembly = CompletionAssembly::new(request_id, structured);
    loop {
        let chunk = tokio::select! {
            () = cancellation.cancelled() => {
                let _ = sender.send(Err(cancelled_error())).await;
                return;
            }
            () = sender.closed() => return,
            chunk = body.next() => chunk,
        };
        let Some(chunk) = chunk else {
            let result = parser.finish().and_then(|()| {
                Err(stream_error(
                    "OpenAI-compatible SSE stream ended before [DONE]",
                ))
            });
            let _ = sender.send(result.map(|()| unreachable!())).await;
            return;
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(source) => {
                let _ = sender.send(Err(transport_error(&source))).await;
                return;
            }
        };
        let events = match parser.push(&chunk) {
            Ok(events) => events,
            Err(error) => {
                let _ = sender.send(Err(error)).await;
                return;
            }
        };
        for event in events {
            match event {
                SseEvent::Data(data) => match assembly.consume(&data) {
                    Ok(items) => {
                        for item in items {
                            if sender.send(Ok(item)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error)).await;
                        return;
                    }
                },
                SseEvent::Done => {
                    let terminal = assembly.finish();
                    let _ = sender.send(terminal.map(ModelStreamItem::Completed)).await;
                    return;
                }
            }
        }
    }
}

#[derive(Default, Deserialize)]
struct WireDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolDelta>,
}

#[derive(Deserialize)]
struct WireChoice {
    index: u32,
    #[serde(default)]
    delta: WireDelta,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireChunk {
    id: Option<String>,
    #[serde(default)]
    choices: Vec<WireChoice>,
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireToolDelta {
    index: u32,
    function: Option<WireFunctionDelta>,
}

#[derive(Deserialize)]
struct WireFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
#[allow(
    clippy::struct_field_names,
    reason = "private wire fields match the OpenAI-compatible usage object"
)]
struct WireUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

struct ToolAssembly {
    name: Option<String>,
    arguments: String,
}

struct CompletionAssembly {
    request_id: String,
    completion_id: Option<String>,
    text: String,
    tools: BTreeMap<u32, ToolAssembly>,
    usage: Usage,
    structured: bool,
}

impl CompletionAssembly {
    fn new(request_id: String, structured: bool) -> Self {
        Self {
            request_id,
            completion_id: None,
            text: String::new(),
            tools: BTreeMap::new(),
            usage: Usage::empty(),
            structured,
        }
    }

    fn consume(&mut self, data: &str) -> Result<Vec<ModelStreamItem>, ModelError> {
        let chunk: WireChunk = serde_json::from_str(data)
            .map_err(|_| stream_error("OpenAI-compatible SSE chunk is invalid JSON"))?;
        if let Some(id) = chunk.id {
            if id.is_empty() {
                return Err(response_error("OpenAI-compatible completion ID is empty"));
            }
            if self
                .completion_id
                .as_deref()
                .is_some_and(|known| known != id)
            {
                return Err(response_error(
                    "OpenAI-compatible completion ID changed within one stream",
                ));
            }
            self.completion_id = Some(id);
        }
        let mut items = Vec::new();
        if !chunk.choices.is_empty() {
            if chunk.choices.len() != 1 || chunk.choices[0].index != 0 {
                return Err(response_error(
                    "OpenAI-compatible stream must contain exactly choice zero",
                ));
            }
            let delta = &chunk.choices[0].delta;
            if let Some(text) = &delta.content
                && !text.is_empty()
            {
                self.text.push_str(text);
                if !self.structured {
                    items.push(ModelStreamItem::TextDelta(TextDelta {
                        text: Arc::from(text.as_str()),
                    }));
                }
            }
            if let Some(reasoning) = &delta.reasoning_content
                && !reasoning.is_empty()
            {
                items.push(ModelStreamItem::ReasoningDelta(ReasoningDelta {
                    text: Arc::from(reasoning.as_str()),
                }));
            }
            for tool in &delta.tool_calls {
                let function = tool.function.as_ref();
                let name = function.and_then(|function| function.name.as_deref());
                let arguments = function
                    .and_then(|function| function.arguments.as_deref())
                    .unwrap_or_default();
                let assembly = self
                    .tools
                    .entry(tool.index)
                    .or_insert_with(|| ToolAssembly {
                        name: None,
                        arguments: String::new(),
                    });
                if let Some(name) = name {
                    if assembly.name.as_deref().is_some_and(|known| known != name) {
                        return Err(response_error(
                            "OpenAI-compatible tool-call name changed within one stream",
                        ));
                    }
                    assembly.name = Some(name.to_owned());
                }
                assembly.arguments.push_str(arguments);
                items.push(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: tool.index,
                    name: name.map(Arc::from),
                    arguments_delta: Arc::from(arguments),
                }));
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
            .map_err(|_| response_error("OpenAI-compatible usage is invalid"))?;
            items.push(ModelStreamItem::Usage(UsageDelta {
                usage: self.usage.clone(),
            }));
        }
        Ok(items)
    }

    fn finish(self) -> Result<ModelResponse, ModelError> {
        let completion_id = self
            .completion_id
            .ok_or_else(|| response_error("OpenAI-compatible stream omitted its completion ID"))?;
        let assistant_content: Arc<[ContentBlock]> = if self.structured {
            let value = RawJson::parse(self.text.as_bytes()).map_err(|_| {
                response_error("OpenAI-compatible structured output is not valid JSON")
            })?;
            Arc::from([ContentBlock::Json(JsonBlock::new(value))])
        } else if self.text.is_empty() {
            Arc::from([])
        } else {
            Arc::from([ContentBlock::Text(TextBlock::try_new(self.text).map_err(
                |_| response_error("OpenAI-compatible assistant text exceeds the kernel bound"),
            )?)])
        };
        let mut tool_calls = Vec::with_capacity(self.tools.len());
        for (expected, (index, tool)) in (0_u32..).zip(self.tools) {
            if expected != index {
                return Err(response_error(
                    "OpenAI-compatible tool-call indices are not contiguous",
                ));
            }
            let name = tool
                .name
                .ok_or_else(|| response_error("OpenAI-compatible tool call omitted its name"))?;
            let arguments = RawJson::parse(tool.arguments.as_bytes()).map_err(|_| {
                response_error("OpenAI-compatible tool-call arguments are invalid JSON")
            })?;
            tool_calls.push(ModelToolCall {
                name: Arc::from(name),
                arguments,
            });
        }
        let provider_ids = ProviderIds::try_new(
            Some(self.request_id.as_str()),
            Some(completion_id.as_str()),
            None::<&str>,
        )
        .map_err(|_| response_error("OpenAI-compatible provider identifiers are invalid"))?;
        Ok(ModelResponse {
            assistant_content,
            tool_calls: tool_calls.into(),
            usage: self.usage,
            provider_ids,
            completion_id: Arc::from(completion_id),
            continuation_state: None,
        })
    }
}

fn cancelled_error() -> ModelError {
    error(
        CANCELLED,
        ErrorCategory::Cancellation,
        false,
        "OpenAI-compatible request was cancelled",
    )
}

fn transport_error(source: &reqwest::Error) -> ModelError {
    if source.is_timeout() {
        error(
            TIMEOUT,
            ErrorCategory::Deadline,
            true,
            "OpenAI-compatible request timed out",
        )
    } else {
        error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "OpenAI-compatible transport failed",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_tools_and_usage() {
        let mut assembly = CompletionAssembly::new("request-1".to_owned(), false);
        let items = assembly
            .consume(
                r#"{"id":"chat-1","choices":[{"index":0,"delta":{"content":"hello","tool_calls":[{"index":0,"function":{"name":"lookup","arguments":"{\"x\":"}}]}}],"usage":null}"#,
            )
            .unwrap();
        assert_eq!(items.len(), 2);
        assembly
            .consume(
                r#"{"id":"chat-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}"#,
            )
            .unwrap();
        let response = assembly.finish().unwrap();
        assert_eq!(response.completion_id.as_ref(), "chat-1");
        assert_eq!(response.tool_calls[0].name.as_ref(), "lookup");
        assert_eq!(response.tool_calls[0].arguments.as_str(), r#"{"x":1}"#);
        assert_eq!(response.usage.total_tokens(), Some(5));
    }

    #[test]
    fn structured_output_becomes_json_block_without_text_deltas() {
        let mut assembly = CompletionAssembly::new("request-1".to_owned(), true);
        let items = assembly
            .consume(
                r#"{"id":"chat-2","choices":[{"index":0,"delta":{"content":"{\"ok\":true}"}}],"usage":null}"#,
            )
            .unwrap();
        assert!(items.is_empty());
        let response = assembly.finish().unwrap();
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Json(_)
        ));
    }
}
