//! Config-driven multi-protocol gateway `Model` implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{PoisonError, RwLock};

use finstack_ai_runtime::{
    AnthropicMessagesAssembly, ContentBlock, ErrorCategory, MODEL_RESPONSE_MISMATCH,
    MODEL_STREAM_LIMIT_EXCEEDED, Metadata, Model, ModelCapabilities, ModelDescriptor, ModelError,
    ModelEventStream, ModelName, ModelReconcileResult, ModelRequest, ModelRequestDraft,
    ModelStreamItem, ModelTokenEstimate, NdjsonParser, OllamaChatAssembly, OpenAiChatAssembly,
    OpenAiResponsesAssembly, OutputSpec, PendingModelEffect, ReconcileContext,
    SUBMIT_FINAL_OUTPUT_TOOL, SseEventParser, SseParseError, StreamNormError, StreamNormKind,
};
use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::config::{provider_name, request_error};
use crate::{CredentialStore, GatewayModelSpec, GatewayRouteConfig, WireProtocol};

const STREAM_CHANNEL_CAPACITY: usize = 32;
const HTTP_ERROR: &str = "gateway_http_error";
const TRANSPORT_ERROR: &str = "gateway_transport_error";
const TIMEOUT: &str = "gateway_timeout";
const CANCELLED: &str = "gateway_cancelled";

/// Reusable config-driven gateway provider.
pub struct GatewayProvider {
    client: reqwest::Client,
    route: GatewayRouteConfig,
    credentials: CredentialStore,
    models: RwLock<BTreeMap<ModelName, GatewayModelSpec>>,
}

impl fmt::Debug for GatewayProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayProvider")
            .field("route", &self.route)
            .field("credentials", &self.credentials)
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

impl GatewayProvider {
    /// Construct one provider and its reusable pooled HTTP client.
    ///
    /// The factory never reads environment variables. Credentials are resolved
    /// per request from the supplied store.
    ///
    /// # Errors
    ///
    /// Rejects an empty/duplicate model catalog or invalid transport configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_gateway::{
    ///     CredentialReference, CredentialStore, GatewayCapabilityFlags, GatewayModelConfig,
    ///     GatewayModelSpec, GatewayProvider, GatewayRouteConfig, WireProtocol,
    /// };
    /// use finstack_ai_runtime::{
    ///     InputCapabilities, Model, StructuredOutputCapability, TokenEstimatorRef,
    ///     TokenEstimatorSource,
    /// };
    /// use std::sync::Arc;
    ///
    /// let route = GatewayRouteConfig::try_new(
    ///     WireProtocol::OpenaiChat,
    ///     "http://127.0.0.1:9/v1/chat/completions",
    ///     CredentialReference::try_new("local").expect("reference"),
    /// )
    /// .expect("route");
    /// let spec = GatewayModelSpec::try_from_config(GatewayModelConfig {
    ///     name: Some("fixture-model".to_owned()),
    ///     hard_input_bytes: Some(1_000_000),
    ///     context_window_tokens: Some(8_192),
    ///     max_output_tokens: Some(1_024),
    ///     reserved_output_tokens: Some(1_024),
    ///     provider_overhead_tokens: Some(64),
    ///     estimator: Some(TokenEstimatorRef {
    ///         id: Arc::from("gateway.utf8-byte-upper-bound"),
    ///         version: Arc::from("1"),
    ///         source: TokenEstimatorSource::ConservativeUpperBound,
    ///     }),
    ///     capabilities: Some(GatewayCapabilityFlags {
    ///         input: InputCapabilities {
    ///             text: true,
    ///             json: true,
    ///             images: false,
    ///             audio: false,
    ///             files: false,
    ///         },
    ///         native_tool_calls: true,
    ///         parallel_tool_calls: false,
    ///         structured_output: StructuredOutputCapability::Unsupported,
    ///         reasoning: false,
    ///         prompt_cache: false,
    ///         resumable_stream: false,
    ///         idempotent_requests: false,
    ///     }),
    /// })
    /// .expect("spec");
    /// let mut store = CredentialStore::empty();
    /// store
    ///     .insert("local", finstack_ai_provider_gateway::Authentication::None)
    ///     .expect("store");
    /// let provider = GatewayProvider::try_new(route, vec![spec], store).expect("provider");
    /// assert_eq!(provider.descriptor().provider.as_ref(), "gateway");
    /// ```
    pub fn try_new(
        route: GatewayRouteConfig,
        models: Vec<GatewayModelSpec>,
        credentials: CredentialStore,
    ) -> Result<Self, ModelError> {
        let _ = route.endpoint()?;
        let by_name = catalog_from_models(models)?;
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|_| request_error("provider HTTP client could not be built"))?;
        Ok(Self {
            client,
            route,
            credentials,
            models: RwLock::new(by_name),
        })
    }

    fn model_spec(&self, name: &ModelName) -> Result<GatewayModelSpec, ModelError> {
        self.models
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| request_error("requested model is not configured"))
    }
}

fn catalog_from_models(
    models: Vec<GatewayModelSpec>,
) -> Result<BTreeMap<ModelName, GatewayModelSpec>, ModelError> {
    let mut by_name = BTreeMap::new();
    for model in models {
        if by_name.insert(model.name().clone(), model).is_some() {
            return Err(request_error("provider contains a duplicate model name"));
        }
    }
    if by_name.is_empty() {
        return Err(request_error("provider requires at least one model"));
    }
    let descriptor = ModelDescriptor {
        provider: provider_name(),
        models: by_name.keys().cloned().collect::<Vec<_>>().into(),
        metadata: Metadata::empty(),
    };
    descriptor.validate()?;
    Ok(by_name)
}

impl Model for GatewayProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        ModelDescriptor {
            provider: provider_name(),
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
            GatewayModelSpec::capabilities,
        )
    }

    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        let spec = self.model_spec(model)?;
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len())
                .map_err(|_| request_error("gateway request length overflowed the estimator"))?,
            estimator: spec.estimator(),
        })
    }

    fn request(
        &self,
        request: ModelRequest,
    ) -> finstack_ai_runtime::PortFuture<Result<ModelEventStream, ModelError>> {
        let client = self.client.clone();
        let route = self.route.clone();
        let credentials = self.credentials.clone();
        let model = self.model_spec(&request.draft.model);
        Box::pin(async move {
            let model = model?;
            let authentication = credentials
                .resolve(route.auth())
                .ok_or_else(|| request_error("credential reference could not be resolved"))?;
            let headers = route.header_map(authentication)?;
            let payload = serialize_wire_request(route.wire_protocol(), &request.draft, &model)?;
            let endpoint = route.endpoint()?;
            let request_id = request.call.request_id.to_string();
            let cancellation = request.call.run.cancellation;
            let timeout = route.request_timeout();
            let mut builder = client
                .post(endpoint)
                .headers(headers)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(timeout)
                .body(payload);
            if route.wire_protocol() != WireProtocol::AnthropicMessages {
                builder = builder.header("x-client-request-id", &request_id);
            }
            let send = builder.send();
            let response = tokio::select! {
                () = cancellation.cancelled() => return Err(cancelled_error()),
                response = send => response.map_err(|source| transport_error(&source))?,
            };
            if !response.status().is_success() {
                let retryable = matches!(response.status().as_u16(), 408 | 409 | 429 | 500..=599);
                return Err(adapter_error(
                    HTTP_ERROR,
                    ErrorCategory::Model,
                    retryable,
                    "gateway endpoint returned an unsuccessful status",
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
                route.wire_protocol(),
                route.max_event_bytes(),
                route.max_stream_bytes(),
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

enum StreamAssembly {
    OpenaiResponses(OpenAiResponsesAssembly),
    OpenaiChat(OpenAiChatAssembly),
    AnthropicMessages(AnthropicMessagesAssembly),
    OllamaChat(OllamaChatAssembly),
}

impl StreamAssembly {
    fn new(protocol: WireProtocol, request_id: String, structured: bool) -> Self {
        match protocol {
            WireProtocol::OpenaiResponses => {
                Self::OpenaiResponses(OpenAiResponsesAssembly::new(request_id, structured))
            }
            WireProtocol::OpenaiChat => {
                Self::OpenaiChat(OpenAiChatAssembly::new(request_id, structured))
            }
            WireProtocol::AnthropicMessages => {
                Self::AnthropicMessages(AnthropicMessagesAssembly::new(request_id, structured))
            }
            WireProtocol::OllamaChat => {
                Self::OllamaChat(OllamaChatAssembly::new(request_id, structured, None))
            }
        }
    }

    fn consume_sse(
        &mut self,
        name: Option<&str>,
        data: &str,
    ) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        match self {
            Self::OpenaiResponses(assembly) => assembly.consume(data),
            Self::OpenaiChat(assembly) => assembly.consume(data),
            Self::AnthropicMessages(assembly) => {
                let name = name.ok_or_else(|| stream_norm("SSE event name is missing"))?;
                assembly.consume(name, data)
            }
            Self::OllamaChat(_) => Err(stream_norm("Ollama chat does not consume SSE events")),
        }
    }

    fn consume_ndjson(&mut self, line: &str) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        match self {
            Self::OllamaChat(assembly) => assembly.consume(line),
            _ => Err(stream_norm(
                "selected wire protocol does not consume NDJSON",
            )),
        }
    }

    fn finish_chat_after_body(&mut self) -> Result<Vec<ModelStreamItem>, StreamNormError> {
        match self {
            Self::OpenaiChat(assembly) => {
                let finished =
                    core::mem::replace(assembly, OpenAiChatAssembly::new(String::new(), false));
                finished.finish_after_body()
            }
            _ => Ok(Vec::new()),
        }
    }

    fn finish_ollama(&mut self) -> Result<ModelStreamItem, StreamNormError> {
        match self {
            Self::OllamaChat(assembly) => {
                let finished = core::mem::replace(
                    assembly,
                    OllamaChatAssembly::new(String::new(), false, None),
                );
                finished.finish().map(ModelStreamItem::Completed)
            }
            _ => Err(stream_norm(
                "selected wire protocol does not finish as NDJSON",
            )),
        }
    }

    fn ollama_done(&self) -> bool {
        match self {
            Self::OllamaChat(assembly) => assembly.done,
            _ => false,
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "stream driver keeps protocol, limits, and cancellation on one task"
)]
async fn drive_response(
    response: reqwest::Response,
    sender: mpsc::Sender<Result<ModelStreamItem, ModelError>>,
    cancellation: finstack_ai_runtime::CancellationSignal,
    request_id: String,
    structured: bool,
    protocol: WireProtocol,
    max_event_bytes: usize,
    max_stream_bytes: usize,
) {
    match protocol {
        WireProtocol::OllamaChat => {
            drive_ndjson(
                response,
                sender,
                cancellation,
                request_id,
                structured,
                max_event_bytes,
                max_stream_bytes,
            )
            .await;
        }
        WireProtocol::OpenaiResponses
        | WireProtocol::OpenaiChat
        | WireProtocol::AnthropicMessages => {
            drive_sse(
                response,
                sender,
                cancellation,
                request_id,
                structured,
                protocol,
                max_event_bytes,
                max_stream_bytes,
            )
            .await;
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "SSE driver keeps protocol, limits, and cancellation on one task"
)]
async fn drive_sse(
    response: reqwest::Response,
    sender: mpsc::Sender<Result<ModelStreamItem, ModelError>>,
    cancellation: finstack_ai_runtime::CancellationSignal,
    request_id: String,
    structured: bool,
    protocol: WireProtocol,
    max_event_bytes: usize,
    max_stream_bytes: usize,
) {
    let mut body = response.bytes_stream();
    let mut parser = SseEventParser::new(max_event_bytes, max_stream_bytes);
    let mut assembly = StreamAssembly::new(protocol, request_id, structured);
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
            if protocol == WireProtocol::OpenaiChat {
                match assembly.finish_chat_after_body() {
                    Ok(items) => {
                        for item in items {
                            if sender.send(Ok(item)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(map_norm(error))).await;
                    }
                }
                return;
            }
            let _ = sender
                .send(Err(request_error(
                    "gateway SSE stream ended before a terminal event",
                )))
                .await;
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
                let _ = sender.send(Err(map_sse(error))).await;
                return;
            }
        };
        for event in events {
            match assembly.consume_sse(event.name.as_deref(), &event.data) {
                Ok(items) => {
                    let mut completed = false;
                    for item in items {
                        completed |= matches!(item, ModelStreamItem::Completed(_));
                        if sender.send(Ok(item)).await.is_err() {
                            return;
                        }
                    }
                    if completed {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(map_norm(error))).await;
                    return;
                }
            }
        }
    }
}

async fn drive_ndjson(
    response: reqwest::Response,
    sender: mpsc::Sender<Result<ModelStreamItem, ModelError>>,
    cancellation: finstack_ai_runtime::CancellationSignal,
    request_id: String,
    structured: bool,
    max_event_bytes: usize,
    max_stream_bytes: usize,
) {
    let mut body = response.bytes_stream();
    let mut parser = NdjsonParser::new(max_event_bytes, max_stream_bytes);
    let mut assembly = StreamAssembly::new(WireProtocol::OllamaChat, request_id, structured);
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
            let leftover = match parser.finish() {
                Ok(lines) => lines,
                Err(error) => {
                    let _ = sender.send(Err(map_ndjson(error))).await;
                    return;
                }
            };
            if let Err(error) = consume_ndjson_lines(&mut assembly, leftover, &sender).await {
                let _ = sender.send(Err(error)).await;
                return;
            }
            if !assembly.ollama_done() {
                let _ = sender
                    .send(Err(request_error(
                        "Ollama NDJSON stream ended before done:true",
                    )))
                    .await;
                return;
            }
            let _ = sender
                .send(assembly.finish_ollama().map_err(map_norm))
                .await;
            return;
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(source) => {
                let _ = sender.send(Err(transport_error(&source))).await;
                return;
            }
        };
        let lines = match parser.push(&chunk) {
            Ok(lines) => lines,
            Err(error) => {
                let _ = sender.send(Err(map_ndjson(error))).await;
                return;
            }
        };
        match consume_ndjson_lines(&mut assembly, lines, &sender).await {
            Ok(true) => {
                let _ = sender
                    .send(assembly.finish_ollama().map_err(map_norm))
                    .await;
                return;
            }
            Ok(false) => {}
            Err(error) => {
                let _ = sender.send(Err(error)).await;
                return;
            }
        }
    }
}

async fn consume_ndjson_lines(
    assembly: &mut StreamAssembly,
    lines: Vec<String>,
    sender: &mpsc::Sender<Result<ModelStreamItem, ModelError>>,
) -> Result<bool, ModelError> {
    for line in lines {
        let items = assembly.consume_ndjson(&line).map_err(map_norm)?;
        for item in items {
            if sender.send(Ok(item)).await.is_err() {
                return Ok(assembly.ollama_done());
            }
        }
        if assembly.ollama_done() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn serialize_wire_request(
    protocol: WireProtocol,
    draft: &ModelRequestDraft,
    model: &GatewayModelSpec,
) -> Result<Vec<u8>, ModelError> {
    draft.validate()?;
    if draft.model != *model.name() {
        return Err(request_error("requested model is not configured"));
    }
    let max_output = draft
        .limits
        .max_output_tokens
        .min(model.max_output_tokens());
    let body = match protocol {
        WireProtocol::OpenaiResponses => openai_responses_body(draft, max_output)?,
        WireProtocol::OpenaiChat => openai_chat_body(draft, max_output)?,
        WireProtocol::AnthropicMessages => anthropic_messages_body(draft, max_output)?,
        WireProtocol::OllamaChat => ollama_chat_body(draft, max_output)?,
    };
    serde_json::to_vec(&body).map_err(|_| request_error("provider request could not be encoded"))
}

fn openai_responses_body(
    draft: &ModelRequestDraft,
    max_output_tokens: u64,
) -> Result<Value, ModelError> {
    Ok(json!({
        "model": draft.model.as_str(),
        "input": conversation_text(draft)?,
        "stream": true,
        "store": false,
        "max_output_tokens": max_output_tokens,
        "tools": function_tools(draft, "parameters")?,
    }))
}

fn openai_chat_body(
    draft: &ModelRequestDraft,
    max_output_tokens: u64,
) -> Result<Value, ModelError> {
    Ok(json!({
        "model": draft.model.as_str(),
        "messages": chat_messages(draft)?,
        "stream": true,
        "max_tokens": max_output_tokens,
        "tools": function_tools(draft, "parameters")?,
    }))
}

fn anthropic_messages_body(
    draft: &ModelRequestDraft,
    max_output_tokens: u64,
) -> Result<Value, ModelError> {
    Ok(json!({
        "model": draft.model.as_str(),
        "max_tokens": max_output_tokens,
        "stream": true,
        "messages": chat_messages(draft)?,
        "tools": anthropic_tools(draft)?,
    }))
}

fn ollama_chat_body(
    draft: &ModelRequestDraft,
    max_output_tokens: u64,
) -> Result<Value, ModelError> {
    Ok(json!({
        "model": draft.model.as_str(),
        "messages": chat_messages(draft)?,
        "stream": true,
        "tools": function_tools(draft, "parameters")?,
        "options": { "num_predict": max_output_tokens },
    }))
}

fn conversation_text(draft: &ModelRequestDraft) -> Result<Vec<Value>, ModelError> {
    let mut input = Vec::new();
    for message in draft.messages.iter() {
        input.push(json!({
            "type": "message",
            "role": role_name(message.role())?,
            "content": [{ "type": "input_text", "text": render_text(message.content())? }],
        }));
    }
    Ok(input)
}

fn chat_messages(draft: &ModelRequestDraft) -> Result<Vec<Value>, ModelError> {
    let mut messages = Vec::new();
    for message in draft.messages.iter() {
        messages.push(json!({
            "role": role_name(message.role())?,
            "content": render_text(message.content())?,
        }));
    }
    Ok(messages)
}

fn function_tools(draft: &ModelRequestDraft, schema_key: &str) -> Result<Vec<Value>, ModelError> {
    let mut tools = Vec::new();
    for tool in draft.tools.iter() {
        if tool.model_name.as_ref() == SUBMIT_FINAL_OUTPUT_TOOL {
            continue;
        }
        let schema = raw_json(&tool.input_schema)?;
        tools.push(json!({
            "type": "function",
            "function": {
                "name": tool.model_name.as_ref(),
                "description": tool.description.as_ref(),
                schema_key: schema,
            }
        }));
    }
    Ok(tools)
}

fn anthropic_tools(draft: &ModelRequestDraft) -> Result<Vec<Value>, ModelError> {
    let mut tools = Vec::new();
    for tool in draft.tools.iter() {
        tools.push(json!({
            "name": tool.model_name.as_ref(),
            "description": tool.description.as_ref(),
            "input_schema": raw_json(&tool.input_schema)?,
        }));
    }
    Ok(tools)
}

fn role_name(role: finstack_ai_runtime::MessageRole) -> Result<&'static str, ModelError> {
    match role {
        finstack_ai_runtime::MessageRole::System | finstack_ai_runtime::MessageRole::Developer => {
            Ok("system")
        }
        finstack_ai_runtime::MessageRole::User => Ok("user"),
        finstack_ai_runtime::MessageRole::Assistant => Ok("assistant"),
        finstack_ai_runtime::MessageRole::Tool => Err(request_error(
            "gateway request mapping does not accept tool-role messages",
        )),
    }
}

fn render_text(blocks: &[ContentBlock]) -> Result<String, ModelError> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => parts.push(text.text().to_owned()),
            _ => {
                return Err(request_error(
                    "gateway request mapping accepts text content only",
                ));
            }
        }
    }
    Ok(parts.join(""))
}

fn raw_json(value: &finstack_ai_runtime::RawJson) -> Result<Value, ModelError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| request_error("canonical provider JSON could not be decoded"))
}

fn stream_norm(message: &'static str) -> StreamNormError {
    StreamNormError {
        kind: StreamNormKind::Stream,
        message,
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err passes the normalization error by value"
)]
fn map_norm(error: StreamNormError) -> ModelError {
    match error.kind {
        StreamNormKind::Limit => adapter_error(
            MODEL_STREAM_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            false,
            error.message,
        ),
        StreamNormKind::Stream | StreamNormKind::Response | StreamNormKind::Incomplete => {
            adapter_error(
                MODEL_RESPONSE_MISMATCH,
                ErrorCategory::Validation,
                false,
                error.message,
            )
        }
    }
}

fn map_sse(error: SseParseError) -> ModelError {
    match error {
        SseParseError::Limit => adapter_error(
            MODEL_STREAM_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            false,
            "provider response exceeded a configured stream limit",
        ),
        SseParseError::InvalidUtf8 | SseParseError::DuplicateName => adapter_error(
            MODEL_RESPONSE_MISMATCH,
            ErrorCategory::Validation,
            false,
            "SSE event could not be framed",
        ),
    }
}

fn map_ndjson(error: finstack_ai_runtime::NdjsonError) -> ModelError {
    match error {
        finstack_ai_runtime::NdjsonError::Limit => adapter_error(
            MODEL_STREAM_LIMIT_EXCEEDED,
            ErrorCategory::Limit,
            false,
            "provider response exceeded a configured stream limit",
        ),
        finstack_ai_runtime::NdjsonError::InvalidUtf8 => adapter_error(
            MODEL_RESPONSE_MISMATCH,
            ErrorCategory::Validation,
            false,
            "Ollama NDJSON line is not UTF-8",
        ),
    }
}

fn cancelled_error() -> ModelError {
    adapter_error(
        CANCELLED,
        ErrorCategory::Cancellation,
        false,
        "gateway request was cancelled",
    )
}

fn transport_error(source: &reqwest::Error) -> ModelError {
    if source.is_timeout() {
        adapter_error(
            TIMEOUT,
            ErrorCategory::Deadline,
            true,
            "gateway request timed out",
        )
    } else {
        adapter_error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "gateway transport failed",
        )
    }
}

fn adapter_error(
    code: &'static str,
    category: ErrorCategory,
    retryable: bool,
    message: &'static str,
) -> ModelError {
    ModelError::try_new(code, category, retryable, message, Metadata::empty())
        .expect("gateway adapter error is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, EffectId, InputCapabilities, LaneId,
        MODEL_REQUEST_INVALID, Message, MessageId, MessageRole, ModelCallContext, ModelName,
        ModelRequestDraft, ModelRequestId, ModelRequestLimits, ModelSettings, ModelTerminal,
        OpenAiChatAssembly, OperationLocator, PrincipalRef, ProviderIds, RawJson, RunCallContext,
        RunId, SessionId, StructuredOutputCapability, TextBlock, Timestamp, TokenEstimatorRef,
        TokenEstimatorSource, Usage,
    };
    use finstack_ai_test::{ModelConformanceCase, check_model_conformance};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use crate::{
        Authentication, CredentialReference, GatewayCapabilityFlags, GatewayModelConfig,
        SecretString,
    };

    const OPENAI_CHAT_SSE: &str = include_str!(
        "../../../../fixtures/compatibility/providers/v1/openai-compatible/valid--text.sse"
    );
    const ANTHROPIC_SSE: &str =
        include_str!("../../../../fixtures/compatibility/providers/v1/anthropic/valid--text.sse");
    const OPENAI_RESPONSES_SSE: &str = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello \",\"sequence_number\":1}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\",\"sequence_number\":2}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"sequence_number\":3,\"response\":{\"id\":\"resp-text-1\",\"output\":[],\"usage\":{\"input_tokens\":4,\"output_tokens\":2,\"total_tokens\":6}}}\n\n",
    );
    const OLLAMA_NDJSON: &str = concat!(
        "{\"message\":{\"content\":\"hello \"},\"done\":false}\n",
        "{\"message\":{\"content\":\"world\"},\"done\":false}\n",
        "{\"message\":{\"content\":\"\"},\"done\":true,\"prompt_eval_count\":4,\"eval_count\":2}\n",
    );
    const REQUEST_ID: &str = "01234567-89ab-7cde-89ab-0123456789a5";

    fn estimator() -> TokenEstimatorRef {
        TokenEstimatorRef {
            id: Arc::from("gateway.utf8-byte-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        }
    }

    fn flags() -> GatewayCapabilityFlags {
        GatewayCapabilityFlags {
            input: InputCapabilities {
                text: true,
                json: true,
                images: false,
                audio: false,
                files: false,
            },
            native_tool_calls: true,
            parallel_tool_calls: false,
            structured_output: StructuredOutputCapability::Unsupported,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
        }
    }

    fn model_spec() -> GatewayModelSpec {
        GatewayModelSpec::try_from_config(GatewayModelConfig {
            name: Some("fixture-model".to_owned()),
            hard_input_bytes: Some(1_000_000),
            context_window_tokens: Some(8_192),
            max_output_tokens: Some(1_024),
            reserved_output_tokens: Some(1_024),
            provider_overhead_tokens: Some(64),
            estimator: Some(estimator()),
            capabilities: Some(flags()),
        })
        .expect("spec")
    }

    fn store_none(name: &str) -> CredentialStore {
        let mut store = CredentialStore::empty();
        store.insert(name, Authentication::None).expect("store");
        store
    }

    fn provider(protocol: WireProtocol, endpoint: &str) -> GatewayProvider {
        let route = GatewayRouteConfig::try_new(
            protocol,
            endpoint,
            CredentialReference::try_new("local").expect("reference"),
        )
        .expect("route");
        GatewayProvider::try_new(route, vec![model_spec()], store_none("local")).expect("provider")
    }

    fn request() -> ModelRequest {
        let selected = ModelName::try_new("fixture-model").expect("name");
        ModelRequest {
            call: ModelCallContext {
                run: RunCallContext {
                    locator: OperationLocator::try_new(
                        "tenant-a",
                        SessionId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("session"),
                        LaneId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("lane"),
                        RunId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("run"),
                    )
                    .expect("locator"),
                    authorization: AuthorizationContext {
                        principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                            .expect("principal"),
                        authentication_method: Arc::from("fixture"),
                        assurance_level: Arc::from("test"),
                        roles: Arc::from([]),
                        permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                        safe_claims: Metadata::empty(),
                        policy_version: Arc::from("policy-v1"),
                        decision_id: Arc::from("decision-v1"),
                    },
                    effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789a4")
                        .expect("effect"),
                    attempt: 1,
                    deadline: None,
                    budget_scope_id: None,
                    cancellation: CancellationSignal::new(),
                },
                request_id: ModelRequestId::parse(REQUEST_ID).expect("request id"),
            },
            draft: ModelRequestDraft {
                model: selected,
                messages: Arc::from([Message::try_new(
                    MessageId::parse("01234567-89ab-7cde-89ab-0123456789a6").expect("message"),
                    MessageRole::User,
                    vec![ContentBlock::Text(
                        TextBlock::try_new("hello").expect("text"),
                    )],
                    Timestamp::from_unix_ms(1).expect("ts"),
                    None,
                    ProviderIds::empty(),
                    Metadata::empty(),
                )
                .expect("message")]),
                tools: Arc::from([]),
                output: OutputSpec::PlainText,
                settings: ModelSettings {
                    values: RawJson::parse(b"{}").expect("settings"),
                },
                limits: ModelRequestLimits {
                    max_input_bytes: 1_024,
                    max_input_tokens: 1_024,
                    max_output_tokens: 128,
                },
            },
            continuation_state: None,
        }
    }

    fn expected_chat() -> ModelTerminal {
        let mut assembly = OpenAiChatAssembly::new(REQUEST_ID.to_owned(), false);
        for payload in [
            r#"{"id":"chat-text-1","choices":[{"index":0,"delta":{"content":"hello "},"finish_reason":null}],"usage":null}"#,
            r#"{"id":"chat-text-1","choices":[{"index":0,"delta":{"content":"world"},"finish_reason":"stop"}],"usage":null}"#,
            r#"{"id":"chat-text-1","choices":[],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#,
            "[DONE]",
        ] {
            let items = assembly.consume(payload).expect("chat event");
            if let Some(ModelStreamItem::Completed(response)) = items.into_iter().last() {
                return ModelTerminal::Completed(response);
            }
        }
        panic!("chat fixture omitted a completion");
    }

    fn expected_responses() -> ModelTerminal {
        let mut assembly = OpenAiResponsesAssembly::new(REQUEST_ID.to_owned(), false);
        for payload in [
            r#"{"type":"response.output_text.delta","delta":"hello ","sequence_number":1}"#,
            r#"{"type":"response.output_text.delta","delta":"world","sequence_number":2}"#,
            r#"{"type":"response.completed","sequence_number":3,"response":{"id":"resp-text-1","output":[],"usage":{"input_tokens":4,"output_tokens":2,"total_tokens":6}}}"#,
        ] {
            let items = assembly.consume(payload).expect("responses event");
            if let Some(ModelStreamItem::Completed(response)) = items.into_iter().last() {
                return ModelTerminal::Completed(response);
            }
        }
        panic!("responses fixture omitted a completion");
    }

    fn expected_anthropic() -> ModelTerminal {
        let mut assembly = AnthropicMessagesAssembly::new(REQUEST_ID.to_owned(), false);
        let events = [
            (
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg-text-1","type":"message","role":"assistant","content":[],"model":"fixture-model","stop_reason":null,"usage":{"input_tokens":4,"output_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":1}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello "}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"world"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ];
        for (name, data) in events {
            let items = assembly.consume(name, data).expect("anthropic event");
            if let Some(ModelStreamItem::Completed(response)) = items.into_iter().last() {
                return ModelTerminal::Completed(response);
            }
        }
        panic!("anthropic fixture omitted a completion");
    }

    fn expected_ollama() -> ModelTerminal {
        let mut assembly = OllamaChatAssembly::new(REQUEST_ID.to_owned(), false, None);
        for line in [
            r#"{"message":{"content":"hello "},"done":false}"#,
            r#"{"message":{"content":"world"},"done":false}"#,
            r#"{"message":{"content":""},"done":true,"prompt_eval_count":4,"eval_count":2}"#,
        ] {
            assembly.consume(line).expect("ollama line");
        }
        ModelTerminal::Completed(assembly.finish().expect("ollama complete"))
    }

    async fn serve_body(
        content_type: &str,
        body: &str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let content_type = content_type.to_owned();
        let body = body.as_bytes().to_vec();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4_096];
            let header_end = loop {
                let count = socket.read(&mut buffer).await.expect("read request");
                assert!(count > 0, "request closed before headers");
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .expect("content length");
            while request.len() < header_end + content_length {
                let count = socket.read(&mut buffer).await.expect("read body");
                assert!(count > 0, "request closed before body");
                request.extend_from_slice(&buffer[..count]);
            }
            let response_headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket
                .write_all(response_headers.as_bytes())
                .await
                .expect("write headers");
            socket.write_all(&body).await.expect("write body");
            String::from_utf8(request).expect("request UTF-8")
        });
        (format!("http://{address}"), task)
    }

    async fn assert_protocol(
        protocol: WireProtocol,
        path: &str,
        content_type: &str,
        body: &str,
        expected: ModelTerminal,
    ) {
        let (base, server) = serve_body(content_type, body).await;
        let endpoint = format!("{base}{path}");
        let model = provider(protocol, &endpoint);
        let selected = model.descriptor().models[0].clone();
        check_model_conformance(
            &model,
            ModelConformanceCase {
                model: selected,
                request: request(),
                expected_terminal: expected,
                stream_limits: finstack_ai_runtime::ModelStreamLimits::default(),
            },
        )
        .await
        .expect("public Model contract");
        server.abort();
    }

    #[tokio::test]
    async fn gateway_rejects_unresolved_credential_reference() {
        let route = GatewayRouteConfig::try_new(
            WireProtocol::OpenaiChat,
            "http://127.0.0.1:9/v1/chat/completions",
            CredentialReference::try_new("missing-key").expect("reference"),
        )
        .expect("route");
        let provider =
            GatewayProvider::try_new(route, vec![model_spec()], CredentialStore::empty())
                .expect("construction succeeds with unresolved reference");
        let Err(error) = provider.request(request()).await else {
            panic!("unresolved credential must fail the request");
        };
        assert_eq!(error.code(), MODEL_REQUEST_INVALID);
    }

    #[test]
    fn secret_values_are_redacted_from_provider_debug() {
        const CANARY: &str = "sk-secret-canary-gateway";
        let secret = SecretString::try_new(CANARY).expect("secret");
        let mut store = CredentialStore::empty();
        store
            .insert("prod", Authentication::Bearer(secret))
            .expect("store");
        let route = GatewayRouteConfig::try_new(
            WireProtocol::OpenaiChat,
            "https://api.example.test/v1/chat/completions",
            CredentialReference::try_new("prod").expect("reference"),
        )
        .expect("route");
        let provider =
            GatewayProvider::try_new(route, vec![model_spec()], store).expect("provider");
        let rendered = format!("{provider:?}");
        assert!(!rendered.contains(CANARY));
        assert!(rendered.contains("prod"));
    }

    #[test]
    fn gateway_is_not_a_default_sdk_dependency() {
        let manifest = include_str!("../../../../crates/finstack-ai/Cargo.toml");
        assert!(!manifest.contains("finstack-ai-provider-gateway"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn openai_chat_conformance() {
        assert_protocol(
            WireProtocol::OpenaiChat,
            "/v1/chat/completions",
            "text/event-stream",
            OPENAI_CHAT_SSE,
            expected_chat(),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn openai_responses_conformance() {
        assert_protocol(
            WireProtocol::OpenaiResponses,
            "/v1/responses",
            "text/event-stream",
            OPENAI_RESPONSES_SSE,
            expected_responses(),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn anthropic_messages_conformance() {
        assert_protocol(
            WireProtocol::AnthropicMessages,
            "/v1/messages",
            "text/event-stream",
            ANTHROPIC_SSE,
            expected_anthropic(),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ollama_chat_conformance() {
        assert_protocol(
            WireProtocol::OllamaChat,
            "/api/chat",
            "application/x-ndjson",
            OLLAMA_NDJSON,
            expected_ollama(),
        )
        .await;
    }

    #[test]
    fn usage_fixture_totals_are_stable() {
        let ModelTerminal::Completed(chat) = expected_chat() else {
            panic!("chat");
        };
        assert_eq!(chat.usage, expected_usage(4, 2, 6));
    }

    fn expected_usage(input: u64, output: u64, total: u64) -> Usage {
        Usage::try_new(
            Some(input),
            Some(output),
            Some(total),
            None,
            BTreeMap::new(),
        )
        .expect("usage")
    }
}
