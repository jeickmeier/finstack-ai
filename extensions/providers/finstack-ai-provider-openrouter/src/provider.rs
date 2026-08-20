//! Native `OpenRouter` Responses provider implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};

use finstack_ai_kernel::{ContentBlock, ErrorCategory, Metadata, OutputSpec, PendingModelEffect};
use finstack_ai_runtime::{
    MediaResolver, Model, ModelCapabilities, ModelDescriptor, ModelError, ModelEventStream,
    ModelName, ModelReconcileResult, ModelRequest, ModelRequestDraft, ModelStreamItem,
    ModelTokenEstimate, ReconcileContext, ResolvedMedia,
};
use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
use tokio::sync::mpsc;

use crate::config::estimator_ref;
use crate::error::{CANCELLED, HTTP_ERROR, RESPONSE_INVALID, TIMEOUT, TRANSPORT_ERROR, error};
use crate::request::{ResponsesRequest, serialize_request};
use crate::sse::SseParser;
use crate::stream::CompletionAssembly;
use crate::{OpenRouterConfig, OpenRouterModelConfig};

const STREAM_CHANNEL_CAPACITY: usize = 32;

/// Reusable native official `OpenRouter` Responses provider.
pub struct OpenRouterProvider {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    config: OpenRouterConfig,
    models: RwLock<BTreeMap<ModelName, OpenRouterModelConfig>>,
}

impl fmt::Debug for OpenRouterProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenRouterProvider")
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

impl OpenRouterProvider {
    /// Construct one provider and its reusable pooled HTTP client.
    ///
    /// # Errors
    ///
    /// Rejects an empty/duplicate model catalog or invalid transport configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_openrouter::{OpenRouterConfig, OpenRouterModelConfig, OpenRouterProvider};
    /// use finstack_ai_runtime::Model;
    ///
    /// let config = OpenRouterConfig::try_new("http://127.0.0.1:9").expect("config");
    /// let model = OpenRouterModelConfig::try_new(
    ///     "openai/gpt-test",
    ///     1_000_000,
    ///     128_000,
    ///     4_096,
    ///     4_096,
    ///     256,
    /// )
    /// .expect("model");
    /// let provider = OpenRouterProvider::try_new(config, vec![model]).expect("provider");
    /// assert_eq!(provider.descriptor().provider.as_ref(), "openrouter");
    /// ```
    pub fn try_new(
        config: OpenRouterConfig,
        models: Vec<OpenRouterModelConfig>,
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
        models: Vec<OpenRouterModelConfig>,
    ) -> Result<(), ModelError> {
        let by_name = catalog_from_models(models)?;
        *self.models.write().unwrap_or_else(PoisonError::into_inner) = by_name;
        Ok(())
    }

    /// Refresh advertised capabilities for one configured model from a local table.
    ///
    /// # Errors
    ///
    /// Returns `openrouter_request_invalid` when the model is not configured.
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

    /// Fetch `GET /api/v1/models` and map it onto conservative model configs.
    ///
    /// The result is returned to the caller; applying it stays explicit via
    /// [`Self::replace_model_catalog`]. The response body is bounded by the
    /// configured `max_stream_bytes`.
    ///
    /// # Errors
    ///
    /// Returns transport, HTTP, limit, or `openrouter_response_invalid`
    /// errors; never surfaces the response body.
    pub async fn fetch_model_catalog(
        &self,
        hard_input_bytes: u64,
    ) -> Result<Vec<OpenRouterModelConfig>, ModelError> {
        let response = self
            .client
            .get(self.config.models_url()?)
            .timeout(self.config.request_timeout())
            .send()
            .await
            .map_err(|source| transport_error(&source))?;
        if !response.status().is_success() {
            let retryable = matches!(response.status().as_u16(), 408 | 409 | 429 | 500..=599);
            return Err(error(
                HTTP_ERROR,
                ErrorCategory::Model,
                retryable,
                "OpenRouter models endpoint returned an unsuccessful status",
            ));
        }
        let cap = self.config.max_stream_bytes();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|source| transport_error(&source))?;
            if body.len().saturating_add(chunk.len()) > cap {
                return Err(crate::error::stream_limit_error());
            }
            body.extend_from_slice(&chunk);
        }
        crate::catalog::model_configs_from_catalog_json(&body, hard_input_bytes)
    }

    fn model_config(&self, name: &ModelName) -> Result<OpenRouterModelConfig, ModelError> {
        self.models
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| crate::error::request_error("requested model is not configured"))
    }
}

fn catalog_from_models(
    models: Vec<OpenRouterModelConfig>,
) -> Result<BTreeMap<ModelName, OpenRouterModelConfig>, ModelError> {
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
        provider: Arc::from("openrouter"),
        models: by_name.keys().cloned().collect::<Vec<_>>().into(),
        metadata: Metadata::empty(),
    };
    descriptor.validate()?;
    Ok(by_name)
}

impl Model for OpenRouterProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        ModelDescriptor {
            provider: Arc::from("openrouter"),
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
                    provider: Arc::from("openrouter"),
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
                structured_output: finstack_ai_runtime::StructuredOutputCapability::Native,
                reasoning: false,
                prompt_cache: false,
                resumable_stream: false,
                idempotent_requests: false,
                native_capabilities: BTreeSet::new(),
            },
            OpenRouterModelConfig::capabilities,
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
                    "OpenRouter request length overflowed the estimator",
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
                resolve_draft_media(media_resolver.as_ref(), &request.draft, max_stream_bytes)
                    .await?;
            let wire = ResponsesRequest::try_from_draft(
                &request.draft,
                &model,
                request.continuation_state.as_ref(),
                &resolved_media,
            )?;
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
                    "OpenRouter endpoint returned an unsuccessful status",
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
            let error = match parser.finish() {
                Ok(()) => crate::error::stream_error(
                    "OpenRouter SSE stream ended before response.completed",
                ),
                Err(error) => error,
            };
            let _ = sender.send(Err(error)).await;
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
            match assembly.consume(&event.data) {
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
                    let _ = sender.send(Err(error)).await;
                    return;
                }
            }
        }
    }
}

/// Resolve every distinct media block in `draft`, bounding both each
/// resolved payload and the running aggregate across the draft by
/// `max_stream_bytes` (ADR-049) so a caller cannot smuggle an oversized
/// request past per-blob checks by splitting it across many blobs.
async fn resolve_draft_media(
    media_resolver: Option<&Arc<dyn MediaResolver>>,
    draft: &ModelRequestDraft,
    max_stream_bytes: usize,
) -> Result<BTreeMap<Arc<str>, ResolvedMedia>, ModelError> {
    let mut resolved_media = BTreeMap::new();
    let mut aggregate_bytes: usize = 0;
    for message in draft.messages.iter() {
        for block in message.content() {
            let (ContentBlock::Image(media)
            | ContentBlock::Audio(media)
            | ContentBlock::File(media)) = block
            else {
                continue;
            };
            let id: Arc<str> = Arc::from(media.blob().id());
            if resolved_media.contains_key(&id) {
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
            if let ResolvedMedia::Bytes { bytes, .. } = &payload {
                if bytes.len() > max_stream_bytes {
                    return Err(crate::error::stream_limit_error());
                }
                aggregate_bytes = aggregate_bytes.saturating_add(bytes.len());
                if aggregate_bytes > max_stream_bytes {
                    return Err(crate::error::stream_limit_error());
                }
            }
            resolved_media.insert(id, payload);
        }
    }
    Ok(resolved_media)
}

fn map_resolve(error: finstack_ai_runtime::MediaResolveError) -> ModelError {
    use finstack_ai_runtime::MediaResolveKind;
    match error.kind {
        MediaResolveKind::NotFound => crate::error::request_error(error.message),
        MediaResolveKind::Unavailable => crate::error::error(
            crate::error::TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "OpenRouter media resolution is unavailable",
        ),
        MediaResolveKind::Limit => crate::error::stream_limit_error(),
    }
}

fn cancelled_error() -> ModelError {
    error(
        CANCELLED,
        ErrorCategory::Cancellation,
        false,
        "OpenRouter request was cancelled",
    )
}

fn transport_error(source: &reqwest::Error) -> ModelError {
    if source.is_timeout() {
        error(
            TIMEOUT,
            ErrorCategory::Deadline,
            true,
            "OpenRouter request timed out",
        )
    } else {
        error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "OpenRouter transport failed",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{
        BlobRef, EffectId, LaneId, MediaRef, Message, MessageId, ModelRequestId, OperationLocator,
        PrincipalRef, ProviderIds, RunId, SessionId, Timestamp,
    };
    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, MediaResolveError, ModelCallContext,
        ModelRequestDraft, ModelRequestLimits, ModelSettings, PortFuture, RunCallContext,
        StructuredOutputCapability,
    };

    #[derive(Debug)]
    struct OversizedResolver;

    impl MediaResolver for OversizedResolver {
        fn resolve(&self, _blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>> {
            Box::pin(async {
                Ok(ResolvedMedia::Bytes {
                    media_type: Arc::from("image/png"),
                    bytes: Arc::from(vec![0_u8; 9 * 1_048_576]),
                })
            })
        }
    }

    fn media_request(model: ModelName) -> ModelRequest {
        let blob = BlobRef::try_new("blob-1", "image/png", 9 * 1_048_576, None, None::<&str>)
            .expect("blob");
        let media = MediaRef::new(blob);
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
                request_id: ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789a5")
                    .expect("request id"),
            },
            draft: ModelRequestDraft {
                model,
                messages: Arc::from([Message::try_new(
                    MessageId::parse("01234567-89ab-7cde-89ab-0123456789a6").expect("message"),
                    finstack_ai_kernel::MessageRole::User,
                    vec![ContentBlock::Image(media)],
                    Timestamp::from_unix_ms(1).expect("ts"),
                    None,
                    ProviderIds::empty(),
                    Metadata::empty(),
                )
                .expect("message")]),
                tools: Arc::from([]),
                output: OutputSpec::PlainText,
                settings: ModelSettings {
                    values: finstack_ai_kernel::RawJson::parse(b"{}").expect("settings"),
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

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_resolved_media_fails_closed() {
        let config = OpenRouterConfig::try_new("http://127.0.0.1:9")
            .expect("config")
            .with_stream_limits(1_048_576, 4 * 1_048_576)
            .expect("stream limits")
            .with_media_resolver(Arc::new(OversizedResolver));
        let model = OpenRouterModelConfig::try_new(
            "openai/gpt-test",
            1_000_000,
            128_000,
            4_096,
            4_096,
            256,
        )
        .expect("model");
        let name = model.name.clone();
        let provider = OpenRouterProvider::try_new(config, vec![model]).expect("provider");
        let result = provider.request(media_request(name)).await;
        let Err(error) = result else {
            panic!("oversized media must fail closed")
        };
        assert_eq!(error.code(), crate::error::STREAM_LIMIT_EXCEEDED);
    }

    #[derive(Debug)]
    struct SizedResolver {
        first_bytes: usize,
        second_bytes: usize,
    }

    impl MediaResolver for SizedResolver {
        fn resolve(&self, blob: &BlobRef) -> PortFuture<Result<ResolvedMedia, MediaResolveError>> {
            let size = if blob.id() == "blob-1" {
                self.first_bytes
            } else {
                self.second_bytes
            };
            Box::pin(async move {
                Ok(ResolvedMedia::Bytes {
                    media_type: Arc::from("image/png"),
                    bytes: Arc::from(vec![0_u8; size]),
                })
            })
        }
    }

    fn two_blob_media_request(model: ModelName) -> ModelRequest {
        let blob_a = BlobRef::try_new("blob-1", "image/png", 3 * 1_048_576, None, None::<&str>)
            .expect("blob");
        let blob_b = BlobRef::try_new("blob-2", "image/png", 3 * 1_048_576, None, None::<&str>)
            .expect("blob");
        let mut request = media_request(model);
        request.draft.messages = Arc::from([Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789a6").expect("message"),
            finstack_ai_kernel::MessageRole::User,
            vec![
                ContentBlock::Image(MediaRef::new(blob_a)),
                ContentBlock::Image(MediaRef::new(blob_b)),
            ],
            Timestamp::from_unix_ms(1).expect("ts"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")]);
        request
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn aggregate_resolved_media_over_the_stream_cap_fails_closed_even_when_each_blob_is_under()
     {
        let config = OpenRouterConfig::try_new("http://127.0.0.1:9")
            .expect("config")
            .with_stream_limits(1_048_576, 4 * 1_048_576)
            .expect("stream limits")
            .with_media_resolver(Arc::new(SizedResolver {
                first_bytes: 3 * 1_048_576,
                second_bytes: 3 * 1_048_576,
            }));
        let model = OpenRouterModelConfig::try_new(
            "openai/gpt-test",
            1_000_000,
            128_000,
            4_096,
            4_096,
            256,
        )
        .expect("model");
        let name = model.name.clone();
        let provider = OpenRouterProvider::try_new(config, vec![model]).expect("provider");
        let result = provider.request(two_blob_media_request(name)).await;
        let Err(error) = result else {
            panic!("aggregate-oversized media must fail closed")
        };
        assert_eq!(error.code(), crate::error::STREAM_LIMIT_EXCEEDED);
    }

    #[test]
    fn unknown_model_uses_a_zeroed_profile_and_native_structured_output() {
        let config = OpenRouterConfig::try_new("http://127.0.0.1:9").expect("config");
        let model = OpenRouterModelConfig::try_new(
            "openai/gpt-test",
            1_000_000,
            128_000,
            4_096,
            4_096,
            256,
        )
        .expect("model");
        let provider = OpenRouterProvider::try_new(config, vec![model]).expect("provider");
        let unknown = ModelName::try_new("openai/gpt-unknown").expect("name");
        let capabilities = provider.capabilities(&unknown);
        assert_eq!(capabilities.context_profile.model, unknown);
        assert_eq!(capabilities.context_profile.context_window_tokens, 0);
        assert_eq!(
            capabilities.structured_output,
            StructuredOutputCapability::Native
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fetch_model_catalog_round_trips_through_replace() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let body = br#"{"data":[{"id":"openai/gpt-5","context_length":400000,"top_provider":{"max_completion_tokens":128000},"supported_parameters":["tools"]}]}"#;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0_u8; 4_096];
            let count = socket.read(&mut buffer).await.expect("read");
            let request = String::from_utf8_lossy(&buffer[..count]).to_string();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(headers.as_bytes()).await.expect("headers");
            socket.write_all(body).await.expect("body");
            request
        });
        let config = OpenRouterConfig::try_new(format!("http://{address}")).expect("config");
        let seed = OpenRouterModelConfig::try_new("seed", 1, 128, 16, 16, 8).expect("seed");
        let provider = OpenRouterProvider::try_new(config, vec![seed]).expect("provider");
        let catalog = provider
            .fetch_model_catalog(1_000_000)
            .await
            .expect("catalog");
        provider.replace_model_catalog(catalog).expect("replace");
        let request = server.await.expect("server");
        assert!(request.starts_with("GET /api/v1/models"));
        let names = provider.descriptor().models;
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].as_str(), "openai/gpt-5");
    }
}
