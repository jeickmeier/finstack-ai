//! Native Anthropic Messages provider implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};

use finstack_ai_runtime::{
    AnthropicMessagesAssembly, ErrorCategory, Metadata, Model, ModelCapabilities, ModelDescriptor,
    ModelError, ModelEventStream, ModelName, ModelReconcileResult, ModelRequest, ModelStreamItem,
    ModelTokenEstimate, OutputSpec, PendingModelEffect, ReconcileContext, StreamNormError,
    StreamNormKind,
};
use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
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
                context_profile: finstack_ai_runtime::ModelContextProfile {
                    provider: Arc::from("anthropic"),
                    model: model.clone(),
                    hard_input_bytes: 0,
                    context_window_tokens: 0,
                    max_output_tokens: 0,
                    reserved_output_tokens: 0,
                    provider_overhead_tokens: 0,
                    estimator: estimator_ref(),
                },
                native_tool_calls: false,
                parallel_tool_calls: false,
                structured_output: finstack_ai_runtime::StructuredOutputCapability::Prompted,
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
    let mut assembly = AnthropicMessagesAssembly::new(request_id, structured);
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
            match assembly.consume(&event.name, &event.data) {
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

#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err passes the normalization error by value"
)]
fn map_norm(error: StreamNormError) -> ModelError {
    match error.kind {
        StreamNormKind::Limit => crate::error::stream_limit_error(),
        StreamNormKind::Stream => stream_error(error.message),
        StreamNormKind::Response | StreamNormKind::Incomplete => response_error(error.message),
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
    use finstack_ai_runtime::{ContentBlock, LimitKey};

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "covers the recorded Anthropic event sequence"
    )]
    fn assembles_text_tools_usage_and_thinking() {
        let mut assembly = AnthropicMessagesAssembly::new("request-1".to_owned(), false);
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
        let items = assembly
            .consume("message_stop", r#"{"type":"message_stop"}"#)
            .unwrap();
        let ModelStreamItem::Completed(response) = &items[0] else {
            panic!("expected completed");
        };
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
                .get(&LimitKey::parse("anthropic.cache_creation_input_tokens").expect("key"))
                .expect("cache counter"),
            2
        );
    }

    #[test]
    fn zero_cache_counters_are_not_reported() {
        let mut assembly = AnthropicMessagesAssembly::new("request-2".to_owned(), false);
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
        let items = assembly
            .consume("message_stop", r#"{"type":"message_stop"}"#)
            .unwrap();
        let ModelStreamItem::Completed(response) = &items[0] else {
            panic!("expected completed");
        };
        assert!(response.usage.extension_counters().is_empty());
        assert_eq!(response.usage.input_tokens(), Some(608));
        assert_eq!(response.usage.output_tokens(), Some(69));
    }

    #[test]
    fn unknown_model_uses_a_zeroed_profile_and_prompted_structured_output() {
        let config = AnthropicConfig::try_new("http://127.0.0.1:9").expect("config");
        let model =
            AnthropicModelConfig::try_new("claude-test", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model");
        let provider = AnthropicProvider::try_new(config, vec![model]).expect("provider");
        let unknown = ModelName::try_new("claude-unknown").expect("name");
        let capabilities = provider.capabilities(&unknown);
        assert_eq!(capabilities.context_profile.model, unknown);
        assert_eq!(capabilities.context_profile.context_window_tokens, 0);
        assert_eq!(
            capabilities.structured_output,
            finstack_ai_runtime::StructuredOutputCapability::Prompted
        );
    }
}
