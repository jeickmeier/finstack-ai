//! Native official `OpenAI` Responses provider implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};

use finstack_ai_kernel::{ErrorCategory, Metadata, OutputSpec, PendingModelEffect};
use finstack_ai_runtime::{
    Model, ModelCapabilities, ModelDescriptor, ModelError, ModelEventStream, ModelName,
    ModelReconcileResult, ModelRequest, ModelStreamItem, ModelTokenEstimate, ReconcileContext,
    ResolveDraftMediaError, resolve_draft_media,
};
use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
use tokio::sync::mpsc;

use crate::config::estimator_ref;
use crate::error::{CANCELLED, HTTP_ERROR, RESPONSE_INVALID, TIMEOUT, TRANSPORT_ERROR, error};
use crate::request::{ResponsesRequest, serialize_request};
use crate::sse::SseParser;
use crate::stream::CompletionAssembly;
use crate::{OpenAiConfig, OpenAiModelConfig};

const STREAM_CHANNEL_CAPACITY: usize = 32;

/// Reusable native official `OpenAI` Responses provider.
pub struct OpenAiProvider {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    config: OpenAiConfig,
    models: RwLock<BTreeMap<ModelName, OpenAiModelConfig>>,
}

impl fmt::Debug for OpenAiProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiProvider")
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

impl OpenAiProvider {
    /// Construct one provider and its reusable pooled HTTP client.
    ///
    /// # Errors
    ///
    /// Rejects an empty/duplicate model catalog or invalid transport configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_openai::{OpenAiConfig, OpenAiModelConfig, OpenAiProvider};
    /// use finstack_ai_runtime::Model;
    ///
    /// let config = OpenAiConfig::try_new("http://127.0.0.1:9").expect("config");
    /// let model = OpenAiModelConfig::try_new(
    ///     "gpt-test",
    ///     1_000_000,
    ///     128_000,
    ///     4_096,
    ///     4_096,
    ///     256,
    /// )
    /// .expect("model");
    /// let provider = OpenAiProvider::try_new(config, vec![model]).expect("provider");
    /// assert_eq!(provider.descriptor().provider.as_ref(), "openai");
    /// ```
    pub fn try_new(
        config: OpenAiConfig,
        models: Vec<OpenAiModelConfig>,
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
    pub fn replace_model_catalog(&self, models: Vec<OpenAiModelConfig>) -> Result<(), ModelError> {
        let by_name = catalog_from_models(models)?;
        *self.models.write().unwrap_or_else(PoisonError::into_inner) = by_name;
        Ok(())
    }

    /// Refresh advertised capabilities for one configured model from a local table.
    ///
    /// # Errors
    ///
    /// Returns `openai_request_invalid` when the model is not configured.
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

    fn model_config(&self, name: &ModelName) -> Result<OpenAiModelConfig, ModelError> {
        self.models
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| crate::error::request_error("requested model is not configured"))
    }
}

fn catalog_from_models(
    models: Vec<OpenAiModelConfig>,
) -> Result<BTreeMap<ModelName, OpenAiModelConfig>, ModelError> {
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
        provider: Arc::from("openai"),
        models: by_name.keys().cloned().collect::<Vec<_>>().into(),
        metadata: Metadata::empty(),
    };
    descriptor.validate()?;
    Ok(by_name)
}

impl Model for OpenAiProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        ModelDescriptor {
            provider: Arc::from("openai"),
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
                    provider: Arc::from("openai"),
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
            OpenAiModelConfig::capabilities,
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
                    "OpenAI request length overflowed the estimator",
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
        let resolver = self.config.media_resolver();
        Box::pin(async move {
            let model = model?;
            let resolved_media =
                resolve_draft_media(resolver.as_ref(), &request.draft, max_stream_bytes)
                    .await
                    .map_err(map_draft_media)?;
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
                    "OpenAI endpoint returned an unsuccessful status",
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
                Ok(()) => {
                    crate::error::stream_error("OpenAI SSE stream ended before response.completed")
                }
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

fn map_draft_media(error: ResolveDraftMediaError) -> ModelError {
    match error {
        ResolveDraftMediaError::MissingResolver => {
            crate::error::request_error("media content requires a configured media resolver")
        }
        ResolveDraftMediaError::Resolve(inner) => map_resolve(inner),
        ResolveDraftMediaError::Limit => crate::error::stream_limit_error(),
    }
}

fn map_resolve(error: finstack_ai_runtime::MediaResolveError) -> ModelError {
    use finstack_ai_runtime::MediaResolveKind;
    match error.kind {
        MediaResolveKind::NotFound => crate::error::request_error(error.message),
        MediaResolveKind::Unavailable => crate::error::error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "OpenAI media resolution is unavailable",
        ),
        MediaResolveKind::Limit => crate::error::stream_limit_error(),
    }
}

fn cancelled_error() -> ModelError {
    error(
        CANCELLED,
        ErrorCategory::Cancellation,
        false,
        "OpenAI request was cancelled",
    )
}

fn transport_error(source: &reqwest::Error) -> ModelError {
    if source.is_timeout() {
        error(
            TIMEOUT,
            ErrorCategory::Deadline,
            true,
            "OpenAI request timed out",
        )
    } else {
        error(
            TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "OpenAI transport failed",
        )
    }
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{
        BlobRef, ContentBlock, EffectId, LaneId, MediaRef, Message, MessageId, MessageRole,
        ModelRequestId, OperationLocator, OutputSpec, PrincipalRef, ProviderIds, RawJson, RunId,
        SessionId, Timestamp,
    };
    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, MediaResolveError, MediaResolver,
        ModelCallContext, ModelRequest, ModelRequestDraft, ModelRequestLimits, ModelSettings,
        PortFuture, ResolvedMedia, RunCallContext, StructuredOutputCapability,
    };

    use super::*;

    #[test]
    fn unknown_model_uses_a_zeroed_profile_and_native_structured_output() {
        let config = OpenAiConfig::try_new("http://127.0.0.1:9").expect("config");
        let model = OpenAiModelConfig::try_new("gpt-test", 1_000_000, 128_000, 4_096, 4_096, 256)
            .expect("model");
        let provider = OpenAiProvider::try_new(config, vec![model]).expect("provider");
        let unknown = ModelName::try_new("gpt-unknown").expect("name");
        let capabilities = provider.capabilities(&unknown);
        assert_eq!(capabilities.context_profile.model, unknown);
        assert_eq!(capabilities.context_profile.context_window_tokens, 0);
        assert_eq!(
            capabilities.structured_output,
            StructuredOutputCapability::Native
        );
    }

    #[test]
    fn refresh_model_metadata_round_trips_the_input_capability_flags() {
        let config = OpenAiConfig::try_new("http://127.0.0.1:9").expect("config");
        let model = OpenAiModelConfig::try_new("gpt-test", 1_000_000, 128_000, 4_096, 4_096, 256)
            .expect("model");
        let provider = OpenAiProvider::try_new(config, vec![model]).expect("provider");
        let name = ModelName::try_new("gpt-test").expect("name");
        assert!(!provider.capabilities(&name).input.images);

        let mut update = provider.capabilities(&name);
        update.input.images = true;
        provider
            .refresh_model_metadata(&name, update)
            .expect("refresh");

        assert!(provider.capabilities(&name).input.images);
    }

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

    fn fixture_request(media: ContentBlock) -> ModelRequest {
        fixture_request_with(vec![media])
    }

    fn fixture_request_with(media: Vec<ContentBlock>) -> ModelRequest {
        let selected = ModelName::try_new("gpt-test").expect("name");
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
                model: selected,
                messages: Arc::from([Message::try_new(
                    MessageId::parse("01234567-89ab-7cde-89ab-0123456789a6").expect("message"),
                    MessageRole::User,
                    media,
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

    #[tokio::test]
    async fn oversized_resolved_media_fails_closed() {
        let config = OpenAiConfig::try_new("http://127.0.0.1:9")
            .expect("config")
            .with_stream_limits(1_048_576, 4 * 1_048_576)
            .expect("stream limits")
            .with_media_resolver(Arc::new(OversizedResolver));
        let model = OpenAiModelConfig::try_new("gpt-test", 1_000_000, 128_000, 4_096, 4_096, 256)
            .expect("model");
        let provider = OpenAiProvider::try_new(config, vec![model]).expect("provider");

        let blob = BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>).expect("blob");
        let request = fixture_request(ContentBlock::Image(MediaRef::new(blob)));

        let Err(error) = provider.request(request).await else {
            panic!("oversized media must fail closed");
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

    #[tokio::test]
    async fn aggregate_resolved_media_over_the_stream_cap_fails_closed_even_when_each_blob_is_under()
     {
        let config = OpenAiConfig::try_new("http://127.0.0.1:9")
            .expect("config")
            .with_stream_limits(1_048_576, 4 * 1_048_576)
            .expect("stream limits")
            .with_media_resolver(Arc::new(SizedResolver {
                first_bytes: 3 * 1_048_576,
                second_bytes: 3 * 1_048_576,
            }));
        let model = OpenAiModelConfig::try_new("gpt-test", 1_000_000, 128_000, 4_096, 4_096, 256)
            .expect("model");
        let provider = OpenAiProvider::try_new(config, vec![model]).expect("provider");

        let blob_a = BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>).expect("blob");
        let blob_b = BlobRef::try_new("blob-2", "image/png", 4, None, None::<&str>).expect("blob");
        let request = fixture_request_with(vec![
            ContentBlock::Image(MediaRef::new(blob_a)),
            ContentBlock::Image(MediaRef::new(blob_b)),
        ]);

        let Err(error) = provider.request(request).await else {
            panic!("aggregate-oversized media must fail closed");
        };
        assert_eq!(error.code(), crate::error::STREAM_LIMIT_EXCEEDED);
    }
}
