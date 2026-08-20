//! Private bounded native model job/result path.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    EffectId, EffectInput, KernelInput, PostCommitAction, ReducerStageOutcome,
};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::coordinator::{DispatchError, ModelDispatchSeed, PostCommitDispatcher, RuntimeDispatch};
use crate::run_types::SameIdentityRetryPolicy;
use crate::settlement::ModelDriverResult;
use crate::{
    CancellationSignal, Clock, LockedModelContextProfile, Metadata, Model, ModelCallContext,
    ModelError, ModelProgress, ModelRequest, ModelStreamAssembler, ModelTerminal,
    MonotonicDeadline, PortFuture, RunCallContext, parse_committed_model_request,
    stable_model_dispatch_code,
};

pub(crate) struct ModelJob {
    pub(crate) seed: ModelDispatchSeed,
    pub(crate) request: ModelRequest,
}

pub(crate) enum ModelDriverMessage {
    Progress {
        effect_id: EffectId,
        provider: Arc<str>,
        progress: ModelProgress,
    },
    Terminal(Box<ModelDriverResult>),
}

type ActiveEffects = Arc<Mutex<BTreeMap<EffectId, CancellationSignal>>>;

pub(crate) struct ModelDispatcher {
    model: Arc<dyn Model>,
    profile: LockedModelContextProfile,
    jobs: mpsc::Sender<ModelJob>,
    active: ActiveEffects,
    parent: CancellationSignal,
}

impl ModelDispatcher {
    pub(crate) fn new(
        model: Arc<dyn Model>,
        profile: LockedModelContextProfile,
        jobs: mpsc::Sender<ModelJob>,
        parent: CancellationSignal,
    ) -> Self {
        Self {
            model,
            profile,
            jobs,
            active: Arc::new(Mutex::new(BTreeMap::new())),
            parent,
        }
    }

    pub(crate) fn active(&self) -> ActiveEffects {
        Arc::clone(&self.active)
    }

    pub(crate) async fn resume_request(
        &self,
        seed: ModelDispatchSeed,
    ) -> Result<(), DispatchError> {
        let effect_id = seed.pending.requested.effect_id();
        self.dispatch(RuntimeDispatch {
            action: finstack_ai_kernel::PostCommitAction::ExecuteEffect { effect_id },
            model: Some(seed),
            tool: None,
            timer: None,
            context: None,
        })
        .await
    }
}

impl PostCommitDispatcher for ModelDispatcher {
    fn validate_before_commit(&self, input: &KernelInput) -> Result<(), DispatchError> {
        let KernelInput::StageSettled(settled) = input else {
            return Ok(());
        };
        let ReducerStageOutcome::ModelRequestPrepared { request, .. } = &settled.outcome else {
            return Ok(());
        };
        parse_committed_model_request(self.model.as_ref(), &self.profile, request)
            .map(|_| ())
            .map_err(|error| DispatchError {
                code: stable_model_dispatch_code(error.code()),
            })
    }

    fn dispatch(&self, dispatch: RuntimeDispatch) -> PortFuture<Result<(), DispatchError>> {
        match dispatch.action {
            PostCommitAction::CancelEffect { effect_id } => {
                let cancellation = match crate::coordinator::cancel_registered_effect(
                    &self.active,
                    effect_id,
                    "model_effect_registry_unavailable",
                ) {
                    Ok(cancellation) => cancellation,
                    Err(error) => return Box::pin(async move { Err(error) }),
                };
                Box::pin(async move {
                    // Completion may win the active-map race after the durable
                    // cancellation decision. The run worker still rejects the
                    // late terminal from committed state, so absence is an
                    // idempotent cancellation acknowledgement rather than a
                    // lane fault.
                    if let Some(cancellation) = cancellation {
                        cancellation.cancel();
                    }
                    Ok(())
                })
            }
            PostCommitAction::ExecuteEffect { effect_id } => {
                let Some(seed) = dispatch.model else {
                    return Box::pin(async {
                        Err(DispatchError {
                            code: "unsupported_effect_driver",
                        })
                    });
                };
                let EffectInput::Model { request: raw } = seed.pending.requested.input() else {
                    return Box::pin(async {
                        Err(DispatchError {
                            code: "model_request_invalid",
                        })
                    });
                };
                let draft =
                    match parse_committed_model_request(self.model.as_ref(), &self.profile, raw) {
                        Ok(draft) => draft,
                        Err(error) => {
                            return Box::pin(async move {
                                Err(DispatchError {
                                    code: stable_model_dispatch_code(error.code()),
                                })
                            });
                        }
                    };
                let cancellation = self.parent.child();
                {
                    let Ok(mut active) = self.active.lock() else {
                        return Box::pin(async {
                            Err(DispatchError {
                                code: "model_effect_registry_unavailable",
                            })
                        });
                    };
                    if active.insert(effect_id, cancellation.clone()).is_some() {
                        return Box::pin(async {
                            Err(DispatchError {
                                code: "model_effect_already_active",
                            })
                        });
                    }
                }
                let request = ModelRequest {
                    call: ModelCallContext {
                        run: RunCallContext {
                            locator: seed.locator.clone(),
                            authorization: seed.authorization.clone(),
                            effect_id,
                            attempt: seed.attempt,
                            deadline: seed.pending.requested.deadline(),
                            budget_scope_id: seed.budget_scope_id,
                            cancellation,
                            relation_depth: seed.relation_depth,
                        },
                        request_id: seed.pending.model_request_id,
                    },
                    draft,
                    continuation_state: seed.continuation_state.clone(),
                };
                let jobs = self.jobs.clone();
                let active = Arc::clone(&self.active);
                Box::pin(async move {
                    if jobs.send(ModelJob { seed, request }).await.is_err() {
                        if let Ok(mut values) = active.lock() {
                            values.remove(&effect_id);
                        }
                        return Err(DispatchError {
                            code: "model_job_queue_closed",
                        });
                    }
                    Ok(())
                })
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the native job loop keeps model, queues, clock, and retry policy contiguous"
)]
pub(crate) async fn run_model_jobs<C>(
    model: Arc<dyn Model>,
    assembler: ModelStreamAssembler,
    active: ActiveEffects,
    mut jobs: mpsc::Receiver<ModelJob>,
    results: mpsc::Sender<ModelDriverMessage>,
    clock: Arc<C>,
    cancellation_grace: Duration,
    retry_policy: SameIdentityRetryPolicy,
) where
    C: Clock + Send + Sync + 'static,
{
    let provider = model.descriptor().provider;
    let mut tasks = JoinSet::new();
    let mut intake_open = true;
    while intake_open || !tasks.is_empty() {
        tokio::select! {
            job = jobs.recv(), if intake_open => {
                if let Some(job) = job {
                    let model = Arc::clone(&model);
                    let provider = Arc::clone(&provider);
                    let results = results.clone();
                    let active = Arc::clone(&active);
                    let clock = Arc::clone(&clock);
                    tasks.spawn(async move {
                        let effect_id = job.seed.pending.requested.effect_id();
                        let draft = job.request.draft.clone();
                        let progress_sender = results.clone();
                        let progress_provider = Arc::clone(&provider);
                        let cancellation = job.request.call.run.cancellation.clone();
                        let deadline = job
                            .request
                            .call
                            .run
                            .deadline
                            .map(|deadline| {
                                MonotonicDeadline::from_persisted(
                                    clock.as_ref(),
                                    job.seed.requested_at,
                                    deadline,
                                )
                            })
                            .transpose();
                        let result = match model_job_preflight(deadline, &cancellation) {
                            Err(error) => Err(error),
                            Ok(deadline) => {
                            let retry_deadline = deadline.clone();
                            let mut child = tokio::spawn(async move {
                                request_with_same_identity_retry(
                                    model,
                                    job.request,
                                    assembler,
                                    retry_policy,
                                    clock,
                                    effect_id,
                                    progress_sender,
                                    progress_provider,
                                    retry_deadline,
                                )
                                .await
                            });
                            if let Some(deadline) = deadline {
                                tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => {
                                        settle_model_cancellation(&mut child, cancellation_grace).await
                                    }
                                    () = deadline.wait() => {
                                        cancellation.cancel();
                                        child.abort();
                                        let _ = (&mut child).await;
                                        Err(model_deadline_error())
                                    }
                                    joined = &mut child => joined_model_result(joined),
                                }
                            } else {
                                tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => {
                                        settle_model_cancellation(&mut child, cancellation_grace).await
                                    }
                                    joined = &mut child => joined_model_result(joined),
                                }
                            }
                            }
                        };
                        if let Ok(mut values) = active.lock() {
                            values.remove(&effect_id);
                        }
                        let _ = results.send(ModelDriverMessage::Terminal(Box::new(ModelDriverResult {
                            seed: job.seed,
                            draft,
                            provider,
                            result,
                        }))).await;
                    });
                } else {
                    intake_open = false;
                    if let Ok(values) = active.lock() {
                        for cancellation in values.values() {
                            cancellation.cancel();
                        }
                    }
                }
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
}

async fn settle_model_cancellation(
    child: &mut tokio::task::JoinHandle<Result<ModelTerminal, ModelError>>,
    grace: Duration,
) -> Result<ModelTerminal, ModelError> {
    if let Ok(joined) = timeout(grace, &mut *child).await {
        return joined_model_result(joined);
    }
    child.abort();
    let _ = child.await;
    Err(model_cancellation_error(
        "model request exceeded its cancellation grace period",
    ))
}

fn model_job_preflight(
    deadline: Result<Option<MonotonicDeadline>, crate::RuntimeTimeError>,
    cancellation: &CancellationSignal,
) -> Result<Option<MonotonicDeadline>, ModelError> {
    if cancellation.is_cancelled() {
        return Err(model_cancellation_error(
            "model request was cancelled before execution",
        ));
    }
    let deadline = deadline.map_err(|_| model_deadline_error())?;
    if deadline
        .as_ref()
        .is_some_and(|deadline| deadline.remaining() == finstack_ai_kernel::Duration::ZERO)
    {
        cancellation.cancel();
        return Err(model_deadline_error());
    }
    Ok(deadline)
}

fn model_cancellation_error(message: &'static str) -> ModelError {
    ModelError::frozen(
        "model_cancelled",
        finstack_ai_kernel::ErrorCategory::Cancellation,
        false,
        message,
    )
}

fn joined_model_result(
    joined: Result<Result<ModelTerminal, ModelError>, tokio::task::JoinError>,
) -> Result<ModelTerminal, ModelError> {
    match joined {
        Ok(result) => result,
        Err(_) => Err(ModelError::frozen(
            "model_panicked",
            finstack_ai_kernel::ErrorCategory::Internal,
            false,
            "model execution failed at the native task boundary",
        )),
    }
}

fn model_deadline_error() -> ModelError {
    ModelError::frozen(
        "model_deadline_exceeded",
        finstack_ai_kernel::ErrorCategory::Deadline,
        false,
        "model request exceeded its committed deadline",
    )
}

fn progress_delivery_error() -> ModelError {
    ModelError::frozen(
        "model_progress_delivery_closed",
        finstack_ai_kernel::ErrorCategory::Internal,
        false,
        "runtime model progress delivery closed",
    )
}

fn same_identity_retryable(error: &ModelError) -> bool {
    error.retryable()
        && !matches!(
            error.category(),
            finstack_ai_kernel::ErrorCategory::Validation
                | finstack_ai_kernel::ErrorCategory::Limit
        )
}

fn retry_after_wait(error: &ModelError) -> Duration {
    parse_retry_after_seconds(error.metadata()).map_or(Duration::ZERO, Duration::from_secs)
}

fn finite_seconds_to_u64(seconds: f64) -> Option<u64> {
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return None;
    }
    let ceiled = seconds.ceil();
    // 2^53 is the largest integer f64 can represent exactly.
    if ceiled >= 9_007_199_254_740_992.0 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "retry-after is clamped to a non-negative finite second count"
    )]
    Some(ceiled as u64)
}

fn parse_retry_after_seconds(metadata: &Metadata) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(metadata.as_str()).ok()?;
    let object = value.as_object()?;
    let raw = object
        .get("retry_after")
        .or_else(|| object.get("Retry-After"))?;
    match raw {
        serde_json::Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().and_then(finite_seconds_to_u64)),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn deadline_due(clock: &impl Clock, deadline: Option<&MonotonicDeadline>) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    match clock.now() {
        Ok(now) => now >= deadline.wall_deadline(),
        Err(_) => true,
    }
}

fn cap_retry_after(
    delay: Duration,
    clock: &impl Clock,
    deadline: Option<&MonotonicDeadline>,
) -> Option<Duration> {
    let Some(deadline) = deadline else {
        return Some(delay);
    };
    let now = clock.now().ok()?;
    let remaining_ms = deadline
        .wall_deadline()
        .as_unix_ms()
        .checked_sub(now.as_unix_ms())?;
    if remaining_ms <= 0 {
        return None;
    }
    let remaining = u64::try_from(remaining_ms)
        .ok()
        .map(Duration::from_millis)?;
    Some(delay.min(remaining))
}

async fn emit_attempt_heartbeat(
    effect_id: EffectId,
    provider: Arc<str>,
    attempt: u32,
    sender: &mpsc::Sender<ModelDriverMessage>,
) -> Result<(), ModelError> {
    let metadata = Metadata::parse(format!(r#"{{"attempt":{attempt}}}"#)).map_err(|_| {
        ModelError::frozen(
            "model_progress_delivery_closed",
            finstack_ai_kernel::ErrorCategory::Internal,
            false,
            "same-identity retry attempt metadata is invalid",
        )
    })?;
    sender
        .send(ModelDriverMessage::Progress {
            effect_id,
            provider,
            progress: ModelProgress::Heartbeat(metadata),
        })
        .await
        .map_err(|_| progress_delivery_error())
}

#[expect(
    clippy::too_many_arguments,
    reason = "same-identity retry keeps the committed request, progress sink, and deadline together"
)]
async fn request_with_same_identity_retry<C>(
    model: Arc<dyn Model>,
    request: ModelRequest,
    assembler: ModelStreamAssembler,
    policy: SameIdentityRetryPolicy,
    clock: Arc<C>,
    effect_id: EffectId,
    progress_sender: mpsc::Sender<ModelDriverMessage>,
    progress_provider: Arc<str>,
    deadline: Option<MonotonicDeadline>,
) -> Result<ModelTerminal, ModelError>
where
    C: Clock + Send + Sync + 'static,
{
    let max_attempts = policy.max_retries.saturating_add(1);
    let mut attempt = 0_u32;
    loop {
        attempt += 1;
        if deadline_due(clock.as_ref(), deadline.as_ref()) {
            return Err(model_deadline_error());
        }
        emit_attempt_heartbeat(
            effect_id,
            Arc::clone(&progress_provider),
            attempt,
            &progress_sender,
        )
        .await?;
        let outcome = match model.request(request.clone()).await {
            Ok(stream) => {
                assembler
                    .assemble_incremental(stream, {
                        let progress_sender = progress_sender.clone();
                        let progress_provider = Arc::clone(&progress_provider);
                        move |progress| {
                            let sender = progress_sender.clone();
                            let provider = Arc::clone(&progress_provider);
                            async move {
                                sender
                                    .send(ModelDriverMessage::Progress {
                                        effect_id,
                                        provider,
                                        progress,
                                    })
                                    .await
                                    .map_err(|_| progress_delivery_error())
                            }
                        }
                    })
                    .await
            }
            Err(error) => Err(error),
        };
        match outcome {
            Ok(terminal) => return Ok(terminal),
            Err(error) if attempt < max_attempts && same_identity_retryable(&error) => {
                let delay = retry_after_wait(&error);
                let Some(wait) = cap_retry_after(delay, clock.as_ref(), deadline.as_ref()) else {
                    return Err(error);
                };
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::task::{Context, Poll};

    use finstack_ai_kernel::{
        ContentBlock, EffectId, EffectTag, ErrorCategory, Id, IdTag, Metadata, ModelRequestTag,
        PrincipalRef, ProviderIds, RawJson, TextBlock, Timestamp,
    };
    use futures_core::Stream;
    use tokio::sync::mpsc;

    use super::*;
    use crate::{
        AuthorizationContext, InputCapabilities, ModelCapabilities, ModelContextProfile,
        ModelDescriptor, ModelEventStream, ModelName, ModelRequestDraft, ModelRequestLimits,
        ModelResponse, ModelSettings, ModelStreamItem, ModelStreamLimits, ModelTokenEstimate,
        ModelWarmupContext, OutputSpec, StructuredOutputCapability, TextDelta, TokenEstimatorRef,
        TokenEstimatorSource, Usage,
    };

    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Result<Timestamp, crate::IdGenerationError> {
            Ok(self.0)
        }
    }

    struct RetryingModel {
        profile: ModelContextProfile,
        requests: AtomicU64,
        effect_ids: std::sync::Mutex<Vec<EffectId>>,
    }

    impl RetryingModel {
        fn new() -> Self {
            Self {
                profile: ModelContextProfile {
                    provider: Arc::from("scripted"),
                    model: ModelName::try_new("scripted-1").expect("model"),
                    hard_input_bytes: 1_000,
                    context_window_tokens: 100,
                    max_output_tokens: 20,
                    reserved_output_tokens: 20,
                    provider_overhead_tokens: 5,
                    estimator: TokenEstimatorRef {
                        id: Arc::from("bytes-upper-bound"),
                        version: Arc::from("1"),
                        source: TokenEstimatorSource::ConservativeUpperBound,
                    },
                },
                requests: AtomicU64::new(0),
                effect_ids: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Model for RetryingModel {
        fn descriptor(&self) -> ModelDescriptor {
            ModelDescriptor {
                provider: Arc::clone(&self.profile.provider),
                models: Arc::from([self.profile.model.clone()]),
                metadata: Metadata::empty(),
            }
        }

        fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
            ModelCapabilities {
                input: InputCapabilities {
                    text: true,
                    json: true,
                    images: false,
                    audio: false,
                    files: false,
                },
                context_profile: self.profile.clone(),
                native_tool_calls: false,
                parallel_tool_calls: false,
                structured_output: StructuredOutputCapability::Unsupported,
                reasoning: false,
                prompt_cache: false,
                resumable_stream: false,
                idempotent_requests: true,
                native_capabilities: BTreeSet::new(),
            }
        }

        fn estimate_input_tokens(
            &self,
            _model: &ModelName,
            canonical_request: &[u8],
        ) -> Result<ModelTokenEstimate, ModelError> {
            Ok(ModelTokenEstimate {
                input_tokens: u64::try_from(canonical_request.len()).unwrap_or(1).max(1),
                estimator: self.profile.estimator.clone(),
            })
        }

        fn warmup(&self, _ctx: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
            Box::pin(async { Ok(()) })
        }

        fn request(
            &self,
            request: ModelRequest,
        ) -> PortFuture<Result<ModelEventStream, ModelError>> {
            if let Ok(mut ids) = self.effect_ids.lock() {
                ids.push(request.call.run.effect_id);
            }
            let attempt = self.requests.fetch_add(1, Ordering::AcqRel) + 1;
            if attempt == 1 {
                return Box::pin(async {
                    Err(ModelError::try_new(
                        "provider_overloaded",
                        ErrorCategory::Model,
                        true,
                        "retryable 429",
                        Metadata::parse(r#"{"retry_after":0}"#).expect("metadata"),
                    )
                    .expect("retryable error"))
                });
            }
            Box::pin(async {
                Ok(Box::pin(OnceStream::new(vec![
                    Ok(ModelStreamItem::TextDelta(TextDelta {
                        text: Arc::from("ok"),
                    })),
                    Ok(ModelStreamItem::Completed(ModelResponse {
                        assistant_content: Arc::from([ContentBlock::Text(
                            TextBlock::try_new("ok").expect("text"),
                        )]),
                        tool_calls: Arc::from([]),
                        usage: Usage::empty(),
                        provider_ids: ProviderIds::empty(),
                        completion_id: Arc::from("retry-completion"),
                        continuation_state: None,
                    })),
                ])) as ModelEventStream)
            })
        }
    }

    struct OnceStream<T> {
        items: VecDeque<T>,
    }

    impl<T> Unpin for OnceStream<T> {}

    impl<T> OnceStream<T> {
        fn new(items: Vec<T>) -> Self {
            Self {
                items: items.into(),
            }
        }
    }

    impl<T> Stream for OnceStream<T> {
        type Item = T;

        fn poll_next(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.get_mut().items.pop_front())
        }
    }

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn test_request(effect_id: EffectId) -> ModelRequest {
        ModelRequest {
            call: ModelCallContext {
                run: RunCallContext {
                    locator: finstack_ai_kernel::OperationLocator::try_new(
                        "tenant-a",
                        id(1),
                        id(2),
                        id(3),
                    )
                    .expect("locator"),
                    authorization: AuthorizationContext {
                        principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                            .expect("principal"),
                        authentication_method: Arc::from("test"),
                        assurance_level: Arc::from("low"),
                        roles: Arc::from([]),
                        permitted_scopes: Arc::from([]),
                        safe_claims: Metadata::empty(),
                        policy_version: Arc::from("1"),
                        decision_id: Arc::from("decision-1"),
                    },
                    effect_id,
                    attempt: 1,
                    deadline: None,
                    budget_scope_id: None,
                    cancellation: CancellationSignal::new(),
                    relation_depth: 0,
                },
                request_id: id::<ModelRequestTag>(4),
            },
            draft: ModelRequestDraft {
                model: ModelName::try_new("scripted-1").expect("model"),
                messages: Arc::from([]),
                tools: Arc::from([]),
                output: OutputSpec::PlainText,
                settings: ModelSettings {
                    values: RawJson::parse(b"{}").expect("settings"),
                },
                limits: ModelRequestLimits {
                    max_input_bytes: 1_000,
                    max_input_tokens: 75,
                    max_output_tokens: 20,
                },
            },
            continuation_state: None,
        }
    }

    #[tokio::test]
    async fn retryable_model_error_retries_same_effect_id_when_policy_enabled() {
        let model = Arc::new(RetryingModel::new());
        let effect_id = id::<EffectTag>(9);
        let (sender, mut receiver) = mpsc::channel(8);
        let assembler = ModelStreamAssembler::new(ModelStreamLimits::default()).expect("assembler");
        let result = request_with_same_identity_retry(
            model.clone(),
            test_request(effect_id),
            assembler,
            SameIdentityRetryPolicy { max_retries: 1 },
            Arc::new(FixedClock(Timestamp::from_unix_ms(1_000).expect("now"))),
            effect_id,
            sender,
            Arc::from("scripted"),
            None,
        )
        .await
        .expect("retried success");
        assert!(matches!(result, ModelTerminal::Completed(_)));
        assert_eq!(model.requests.load(Ordering::Acquire), 2);
        let ids = model.effect_ids.lock().expect("ids");
        assert_eq!(ids.as_slice(), [effect_id, effect_id]);
        let mut heartbeats = Vec::new();
        while let Ok(message) = receiver.try_recv() {
            if let ModelDriverMessage::Progress {
                progress: ModelProgress::Heartbeat(metadata),
                effect_id: observed,
                ..
            } = message
            {
                assert_eq!(observed, effect_id);
                heartbeats.push(metadata.as_str().to_owned());
            }
        }
        assert_eq!(heartbeats, [r#"{"attempt":1}"#, r#"{"attempt":2}"#]);
    }
}
