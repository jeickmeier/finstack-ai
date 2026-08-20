//! Native Ollama `/api/chat` provider implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};

use finstack_ai_kernel::{ContentBlock, ErrorCategory, Metadata, OutputSpec, PendingModelEffect};
use finstack_ai_runtime::{
    MediaResolver, Model, ModelCapabilities, ModelDescriptor, ModelError, ModelEventStream,
    ModelName, ModelReconcileResult, ModelRequest, ModelRequestDraft, ModelStreamItem,
    ModelTokenEstimate, OllamaChatAssembly, OllamaReplayEntry, ReconcileContext, ResolvedMedia,
    StreamNormError, StreamNormKind,
};
use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
use tokio::sync::mpsc;

use crate::config::estimator_ref;
use crate::error::{
    CANCELLED, HTTP_ERROR, RESPONSE_INVALID, TIMEOUT, TRANSPORT_ERROR, error, response_error,
    stream_error,
};
use crate::ndjson::NdjsonParser;
use crate::request::{ChatRequest, ReplayEntry, serialize_request};
use crate::{OllamaConfig, OllamaModelConfig};

const STREAM_CHANNEL_CAPACITY: usize = 32;

/// Reusable native Ollama `/api/chat` provider.
pub struct OllamaProvider {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    config: OllamaConfig,
    models: RwLock<BTreeMap<ModelName, OllamaModelConfig>>,
}

impl fmt::Debug for OllamaProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OllamaProvider")
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

impl OllamaProvider {
    /// Construct one provider and its reusable pooled HTTP client.
    ///
    /// # Errors
    ///
    /// Rejects an empty/duplicate model catalog or invalid transport configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
    /// use finstack_ai_runtime::Model;
    ///
    /// let config = OllamaConfig::try_new("http://127.0.0.1:9").expect("config");
    /// let model = OllamaModelConfig::try_new(
    ///     "gemma3",
    ///     1_000_000,
    ///     128_000,
    ///     4_096,
    ///     4_096,
    ///     256,
    /// )
    /// .expect("model");
    /// let provider = OllamaProvider::try_new(config, vec![model]).expect("provider");
    /// assert_eq!(provider.descriptor().provider.as_ref(), "ollama");
    /// ```
    pub fn try_new(
        config: OllamaConfig,
        models: Vec<OllamaModelConfig>,
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
    pub fn replace_model_catalog(&self, models: Vec<OllamaModelConfig>) -> Result<(), ModelError> {
        let by_name = catalog_from_models(models)?;
        *self.models.write().unwrap_or_else(PoisonError::into_inner) = by_name;
        Ok(())
    }

    /// Refresh advertised capabilities for one configured model from a local table.
    ///
    /// # Errors
    ///
    /// Returns `ollama_request_invalid` when the model is not configured.
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

    fn model_config(&self, name: &ModelName) -> Result<OllamaModelConfig, ModelError> {
        self.models
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| crate::error::request_error("requested model is not configured"))
    }
}

const MAX_INLINE_MEDIA_BYTES: usize = 8 * 1_048_576;

async fn resolve_draft_media(
    media_resolver: Option<&Arc<dyn MediaResolver>>,
    draft: &ModelRequestDraft,
) -> Result<BTreeMap<Arc<str>, ResolvedMedia>, ModelError> {
    let mut media_by_id = BTreeMap::new();
    for message in draft.messages.iter() {
        for block in message.content() {
            let ContentBlock::Image(media) = block else {
                continue;
            };
            let id: Arc<str> = Arc::from(media.blob().id());
            if media_by_id.contains_key(&id) {
                continue;
            }
            let Some(media_resolver) = media_resolver else {
                return Err(crate::error::request_error(
                    "media content requires a configured media resolver",
                ));
            };
            let payload = media_resolver
                .resolve(media.blob())
                .await
                .map_err(map_resolve)?;
            if let ResolvedMedia::Bytes { bytes, .. } = &payload
                && bytes.len() > MAX_INLINE_MEDIA_BYTES
            {
                return Err(crate::error::stream_limit_error());
            }
            media_by_id.insert(id, payload);
        }
    }
    Ok(media_by_id)
}

fn map_resolve(error: finstack_ai_runtime::MediaResolveError) -> ModelError {
    use finstack_ai_runtime::MediaResolveKind;
    match error.kind {
        MediaResolveKind::NotFound => crate::error::request_error(error.message),
        MediaResolveKind::Unavailable => crate::error::error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "Ollama media resolution is unavailable",
        ),
        MediaResolveKind::Limit => crate::error::stream_limit_error(),
    }
}

fn catalog_from_models(
    models: Vec<OllamaModelConfig>,
) -> Result<BTreeMap<ModelName, OllamaModelConfig>, ModelError> {
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
        provider: Arc::from("ollama"),
        models: by_name.keys().cloned().collect::<Vec<_>>().into(),
        metadata: Metadata::empty(),
    };
    descriptor.validate()?;
    Ok(by_name)
}

impl Model for OllamaProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        ModelDescriptor {
            provider: Arc::from("ollama"),
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
                    provider: Arc::from("ollama"),
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
            OllamaModelConfig::capabilities,
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
                    "Ollama request length overflowed the estimator",
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
        let media_resolver = self.config.media_resolver();
        Box::pin(async move {
            let model = model?;
            let resolved_media =
                resolve_draft_media(media_resolver.as_ref(), &request.draft).await?;
            let prepared = ChatRequest::try_from_draft(
                &request.draft,
                &model,
                request.continuation_state.as_ref(),
                &resolved_media,
            )?;
            let payload = serialize_request(&prepared.request)?;
            let request_id = request.call.request_id.to_string();
            let cancellation = request.call.run.cancellation;
            let send = client
                .post(endpoint)
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
                    "Ollama endpoint returned an unsuccessful status",
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
                prepared.matched_replay,
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

#[expect(
    clippy::too_many_arguments,
    reason = "stream driver keeps timeout, limits, and replay on one task"
)]
async fn drive_response(
    response: reqwest::Response,
    sender: mpsc::Sender<Result<ModelStreamItem, ModelError>>,
    cancellation: finstack_ai_runtime::CancellationSignal,
    request_id: String,
    structured: bool,
    matched_replay: Option<Vec<ReplayEntry>>,
    max_event_bytes: usize,
    max_stream_bytes: usize,
) {
    let mut body = response.bytes_stream();
    let mut parser = NdjsonParser::new(max_event_bytes, max_stream_bytes);
    let mut assembly = OllamaChatAssembly::new(request_id, structured, map_replay(matched_replay));
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
                    let _ = sender.send(Err(error)).await;
                    return;
                }
            };
            if let Err(error) = consume_lines(&mut assembly, leftover, &sender).await {
                let _ = sender.send(Err(error)).await;
                return;
            }
            if !assembly.done {
                let _ = sender
                    .send(Err(stream_error(
                        "Ollama NDJSON stream ended before done:true",
                    )))
                    .await;
                return;
            }
            let _ = sender
                .send(
                    assembly
                        .finish()
                        .map(ModelStreamItem::Completed)
                        .map_err(map_norm),
                )
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
                let _ = sender.send(Err(error)).await;
                return;
            }
        };
        match consume_lines(&mut assembly, lines, &sender).await {
            Ok(true) => {
                let _ = sender
                    .send(
                        assembly
                            .finish()
                            .map(ModelStreamItem::Completed)
                            .map_err(map_norm),
                    )
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

async fn consume_lines(
    assembly: &mut OllamaChatAssembly,
    lines: Vec<String>,
    sender: &mpsc::Sender<Result<ModelStreamItem, ModelError>>,
) -> Result<bool, ModelError> {
    for line in lines {
        let items = assembly.consume(&line).map_err(map_norm)?;
        for item in items {
            if sender.send(Ok(item)).await.is_err() {
                return Ok(assembly.done);
            }
        }
        if assembly.done {
            return Ok(true);
        }
    }
    Ok(false)
}

fn map_replay(replay: Option<Vec<ReplayEntry>>) -> Option<Vec<OllamaReplayEntry>> {
    replay.map(|entries| {
        entries
            .into_iter()
            .map(|entry| OllamaReplayEntry {
                thinking: entry.thinking,
                digest: entry.digest,
            })
            .collect()
    })
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
        "Ollama request was cancelled",
    )
}

fn transport_error(source: &reqwest::Error) -> ModelError {
    if source.is_timeout() {
        error(
            TIMEOUT,
            ErrorCategory::Deadline,
            true,
            "Ollama request timed out",
        )
    } else {
        error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "Ollama transport failed",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_thinking_tools_and_usage() {
        let mut assembly = OllamaChatAssembly::new("request-1".to_owned(), false, None);
        let items = assembly
            .consume(r#"{"message":{"content":"hel","thinking":"con"},"done":false}"#)
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::TextDelta(_)));
        assert!(matches!(items[1], ModelStreamItem::ReasoningDelta(_)));
        assembly
            .consume(r#"{"message":{"content":"lo","thinking":"sider"},"done":false}"#)
            .unwrap();
        let items = assembly
            .consume(
                r#"{"message":{"content":"","tool_calls":[{"function":{"name":"lookup","arguments":{"x":1}}}]},"done":false}"#,
            )
            .unwrap();
        match &items[0] {
            ModelStreamItem::ToolCallDelta(delta) => {
                assert_eq!(delta.name.as_deref(), Some("lookup"));
                assert_eq!(delta.provider_call_id, None);
            }
            other => panic!("expected tool-call delta, got {other:?}"),
        }
        let items = assembly
            .consume(
                r#"{"message":{"content":""},"done":true,"prompt_eval_count":4,"eval_count":3}"#,
            )
            .unwrap();
        assert!(matches!(items[0], ModelStreamItem::Usage(_)));
        let response = assembly.finish().unwrap();
        assert_eq!(response.completion_id.as_ref(), "request-1");
        assert_eq!(response.provider_ids.request_id(), Some("request-1"));
        assert_eq!(response.provider_ids.response_id(), None);
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Text(_)
        ));
        assert_eq!(response.tool_calls[0].name.as_ref(), "lookup");
        assert_eq!(response.tool_calls[0].arguments.as_str(), r#"{"x":1}"#);
        assert_eq!(response.tool_calls[0].provider_call_id, None);
        assert_eq!(response.usage.input_tokens(), Some(4));
        assert_eq!(response.usage.output_tokens(), Some(3));
        assert_eq!(response.usage.total_tokens(), Some(7));
        let envelope: serde_json::Value = serde_json::from_slice(
            response
                .continuation_state
                .expect("continuation")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(envelope["provider"], "ollama.api_chat");
        assert_eq!(envelope["version"], 1);
        assert_eq!(envelope["assistant_replay"][0]["thinking"], "consider");
    }

    #[test]
    fn stream_without_done_is_invalid() {
        let mut assembly = OllamaChatAssembly::new("request-1".to_owned(), false, None);
        assembly
            .consume(r#"{"message":{"content":"hello"},"done":false}"#)
            .unwrap();
        assert!(!assembly.done);
        assert_eq!(
            assembly.finish().expect_err("missing done").kind,
            StreamNormKind::Stream
        );
    }

    #[test]
    fn unknown_model_uses_a_zeroed_profile_and_prompted_structured_output() {
        let config = OllamaConfig::try_new("http://127.0.0.1:9").expect("config");
        let model = OllamaModelConfig::try_new("gemma3", 1_000_000, 128_000, 4_096, 4_096, 256)
            .expect("model");
        let provider = OllamaProvider::try_new(config, vec![model]).expect("provider");
        let unknown = ModelName::try_new("gemma-unknown").expect("name");
        let capabilities = provider.capabilities(&unknown);
        assert_eq!(capabilities.context_profile.model, unknown);
        assert_eq!(capabilities.context_profile.context_window_tokens, 0);
        assert_eq!(
            capabilities.structured_output,
            finstack_ai_runtime::StructuredOutputCapability::Prompted
        );
    }

    #[derive(Debug)]
    struct OversizedResolver;

    impl MediaResolver for OversizedResolver {
        fn resolve(
            &self,
            _blob: &finstack_ai_kernel::BlobRef,
        ) -> finstack_ai_runtime::PortFuture<
            Result<ResolvedMedia, finstack_ai_runtime::MediaResolveError>,
        > {
            Box::pin(async {
                Ok(ResolvedMedia::Bytes {
                    media_type: Arc::from("image/png"),
                    bytes: Arc::from(vec![0_u8; 9 * 1_048_576]),
                })
            })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_resolved_media_fails_closed() {
        use finstack_ai_kernel::{
            EffectId, LaneId, MediaRef, MessageId, OperationLocator, OutputSpec, PrincipalRef,
            ProviderIds, RawJson, RunId, SessionId, Timestamp,
        };
        use finstack_ai_runtime::{
            AuthorizationContext, CancellationSignal, ModelCallContext, ModelRequest,
            ModelRequestDraft, ModelRequestLimits, ModelSettings, RunCallContext,
        };

        let config = OllamaConfig::try_new("http://127.0.0.1:9")
            .expect("config")
            .with_media_resolver(Arc::new(OversizedResolver));
        let model =
            OllamaModelConfig::try_new("fixture-model", 1_000_000, 128_000, 4_096, 4_096, 256)
                .expect("model");
        let provider = OllamaProvider::try_new(config, vec![model]).expect("provider");
        let selected = ModelName::try_new("fixture-model").expect("name");

        let blob =
            finstack_ai_kernel::BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>)
                .expect("blob");
        let message = finstack_ai_kernel::Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789a6").expect("message id"),
            finstack_ai_kernel::MessageRole::User,
            vec![ContentBlock::Image(MediaRef::new(blob))],
            Timestamp::from_unix_ms(1).expect("ts"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");

        let request = ModelRequest {
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
                request_id: finstack_ai_kernel::ModelRequestId::parse(
                    "01234567-89ab-7cde-89ab-0123456789a5",
                )
                .expect("request id"),
            },
            draft: ModelRequestDraft {
                model: selected,
                messages: Arc::from([message]),
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
        };

        let Err(error) = provider.request(request).await else {
            panic!("oversized media must fail closed before any HTTP call");
        };
        assert_eq!(error.code(), crate::error::STREAM_LIMIT_EXCEEDED);
    }
}
