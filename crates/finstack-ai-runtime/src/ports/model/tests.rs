use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_kernel::{
    Digest, ErrorCategory, ExternalHandleRef, KernelState, Metadata, OutputSpec, ProviderIds,
    RawJson, ReconciliationPolicy, RetrySafety, RunPhase, Usage,
};

use crate::ports::PortFuture;

use super::*;

struct EstimatorModel {
    profile: ModelContextProfile,
    estimate: u64,
    estimator: TokenEstimatorRef,
}

struct WarmupModel {
    inner: EstimatorModel,
    warmups: AtomicUsize,
}

impl Model for WarmupModel {
    fn descriptor(&self) -> ModelDescriptor {
        self.inner.descriptor()
    }

    fn capabilities(&self, model: &ModelName) -> ModelCapabilities {
        self.inner.capabilities(model)
    }

    fn warmup(&self, _context: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
        self.warmups.fetch_add(1, Ordering::AcqRel);
        Box::pin(async { Ok(()) })
    }

    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        self.inner.estimate_input_tokens(model, canonical_request)
    }

    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        self.inner.request(request)
    }
}

impl Model for EstimatorModel {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            provider: Arc::from("test"),
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
        _canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        Ok(ModelTokenEstimate {
            input_tokens: self.estimate,
            estimator: self.estimator.clone(),
        })
    }

    fn request(&self, _request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        Box::pin(async {
            Err(ModelError::validation(
                "model_request_invalid",
                "not used by profile tests",
            ))
        })
    }
}

fn profile() -> ModelContextProfile {
    ModelContextProfile {
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
    }
}

fn draft() -> ModelRequestDraft {
    ModelRequestDraft {
        model: profile().model,
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
    }
}

#[tokio::test]
async fn ready_model_is_a_reusable_proof_for_one_successful_warmup() {
    let provider = profile();
    let model = Arc::new(WarmupModel {
        inner: EstimatorModel {
            profile: provider.clone(),
            estimate: 1,
            estimator: provider.estimator,
        },
        warmups: AtomicUsize::new(0),
    });
    let model_port: Arc<dyn Model> = model.clone();
    let ready = ReadyModel::prepare(model_port).await.expect("ready model");

    assert_eq!(model.warmups.load(Ordering::Acquire), 1);
    let first = ready.shared_model();
    let second = ready.shared_model();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(model.warmups.load(Ordering::Acquire), 1);
}

#[test]
fn cancellation_hierarchy_propagates_downward_only_and_late_children_start_cancelled() {
    let run = CancellationSignal::new();
    let model = run.child();
    let tool_batch = run.child();
    let first_tool = tool_batch.child();
    let second_tool = tool_batch.child();

    first_tool.cancel();
    assert!(first_tool.is_cancelled());
    assert!(!second_tool.is_cancelled());
    assert!(!tool_batch.is_cancelled());
    assert!(!run.is_cancelled());

    run.cancel();
    assert!(model.is_cancelled());
    assert!(tool_batch.is_cancelled());
    assert!(second_tool.is_cancelled());
    assert!(run.child().is_cancelled());
}

#[test]
fn profile_overrides_only_tighten_and_lock_stably() {
    let overlay = ModelContextProfileOverride {
        hard_input_bytes: Some(900),
        max_output_tokens: Some(10),
        reserved_output_tokens: Some(25),
        ..ModelContextProfileOverride::default()
    };
    let first =
        resolve_model_context_profile(profile(), Some(&overlay), None, false).expect("profile");
    let second =
        resolve_model_context_profile(profile(), Some(&overlay), None, false).expect("profile");
    assert_eq!(first, second);
    assert_eq!(first.profile.hard_input_bytes, 900);
    assert_eq!(first.profile.max_output_tokens, 10);
    assert_eq!(first.profile.reserved_output_tokens, 25);

    let mut other_provider = profile();
    other_provider.provider = Arc::from("other");
    let other = resolve_model_context_profile(other_provider, Some(&overlay), None, false)
        .expect("other provider");
    assert_ne!(first.digest, other.digest);
}

#[test]
fn profile_rejects_relaxation_and_nonallowlisted_run_override() {
    let relaxation = ModelContextProfileOverride {
        max_output_tokens: Some(21),
        ..ModelContextProfileOverride::default()
    };
    let error = resolve_model_context_profile(profile(), Some(&relaxation), None, false)
        .expect_err("relaxation");
    assert_eq!(error.code(), MODEL_PROFILE_RELAXATION);

    let tightening = ModelContextProfileOverride {
        max_output_tokens: Some(19),
        ..ModelContextProfileOverride::default()
    };
    let error = resolve_model_context_profile(profile(), None, Some(&tightening), false)
        .expect_err("allowlist");
    assert_eq!(error.code(), MODEL_PROFILE_OVERRIDE_NOT_ALLOWED);

    let allowed = resolve_model_context_profile(profile(), None, Some(&tightening), true)
        .expect("allowlisted tightening");
    assert_eq!(allowed.profile.max_output_tokens, 19);
}

#[test]
fn profile_rejects_invalid_boundaries_and_overflow() {
    let mut invalid = profile();
    invalid.hard_input_bytes = 0;
    assert_eq!(
        resolve_model_context_profile(invalid, None, None, false)
            .expect_err("zero")
            .code(),
        MODEL_PROFILE_INVALID
    );
    let mut overflow = profile();
    overflow.reserved_output_tokens = u64::MAX;
    overflow.provider_overhead_tokens = 1;
    assert_eq!(
        resolve_model_context_profile(overflow, None, None, false)
            .expect_err("overflow")
            .code(),
        MODEL_PROFILE_INVALID
    );

    let mut exact_margin = profile();
    exact_margin.reserved_output_tokens = 95;
    assert!(resolve_model_context_profile(exact_margin, None, None, false).is_ok());
    let mut margin_over = profile();
    margin_over.reserved_output_tokens = 96;
    assert_eq!(
        resolve_model_context_profile(margin_over, None, None, false)
            .expect_err("margin + 1")
            .code(),
        MODEL_PROFILE_INVALID
    );
}

#[test]
fn request_validator_accepts_exact_limits_and_rejects_limit_plus_one() {
    let mut request = draft();
    for _ in 0..4 {
        let length =
            u64::try_from(request.canonical_bytes().expect("bytes").len()).expect("length");
        if request.limits.max_input_bytes == length {
            break;
        }
        request.limits.max_input_bytes = length;
    }
    let exact_bytes = request.limits.max_input_bytes;
    assert_eq!(
        u64::try_from(request.canonical_bytes().expect("bytes").len()).expect("length"),
        exact_bytes
    );
    let mut exact_profile = profile();
    exact_profile.hard_input_bytes = exact_bytes;
    let locked =
        resolve_model_context_profile(exact_profile.clone(), None, None, false).expect("profile");
    let exact_model = EstimatorModel {
        profile: exact_profile.clone(),
        estimate: 75,
        estimator: exact_profile.estimator.clone(),
    };
    let validated = validate_model_request(&exact_model, &request, &locked).expect("exact limits");
    assert_eq!(validated.estimated_input_tokens, 75);
    assert_eq!(validated.available_input_tokens, 75);

    let token_over = EstimatorModel {
        profile: exact_profile.clone(),
        estimate: 76,
        estimator: exact_profile.estimator.clone(),
    };
    assert_eq!(
        validate_model_request(&token_over, &request, &locked)
            .expect_err("token + 1")
            .code(),
        MODEL_CONTEXT_LIMIT_EXCEEDED
    );

    let mut byte_over = request.clone();
    byte_over.settings.values = RawJson::parse(br#"{"extra":true}"#).expect("settings");
    assert_eq!(
        validate_model_request(&exact_model, &byte_over, &locked)
            .expect_err("bytes + 1")
            .code(),
        MODEL_CONTEXT_LIMIT_EXCEEDED
    );

    let mut output_over = request;
    output_over.limits.max_output_tokens = 21;
    assert_eq!(
        validate_model_request(&exact_model, &output_over, &locked)
            .expect_err("output + 1")
            .code(),
        MODEL_CONTEXT_LIMIT_EXCEEDED
    );
}

#[test]
fn request_validator_rejects_estimator_identity_mismatch() {
    let request = draft();
    let provider = profile();
    let locked =
        resolve_model_context_profile(provider.clone(), None, None, false).expect("profile");
    let model = EstimatorModel {
        profile: provider,
        estimate: 1,
        estimator: TokenEstimatorRef {
            id: Arc::from("different"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    };
    assert_eq!(
        validate_model_request(&model, &request, &locked)
            .expect_err("estimator mismatch")
            .code(),
        MODEL_ESTIMATOR_MISMATCH
    );
}

#[test]
fn reserved_adapter_codes_enforce_category_and_retryability() {
    let error = ModelError::try_new(
        MODEL_STREAM_LIMIT_EXCEEDED,
        ErrorCategory::Validation,
        false,
        "wrong category",
        Metadata::empty(),
    )
    .expect_err("reserved category");
    assert_eq!(ModelError::from(error).code(), MODEL_REQUEST_INVALID);

    let error = ModelError::try_new(
        MODEL_RESPONSE_MISMATCH,
        ErrorCategory::Validation,
        true,
        "wrong retryability",
        Metadata::empty(),
    )
    .expect_err("reserved retryability");
    assert_eq!(ModelError::from(error).code(), MODEL_REQUEST_INVALID);

    let exact = ModelError::try_new(
        MODEL_STREAM_LIMIT_EXCEEDED,
        ErrorCategory::Limit,
        false,
        "bounded",
        Metadata::empty(),
    )
    .expect("reserved classification");
    assert_eq!(exact.category(), ErrorCategory::Limit);
    assert!(!exact.retryable());
}

fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    finstack_ai_kernel::Id::from_bytes(bytes)
}

fn requested(retry_safety: RetrySafety) -> finstack_ai_kernel::EffectRequested {
    finstack_ai_kernel::EffectRequested::try_new(
        id(4),
        finstack_ai_kernel::EffectKind::Model,
        None,
        None,
        None,
        finstack_ai_kernel::EffectOutputContract {
            kind: finstack_ai_kernel::EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"model-response"),
        },
        finstack_ai_kernel::EffectInput::Model {
            request: RawJson::parse(b"{}").expect("request"),
        },
        retry_safety,
        None,
    )
    .expect("requested")
}

fn pending(
    deferred: Option<finstack_ai_kernel::EffectDeferred>,
) -> finstack_ai_kernel::PendingModelEffect {
    finstack_ai_kernel::PendingModelEffect {
        cycle: 0,
        turn_id: id(7),
        model_request_id: id(5),
        requested: requested(RetrySafety::SafeToRetry),
        deferred,
    }
}

fn deferred(policy: ReconciliationPolicy) -> finstack_ai_kernel::EffectDeferred {
    finstack_ai_kernel::EffectDeferred {
        effect_id: id(4),
        handle: ExternalHandleRef::try_new(
            finstack_ai_kernel::ComponentId::parse("finstack.model.scripted").expect("component"),
            "handle-1",
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: policy,
        next_poll_at: None,
        expires_at: None,
        output_contract: requested(RetrySafety::SafeToRetry)
            .output_contract()
            .clone(),
    }
}

fn state_with(
    phase: Option<RunPhase>,
    pending_effect: Option<finstack_ai_kernel::PendingModelEffect>,
    settled: bool,
) -> KernelState {
    let mut state = KernelState {
        phase,
        pending_model_effect: pending_effect,
        ..KernelState::default()
    };
    if settled && let Some(pending) = state.pending_model_effect.as_ref() {
        let effect_id = pending.requested.effect_id();
        state.model_settlements.insert(
            effect_id,
            finstack_ai_kernel::ModelSettlementFingerprint {
                kind: finstack_ai_kernel::ModelSettlementKind::Completed,
                digest: Digest::raw_json(b"settled"),
            },
        );
    }
    state
}

#[test]
fn model_resume_action_classifies_journal_only_states() {
    assert_eq!(
        model_resume_action(&KernelState::default()),
        ModelResumeAction::NoOutstanding
    );
    assert_eq!(
        model_resume_action(&state_with(
            Some(RunPhase::AwaitingModel),
            Some(pending(None)),
            false
        )),
        ModelResumeAction::Reconcile
    );
    assert_eq!(
        model_resume_action(&state_with(
            Some(RunPhase::AwaitingModel),
            Some(pending(None)),
            true
        )),
        ModelResumeAction::UseRecorded
    );
    assert_eq!(
        model_resume_action(&state_with(
            Some(RunPhase::AwaitingExternal),
            Some(pending(Some(deferred(ReconciliationPolicy::CallbackOnly)))),
            false
        )),
        ModelResumeAction::WaitExternal
    );
    assert_eq!(
        model_resume_action(&state_with(
            Some(RunPhase::AwaitingExternal),
            Some(pending(Some(deferred(ReconciliationPolicy::Poll)))),
            false
        )),
        ModelResumeAction::Reconcile
    );
    assert_eq!(
        model_resume_action(&state_with(
            Some(RunPhase::AwaitingExternal),
            Some(pending(Some(deferred(
                ReconciliationPolicy::CallbackOrPoll
            )))),
            false
        )),
        ModelResumeAction::Reconcile
    );
    let settled = state_with(Some(RunPhase::BeforeFinalize), None, false);
    assert_eq!(
        model_resume_action(&settled),
        ModelResumeAction::NoOutstanding
    );
    assert_ne!(
        model_resume_action(&state_with(
            Some(RunPhase::AwaitingModel),
            Some(pending(None)),
            false
        )),
        ModelResumeAction::Retry
    );
}

#[test]
fn model_resume_maps_reconcile_results_to_documented_actions() {
    let awaiting = state_with(Some(RunPhase::AwaitingModel), Some(pending(None)), false);
    let deferred_state = state_with(
        Some(RunPhase::AwaitingExternal),
        Some(pending(Some(deferred(
            ReconciliationPolicy::CallbackOrPoll,
        )))),
        false,
    );
    let response = ModelResponse {
        assistant_content: Arc::from([]),
        tool_calls: Arc::from([]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from("completion-1"),
        continuation_state: None,
    };
    assert_eq!(
        map_model_reconcile_result(
            &awaiting,
            &ModelReconcileResult::Completed(response.clone()),
            true
        ),
        ModelResumeAction::UseRecorded
    );
    assert_eq!(
        map_model_reconcile_result(
            &awaiting,
            &ModelReconcileResult::StillRunning(ModelDeferral {
                handle: deferred(ReconciliationPolicy::CallbackOrPoll).handle,
                reconciliation: ReconciliationPolicy::CallbackOrPoll,
                next_poll_at: None,
                expires_at: None,
            }),
            true
        ),
        ModelResumeAction::WaitExternal
    );
    assert_eq!(
        map_model_reconcile_result(&awaiting, &ModelReconcileResult::NotStarted, true),
        ModelResumeAction::Retry
    );
    assert_eq!(
        map_model_reconcile_result(&awaiting, &ModelReconcileResult::Unknown, true),
        ModelResumeAction::Retry
    );
    assert_eq!(
        map_model_reconcile_result(&awaiting, &ModelReconcileResult::Unknown, false),
        ModelResumeAction::SuspendUncertain
    );
    assert_eq!(
        map_model_reconcile_result(&awaiting, &ModelReconcileResult::NonRepeatable, true),
        ModelResumeAction::SuspendUncertain
    );
    assert_eq!(
        map_model_reconcile_result(&deferred_state, &ModelReconcileResult::NotStarted, true),
        ModelResumeAction::SuspendUncertain
    );
    assert_eq!(
        map_model_reconcile_result(
            &state_with(Some(RunPhase::AwaitingModel), Some(pending(None)), true),
            &ModelReconcileResult::Completed(response),
            true
        ),
        ModelResumeAction::UseRecorded
    );
    assert!(!model_retry_allowed(
        &requested(RetrySafety::AtMostOnce),
        &ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: false,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: profile(),
            native_tool_calls: false,
            parallel_tool_calls: false,
            structured_output: StructuredOutputCapability::Unsupported,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: true,
            native_capabilities: BTreeSet::new(),
        }
    ));
}
