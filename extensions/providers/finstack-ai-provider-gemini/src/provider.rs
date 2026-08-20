//! Native Gemini `generateContent` provider implementation.

use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};

use finstack_ai_kernel::{ErrorCategory, Metadata, OutputSpec, PendingModelEffect};
use finstack_ai_runtime::{
    CancellationSignal, GeminiGenerateContentAssembly, InputCapabilities, MediaResolveError,
    MediaResolveKind, Model, ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError,
    ModelEventStream, ModelName, ModelReconcileResult, ModelRequest, ModelStreamItem,
    ModelTokenEstimate, PortFuture, ReconcileContext, ResolveDraftMediaError, StreamNormError,
    StreamNormKind, StructuredOutputCapability, resolve_draft_media,
};
use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
use tokio::sync::mpsc;

use crate::config::config_error;
use crate::error::{
    GEMINI_HTTP_ERROR, GEMINI_RESPONSE_INVALID, GEMINI_TIMEOUT, GEMINI_TRANSPORT_ERROR, error,
};
use crate::request::{GenerateContentRequest, request_error};
use crate::sse::{GeminiSse, stream_error, stream_limit_error};
use crate::{GeminiConfig, GeminiModelConfig};

const PROVIDER: &str = "gemini";
const STREAM_CHANNEL_CAPACITY: usize = 32;

/// Reusable native Gemini `generateContent` provider.
pub struct GeminiProvider {
    client: reqwest::Client,
    config: GeminiConfig,
    models: RwLock<BTreeMap<ModelName, GeminiModelConfig>>,
}

impl fmt::Debug for GeminiProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeminiProvider")
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

impl GeminiProvider {
    /// Construct one provider and its reusable pooled HTTP client.
    ///
    /// # Errors
    ///
    /// Rejects an empty/duplicate model catalog or invalid transport configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_provider_gemini::{GeminiConfig, GeminiModelConfig, GeminiProvider};
    /// use finstack_ai_runtime::Model;
    ///
    /// let config = GeminiConfig::try_new("http://127.0.0.1:9").expect("config");
    /// let model =
    ///     GeminiModelConfig::try_new("gemini-test", 1_000_000, 128_000, 4_096).expect("model");
    /// let provider = GeminiProvider::try_new(config, vec![model]).expect("provider");
    /// assert_eq!(provider.descriptor().provider.as_ref(), "gemini");
    /// ```
    pub fn try_new(
        config: GeminiConfig,
        models: Vec<GeminiModelConfig>,
    ) -> Result<Self, ModelError> {
        let headers = config.header_map()?;
        let by_name = catalog_from_models(models)?;
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(Policy::none())
            .build()
            .map_err(|_| config_error("provider HTTP client could not be built"))?;
        Ok(Self {
            client,
            config,
            models: RwLock::new(by_name),
        })
    }

    /// Replace the in-memory model catalog from a local table.
    ///
    /// # Errors
    ///
    /// Rejects an empty or duplicate catalog.
    pub fn replace_model_catalog(&self, models: Vec<GeminiModelConfig>) -> Result<(), ModelError> {
        let by_name = catalog_from_models(models)?;
        *self.models.write().unwrap_or_else(PoisonError::into_inner) = by_name;
        Ok(())
    }

    /// Refresh advertised capabilities for one configured model from a local table.
    ///
    /// # Errors
    ///
    /// Returns `gemini_request_invalid` when the model is not configured, and
    /// `gemini_config_invalid` when the replacement context profile is invalid.
    pub fn refresh_model_metadata(
        &self,
        model: &ModelName,
        update: &ModelCapabilities,
    ) -> Result<(), ModelError> {
        let mut models = self.models.write().unwrap_or_else(PoisonError::into_inner);
        let configured = models
            .get_mut(model)
            .ok_or_else(|| request_error("requested model is not configured"))?;
        configured.apply_capabilities(update)
    }

    fn model_config(&self, name: &ModelName) -> Result<GeminiModelConfig, ModelError> {
        self.models
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| request_error("requested model is not configured"))
    }
}

fn catalog_from_models(
    models: Vec<GeminiModelConfig>,
) -> Result<BTreeMap<ModelName, GeminiModelConfig>, ModelError> {
    let mut by_name = BTreeMap::new();
    for model in models {
        if by_name.insert(model.name().clone(), model).is_some() {
            return Err(config_error("provider contains a duplicate model name"));
        }
    }
    if by_name.is_empty() {
        return Err(config_error("provider requires at least one model"));
    }
    let descriptor = ModelDescriptor {
        provider: Arc::from(PROVIDER),
        models: by_name.keys().cloned().collect::<Vec<_>>().into(),
        metadata: Metadata::empty(),
    };
    descriptor.validate()?;
    Ok(by_name)
}

fn map_draft_media(error: ResolveDraftMediaError) -> ModelError {
    match error {
        ResolveDraftMediaError::MissingResolver => {
            request_error("media content requires a configured media resolver")
        }
        ResolveDraftMediaError::Resolve(inner) => map_resolve(inner),
        ResolveDraftMediaError::Limit => stream_limit_error(),
    }
}

fn map_resolve(error: MediaResolveError) -> ModelError {
    match error.kind {
        MediaResolveKind::NotFound => request_error(error.message),
        MediaResolveKind::Unavailable => crate::error::error(
            GEMINI_TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "Gemini media resolution is unavailable",
        ),
        MediaResolveKind::Limit => stream_limit_error(),
    }
}

impl Model for GeminiProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        ModelDescriptor {
            provider: Arc::from(PROVIDER),
            models: models.keys().cloned().collect::<Vec<_>>().into(),
            metadata: Metadata::empty(),
        }
    }

    fn capabilities(&self, model: &ModelName) -> ModelCapabilities {
        let models = self.models.read().unwrap_or_else(PoisonError::into_inner);
        models.get(model).map_or_else(
            || ModelCapabilities {
                input: InputCapabilities {
                    text: false,
                    json: false,
                    images: false,
                    audio: false,
                    files: false,
                },
                context_profile: ModelContextProfile {
                    provider: Arc::from(PROVIDER),
                    model: model.clone(),
                    hard_input_bytes: 0,
                    context_window_tokens: 0,
                    max_output_tokens: 0,
                    reserved_output_tokens: 0,
                    provider_overhead_tokens: 0,
                    estimator: GeminiModelConfig::estimator_ref(),
                },
                native_tool_calls: false,
                parallel_tool_calls: false,
                structured_output: StructuredOutputCapability::Prompted,
                reasoning: false,
                prompt_cache: false,
                resumable_stream: false,
                idempotent_requests: false,
                native_capabilities: BTreeSet::new(),
            },
            |configured| configured.capabilities(PROVIDER),
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
                    GEMINI_RESPONSE_INVALID,
                    ErrorCategory::Limit,
                    false,
                    "Gemini request length overflowed the estimator",
                )
            })?,
            estimator: GeminiModelConfig::estimator_ref(),
        })
    }

    #[expect(
        clippy::similar_names,
        reason = "`resolver` (host port) and `resolved` (its output map) are the clearest names"
    )]
    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        let client = self.client.clone();
        let model = self.model_config(&request.draft.model);
        let endpoint = self.config.model_url(&request.draft.model);
        let timeout = self.config.request_timeout();
        let max_event_bytes = self.config.max_event_bytes();
        let max_stream_bytes = self.config.max_stream_bytes();
        let resolver = self.config.media_resolver();
        Box::pin(async move {
            let model = model?;
            let endpoint = endpoint?;
            let resolved = resolve_draft_media(resolver.as_ref(), &request.draft, max_stream_bytes)
                .await
                .map_err(map_draft_media)?;
            let wire = GenerateContentRequest::try_from_draft(
                &request.draft,
                &model,
                request.continuation_state.as_ref(),
                &resolved,
            )?;
            let payload = wire.serialize()?;
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
                    GEMINI_HTTP_ERROR,
                    ErrorCategory::Model,
                    retryable,
                    "Gemini endpoint returned an unsuccessful status",
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
    ) -> PortFuture<Result<ModelReconcileResult, ModelError>> {
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
    cancellation: CancellationSignal,
    request_id: String,
    structured: bool,
    max_event_bytes: usize,
    max_stream_bytes: usize,
) {
    let mut body = response.bytes_stream();
    let mut parser = GeminiSse::new(max_event_bytes, max_stream_bytes);
    let mut received: usize = 0;
    let mut assembly = GeminiGenerateContentAssembly::new(request_id, structured);
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
                Ok(()) => match assembly.finish() {
                    Ok(()) => stream_error("Gemini SSE stream ended before a terminal chunk"),
                    Err(error) => map_norm(error),
                },
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
        // Belt-and-braces: the parser bounds the cumulative *framed* bytes, while
        // this counter bounds the raw *pre-framing* bytes read off the socket.
        received = match received.checked_add(chunk.len()) {
            Some(total) if total <= max_stream_bytes => total,
            _ => {
                let _ = sender.send(Err(stream_limit_error())).await;
                return;
            }
        };
        let payloads = match parser.push(&chunk) {
            Ok(payloads) => payloads,
            Err(error) => {
                let _ = sender.send(Err(error)).await;
                return;
            }
        };
        for payload in payloads {
            match assembly.consume(&payload) {
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
        StreamNormKind::Limit => stream_limit_error(),
        StreamNormKind::Stream => stream_error(error.message),
        StreamNormKind::Response | StreamNormKind::Incomplete => response_error(error.message),
    }
}

fn response_error(message: &'static str) -> ModelError {
    error(
        GEMINI_RESPONSE_INVALID,
        ErrorCategory::Model,
        false,
        message,
    )
}

fn cancelled_error() -> ModelError {
    error(
        crate::error::GEMINI_CANCELLED,
        ErrorCategory::Cancellation,
        false,
        "Gemini request was cancelled",
    )
}

fn transport_error(source: &reqwest::Error) -> ModelError {
    if source.is_timeout() {
        error(
            GEMINI_TIMEOUT,
            ErrorCategory::Deadline,
            true,
            "Gemini request timed out",
        )
    } else {
        error(
            GEMINI_TRANSPORT_ERROR,
            ErrorCategory::Model,
            true,
            "Gemini transport failed",
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::{
        BlobRef, ContentBlock, EffectId, LaneId, LimitKey, MediaRef, Message, MessageId,
        MessageRole, Metadata, ModelRequestId, OperationLocator, OutputSpec, PrincipalRef,
        ProviderIds, RawJson, RunId, SessionId, Timestamp,
    };
    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, GEMINI_THOUGHTS_TOKENS_KEY,
        GeminiGenerateContentAssembly, Model, ModelCallContext, ModelName, ModelRequest,
        ModelRequestDraft, ModelRequestLimits, ModelSettings, ModelStreamItem, RunCallContext,
    };

    use crate::sse::GeminiSse;
    use crate::{GeminiConfig, GeminiModelConfig, GeminiProvider};

    const CHUNK_TEXT_ONE: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello "}]},"index":0}],"responseId":"resp-1","modelVersion":"gemini-2.5-pro"}"#;
    const CHUNK_TEXT_TWO: &str = r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"world"}]},"index":0}],"responseId":"resp-1"}"#;
    const CHUNK_STOP: &str = r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5,"thoughtsTokenCount":3,"totalTokenCount":18},"responseId":"resp-1"}"#;

    /// Drive recorded SSE bytes through the exact framing + assembly pipeline
    /// the provider's `drive_response` loop uses.
    fn drive_recorded(frames: &[&str], structured: bool) -> Vec<ModelStreamItem> {
        let mut parser = GeminiSse::new(64 * 1024, 1_048_576);
        let mut assembly = GeminiGenerateContentAssembly::new("request-1".to_owned(), structured);
        let mut items = Vec::new();
        for frame in frames {
            let bytes = format!("data: {frame}\n\n");
            for payload in parser.push(bytes.as_bytes()).expect("sse frame") {
                items.extend(assembly.consume(&payload).expect("assembly chunk"));
            }
        }
        parser.finish().expect("clean sse eof");
        assembly.finish().expect("terminal chunk");
        items
    }

    #[test]
    fn recorded_text_sequence_assembles_response() {
        let items = drive_recorded(&[CHUNK_TEXT_ONE, CHUNK_TEXT_TWO, CHUNK_STOP], false);
        assert!(matches!(items[0], ModelStreamItem::TextDelta(_)));
        assert!(matches!(items[1], ModelStreamItem::TextDelta(_)));
        let ModelStreamItem::Completed(response) = items.last().expect("items") else {
            panic!("expected a completed item");
        };
        let ContentBlock::Text(text) = &response.assistant_content[0] else {
            panic!("expected assistant text");
        };
        assert_eq!(text.text(), "Hello world");
        assert_eq!(response.completion_id.as_ref(), "resp-1");
        assert_eq!(response.usage.input_tokens(), Some(10));
        // candidatesTokenCount + thoughtsTokenCount, never the wire total.
        assert_eq!(response.usage.output_tokens(), Some(8));
        assert_eq!(response.usage.total_tokens(), Some(18));
        assert_eq!(
            *response
                .usage
                .extension_counters()
                .get(&LimitKey::parse(GEMINI_THOUGHTS_TOKENS_KEY).expect("key"))
                .expect("thoughts counter"),
            3
        );
    }

    #[test]
    fn zero_extension_counters_are_not_reported() {
        let stop = r#"{"candidates":[{"content":{"role":"model","parts":[]},"finishReason":"STOP","index":0}],"usageMetadata":{"promptTokenCount":608,"candidatesTokenCount":69,"thoughtsTokenCount":0,"cachedContentTokenCount":0,"totalTokenCount":677},"responseId":"resp-1"}"#;
        let items = drive_recorded(&[CHUNK_TEXT_ONE, stop], false);
        let ModelStreamItem::Completed(response) = items.last().expect("items") else {
            panic!("expected a completed item");
        };
        assert!(response.usage.extension_counters().is_empty());
        assert_eq!(response.usage.input_tokens(), Some(608));
        assert_eq!(response.usage.output_tokens(), Some(69));
        assert_eq!(response.usage.total_tokens(), Some(677));
    }

    fn fixture_model() -> GeminiModelConfig {
        GeminiModelConfig::try_new("gemini-test", 1_000_000, 128_000, 4_096).expect("model")
    }

    fn fixture_request(content: Vec<ContentBlock>) -> ModelRequest {
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
                model: ModelName::try_new("gemini-test").expect("name"),
                messages: Arc::from([Message::try_new(
                    MessageId::parse("01234567-89ab-7cde-89ab-0123456789a6").expect("message"),
                    MessageRole::User,
                    content,
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
    async fn missing_media_resolver_fails_closed() {
        let config = GeminiConfig::try_new("http://127.0.0.1:9").expect("config");
        let provider =
            GeminiProvider::try_new(config, vec![fixture_model().with_input_images(true)])
                .expect("provider");
        let blob = BlobRef::try_new("blob-1", "image/png", 4, None, None::<&str>).expect("blob");
        let request = fixture_request(vec![ContentBlock::Image(MediaRef::new(blob))]);

        let Err(error) = provider.request(request).await else {
            panic!("media without a configured resolver must fail closed");
        };
        assert_eq!(error.code(), crate::error::GEMINI_REQUEST_INVALID);
    }

    #[test]
    fn duplicate_model_names_rejected() {
        let config = GeminiConfig::try_new("http://127.0.0.1:9").expect("config");
        let Err(error) = GeminiProvider::try_new(config, vec![fixture_model(), fixture_model()])
        else {
            panic!("a duplicate model catalog must be rejected");
        };
        assert_eq!(error.code(), crate::error::GEMINI_CONFIG_INVALID);
    }
}
