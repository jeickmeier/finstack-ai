//! Native Anthropic Messages provider implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};

use finstack_ai_runtime::{
    ContentBlock, ErrorCategory, JsonBlock, LimitKey, Metadata, Model, ModelCapabilities,
    ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelReconcileResult, ModelRequest,
    ModelResponse, ModelStreamItem, ModelTokenEstimate, ModelToolCall, OpaqueBlock, OpaquePayload,
    OutputSpec, PendingModelEffect, ProviderIds, RawJson, ReasoningDelta, ReconcileContext,
    TextBlock, TextDelta, ToolCallDelta, Usage, UsageDelta,
};
use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::config::estimator_ref;
use crate::error::{
    CANCELLED, HTTP_ERROR, RESPONSE_INVALID, TIMEOUT, TRANSPORT_ERROR, error, response_error,
    stream_error,
};
use crate::request::{MessagesRequest, serialize_request};
use crate::sse::SseParser;
use crate::{AnthropicConfig, AnthropicModelConfig};

const STREAM_CHANNEL_CAPACITY: usize = 32;
const THINKING_SIGNATURE_MEDIA_TYPE: &str = "application/vnd.finstack.anthropic.thinking-signature";
const CACHE_CREATION_KEY: &str = "anthropic.cache_creation_input_tokens";
const CACHE_READ_KEY: &str = "anthropic.cache_read_input_tokens";

/// Reusable native Anthropic Messages provider.
pub struct AnthropicProvider {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    config: AnthropicConfig,
    models: RwLock<BTreeMap<ModelName, AnthropicModelConfig>>,
}

impl fmt::Debug for AnthropicProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicProvider")
            .field("config", &self.config)
            .field(
                "models",
                &self
                    .models
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl AnthropicProvider {
    /// Construct one provider and its reusable pooled HTTP client.
    ///
    /// # Errors
    ///
    /// Rejects an empty/duplicate model catalog or invalid transport configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_anthropic::{
    ///     AnthropicConfig, AnthropicModelConfig, AnthropicProvider,
    /// };
    /// use finstack_ai_runtime::Model;
    ///
    /// let config = AnthropicConfig::try_new("http://127.0.0.1:9").expect("config");
    /// let model = AnthropicModelConfig::try_new(
    ///     "claude-test",
    ///     1_000_000,
    ///     128_000,
    ///     4_096,
    ///     4_096,
    ///     256,
    /// )
    /// .expect("model");
    /// let provider = AnthropicProvider::try_new(config, vec![model]).expect("provider");
    /// assert_eq!(provider.descriptor().provider.as_ref(), "anthropic");
    /// ```
    pub fn try_new(
        config: AnthropicConfig,
        models: Vec<AnthropicModelConfig>,
    ) -> Result<Self, ModelError> {
        let endpoint = config.endpoint_url()?;
        let headers = config.header_map()?;
        let by_name = catalog_from_models(models)?;
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(Policy::none())
            .build()
            .map_err(|_| crate::error::config_error("provider HTTP client could not be built"))?;
        Ok(Self {
            client,
            endpoint,
            config,
            models: RwLock::new(by_name),
        })
    }

    /// Replace the in-memory model catalog from a local table.
    ///
    /// # Errors
    ///
    /// Rejects an empty or duplicate catalog.
    pub fn replace_model_catalog(
        &self,
        models: Vec<AnthropicModelConfig>,
    ) -> Result<(), ModelError> {
        let by_name = catalog_from_models(models)?;
        *self.models.write().unwrap_or_else(PoisonError::into_inner) = by_name;
        Ok(())
    }

    /// Refresh advertised capabilities for one configured model from a local table.
    ///
    /// # Errors
    ///
    /// Returns `anthropic_request_invalid` when the model is not configured.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "leaf refresh takes the replacement capability snapshot by value"
    )]
    pub fn refresh_model_metadata(
        &self,
        model: &ModelName,
        update: ModelCapabilities,
    ) -> Result<(), ModelError> {
        let mut models = self.models.write().unwrap_or_else(PoisonError::into_inner);
        let configured = models
            .get_mut(model)
            .ok_or_else(|| crate::error::request_error("requested model is not configured"))?;
        configured.apply_capabilities(&update);
        Ok(())
    }

    fn model_config(&self, name: &ModelName) -> Result<AnthropicModelConfig, ModelError> {
        self.models
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| crate::error::request_error("requested model is not configured"))
    }
}

fn catalog_from_models(
    models: Vec<AnthropicModelConfig>,
) -> Result<BTreeMap<ModelName, AnthropicModelConfig>, ModelError> {
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
        provider: Arc::from("anthropic"),
        models: by_name.keys().cloned().collect::<Vec<_>>().into(),
        metadata: Metadata::empty(),
    };
    descriptor.validate()?;
    Ok(by_name)
}

impl Model for AnthropicProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        ModelDescriptor {
            provider: Arc::from("anthropic"),
            models: models.keys().cloned().collect::<Vec<_>>().into(),
            metadata: Metadata::empty(),
        }
    }

    fn capabilities(&self, model: &ModelName) -> ModelCapabilities {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        models.get(model).map_or_else(
            || ModelCapabilities {
                input: finstack_ai_runtime::InputCapabilities {
                    text: false,
                    json: false,
                    images: false,
                    audio: false,
                    files: false,
                },
                context_profile: models
                    .values()
                    .next()
                    .expect("provider model catalog is non-empty")
                    .capabilities()
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
            AnthropicModelConfig::capabilities,
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
                    "Anthropic request length overflowed the estimator",
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
        let model = self.model_config(&request.draft.model);
        let timeout = self.config.request_timeout();
        let max_event_bytes = self.config.max_event_bytes();
        let max_stream_bytes = self.config.max_stream_bytes();
        Box::pin(async move {
            let model = model?;
            let wire = MessagesRequest::try_from_draft(&request.draft, &model)?;
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
                    "Anthropic endpoint returned an unsuccessful status",
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
                    "Anthropic SSE stream ended before message_stop",
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
            match event.name.as_str() {
                "ping" => {}
                "error" => {
                    let _ = sender
                        .send(Err(stream_error("Anthropic stream reported an error")))
                        .await;
                    return;
                }
                "message_stop" => {
                    let terminal = assembly.finish();
                    let _ = sender.send(terminal.map(ModelStreamItem::Completed)).await;
                    return;
                }
                "message_start"
                | "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
                | "message_delta" => match assembly.consume(&event.name, &event.data) {
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
                _ => {
                    let _ = sender
                        .send(Err(stream_error("Anthropic SSE event name is unknown")))
                        .await;
                    return;
                }
            }
        }
    }
}

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
    Tool {
        stream_index: u32,
        name: String,
        arguments: String,
    },
    Redacted,
}

struct CompletionAssembly {
    request_id: String,
    completion_id: Option<String>,
    text: String,
    thinking_signature: Option<String>,
    tools: BTreeMap<u32, (String, String, String)>,
    next_tool_index: u32,
    blocks: BTreeMap<u32, OpenBlock>,
    usage: Usage,
    structured: bool,
}

impl CompletionAssembly {
    fn new(request_id: String, structured: bool) -> Self {
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

    fn consume(&mut self, name: &str, data: &str) -> Result<Vec<ModelStreamItem>, ModelError> {
        let event: WireEvent = serde_json::from_str(data)
            .map_err(|_| stream_error("Anthropic SSE event is invalid JSON"))?;
        if event.kind != name {
            return Err(stream_error(
                "Anthropic SSE event name does not match its payload",
            ));
        }
        match name {
            "message_start" => self.consume_message_start(event),
            "content_block_start" => self.consume_block_start(event),
            "content_block_delta" => self.consume_block_delta(event),
            "content_block_stop" => self.consume_block_stop(&event),
            "message_delta" => self.consume_message_delta(event),
            _ => Err(stream_error("Anthropic SSE event name is unknown")),
        }
    }

    fn consume_message_start(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, ModelError> {
        let message = event
            .message
            .ok_or_else(|| response_error("Anthropic message_start omitted its message"))?;
        let id = message
            .id
            .filter(|value| !value.is_empty())
            .ok_or_else(|| response_error("Anthropic completion ID is empty"))?;
        if self
            .completion_id
            .as_deref()
            .is_some_and(|known| known != id)
        {
            return Err(response_error(
                "Anthropic completion ID changed within one stream",
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
    ) -> Result<Vec<ModelStreamItem>, ModelError> {
        let index = event
            .index
            .ok_or_else(|| response_error("Anthropic content block omitted its index"))?;
        let block = event
            .content_block
            .ok_or_else(|| response_error("Anthropic content_block_start omitted its block"))?;
        let open = match block.kind.as_str() {
            "text" => OpenBlock::Text,
            "thinking" => OpenBlock::Thinking,
            "redacted_thinking" => OpenBlock::Redacted,
            "tool_use" => {
                let name = block
                    .name
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| response_error("Anthropic tool_use omitted its name"))?;
                let id = block
                    .id
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| response_error("Anthropic tool_use omitted its id"))?;
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
                    .ok_or_else(|| response_error("Anthropic tool-call index overflowed"))?;
                let mut items = Vec::new();
                if !name.is_empty() {
                    items.push(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                        index: stream_index,
                        name: Some(Arc::from(name.as_str())),
                        arguments_delta: Arc::from(arguments.as_str()),
                        provider_call_id: None,
                    }));
                }
                self.tools
                    .insert(stream_index, (id, name.clone(), arguments.clone()));
                let open = OpenBlock::Tool {
                    stream_index,
                    name,
                    arguments,
                };
                self.blocks.insert(index, open);
                return Ok(items);
            }
            _ => return Err(response_error("Anthropic content block type is unknown")),
        };
        if self.blocks.insert(index, open).is_some() {
            return Err(response_error("Anthropic content block index was reused"));
        }
        Ok(Vec::new())
    }

    fn consume_block_delta(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, ModelError> {
        let index = event
            .index
            .ok_or_else(|| response_error("Anthropic content block omitted its index"))?;
        let delta = event
            .delta
            .ok_or_else(|| response_error("Anthropic content_block_delta omitted its delta"))?;
        let kind = delta
            .kind
            .as_deref()
            .ok_or_else(|| response_error("Anthropic content_block_delta omitted its type"))?;
        if !self.blocks.contains_key(&index) {
            return Err(response_error(
                "Anthropic content_block_delta referenced no block",
            ));
        }
        match kind {
            "text_delta" => {
                if !matches!(self.blocks.get(&index), Some(OpenBlock::Text)) {
                    return Err(response_error("Anthropic content_block_delta is invalid"));
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
                    return Err(response_error("Anthropic content_block_delta is invalid"));
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
                    return Err(response_error("Anthropic content_block_delta is invalid"));
                }
                if let Some(signature) = delta.signature.filter(|value| !value.is_empty()) {
                    self.thinking_signature = Some(signature);
                }
                Ok(Vec::new())
            }
            "input_json_delta" => {
                let fragment = delta.partial_json.unwrap_or_default();
                let (stream_index, name) = match self.blocks.get_mut(&index) {
                    Some(OpenBlock::Tool {
                        stream_index,
                        arguments,
                        name,
                        ..
                    }) => {
                        arguments.push_str(&fragment);
                        (*stream_index, name.clone())
                    }
                    _ => return Err(response_error("Anthropic content_block_delta is invalid")),
                };
                if let Some((_, _, stored)) = self.tools.get_mut(&stream_index) {
                    stored.push_str(&fragment);
                }
                Ok(vec![ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: stream_index,
                    name: Some(Arc::from(name.as_str())),
                    arguments_delta: Arc::from(fragment.as_str()),
                    provider_call_id: None,
                })])
            }
            _ => Err(response_error("Anthropic content_block_delta is invalid")),
        }
    }

    fn consume_block_stop(
        &mut self,
        event: &WireEvent,
    ) -> Result<Vec<ModelStreamItem>, ModelError> {
        let index = event
            .index
            .ok_or_else(|| response_error("Anthropic content block omitted its index"))?;
        self.blocks
            .remove(&index)
            .ok_or_else(|| response_error("Anthropic content_block_stop referenced no block"))?;
        Ok(Vec::new())
    }

    fn consume_message_delta(
        &mut self,
        event: WireEvent,
    ) -> Result<Vec<ModelStreamItem>, ModelError> {
        if let Some(usage) = event.usage {
            return self.apply_usage(&usage);
        }
        Ok(Vec::new())
    }

    fn apply_usage(&mut self, usage: &WireUsage) -> Result<Vec<ModelStreamItem>, ModelError> {
        // Anthropic reports both cache counters on every completion, including
        // zeroes when no cache breakpoint was sent. A run only accepts counters
        // its `RunLimits` registered, so report cache activity only when there
        // is some.
        let mut counters = BTreeMap::new();
        if let Some(value) = usage.cache_creation_input_tokens.filter(|value| *value > 0) {
            counters.insert(
                LimitKey::parse(CACHE_CREATION_KEY).expect("cache key"),
                value,
            );
        }
        if let Some(value) = usage.cache_read_input_tokens.filter(|value| *value > 0) {
            counters.insert(LimitKey::parse(CACHE_READ_KEY).expect("cache key"), value);
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
            .map_err(|_| response_error("Anthropic usage is invalid"))?;
        Ok(vec![ModelStreamItem::Usage(UsageDelta {
            usage: self.usage.clone(),
        })])
    }

    fn finish(self) -> Result<ModelResponse, ModelError> {
        let completion_id = self
            .completion_id
            .ok_or_else(|| response_error("Anthropic stream omitted its completion ID"))?;
        let mut assistant = Vec::new();
        if let Some(signature) = self.thinking_signature {
            let payload = OpaquePayload::json(
                RawJson::parse(
                    serde_json::to_vec(&serde_json::json!({ "signature": signature }))
                        .map_err(|_| response_error("Anthropic thinking signature is invalid"))?
                        .as_slice(),
                )
                .map_err(|_| response_error("Anthropic thinking signature is invalid"))?,
            );
            assistant.push(ContentBlock::Opaque(
                OpaqueBlock::try_new(THINKING_SIGNATURE_MEDIA_TYPE, payload)
                    .map_err(|_| response_error("Anthropic thinking signature is invalid"))?,
            ));
        }
        if self.structured && !self.text.is_empty() {
            let value = RawJson::parse(self.text.as_bytes())
                .map_err(|_| response_error("Anthropic structured output is not valid JSON"))?;
            assistant.push(ContentBlock::Json(JsonBlock::new(value)));
        } else if !self.text.is_empty() {
            assistant.push(ContentBlock::Text(TextBlock::try_new(self.text).map_err(
                |_| response_error("Anthropic assistant text exceeds the kernel bound"),
            )?));
        }
        let mut tool_calls = Vec::with_capacity(self.tools.len());
        for (expected, (index, (_id, name, arguments))) in (0_u32..).zip(self.tools) {
            if expected != index {
                return Err(response_error(
                    "Anthropic tool-call indices are not contiguous",
                ));
            }
            let arguments = if arguments.is_empty() {
                RawJson::parse(b"{}")
                    .map_err(|_| response_error("Anthropic tool-call arguments are invalid JSON"))?
            } else {
                RawJson::parse(arguments.as_bytes())
                    .map_err(|_| response_error("Anthropic tool-call arguments are invalid JSON"))?
            };
            tool_calls.push(ModelToolCall {
                name: Arc::from(name),
                arguments,
                provider_call_id: None,
            });
        }
        let provider_ids = ProviderIds::try_new(
            Some(self.request_id.as_str()),
            Some(completion_id.as_str()),
            None::<&str>,
        )
        .map_err(|_| response_error("Anthropic provider identifiers are invalid"))?;
        Ok(ModelResponse {
            assistant_content: assistant.into(),
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
        "Anthropic request was cancelled",
    )
}

fn transport_error(source: &reqwest::Error) -> ModelError {
    if source.is_timeout() {
        error(
            TIMEOUT,
            ErrorCategory::Deadline,
            true,
            "Anthropic request timed out",
        )
    } else {
        error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "Anthropic transport failed",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_tools_usage_and_thinking() {
        let mut assembly = CompletionAssembly::new("request-1".to_owned(), false);
        assembly
            .consume(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg-1","usage":{"input_tokens":4,"output_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":1}}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            )
            .unwrap();
        let items = assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"consider"}}"#,
            )
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::ReasoningDelta(_)));
        assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"hello"}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_stop",
                r#"{"type":"content_block_stop","index":1}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_start",
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"x\":1}"}}"#,
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
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}"#,
            )
            .unwrap();
        let response = assembly.finish().unwrap();
        assert_eq!(response.completion_id.as_ref(), "msg-1");
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Opaque(_)
        ));
        assert!(matches!(
            response.assistant_content[1],
            ContentBlock::Text(_)
        ));
        assert_eq!(response.tool_calls[0].name.as_ref(), "lookup");
        assert_eq!(response.tool_calls[0].arguments.as_str(), r#"{"x":1}"#);
        assert_eq!(response.usage.input_tokens(), Some(4));
        assert_eq!(response.usage.output_tokens(), Some(3));
        assert_eq!(
            *response
                .usage
                .extension_counters()
                .get(&LimitKey::parse(CACHE_CREATION_KEY).expect("key"))
                .expect("cache counter"),
            2
        );
    }

    #[test]
    fn zero_cache_counters_are_not_reported() {
        let mut assembly = CompletionAssembly::new("request-2".to_owned(), false);
        assembly
            .consume(
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg-2","usage":{"input_tokens":608,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":22}}}"#,
            )
            .unwrap();
        assembly
            .consume(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":608,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":69}}"#,
            )
            .unwrap();
        let response = assembly.finish().unwrap();
        assert!(response.usage.extension_counters().is_empty());
        assert_eq!(response.usage.input_tokens(), Some(608));
        assert_eq!(response.usage.output_tokens(), Some(69));
    }
}
