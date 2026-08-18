use super::*;
use crate::{
    InputCapabilities, ModelContextProfile, ModelName, StructuredOutputCapability,
    TokenEstimatorRef, TokenEstimatorSource,
};
use finstack_ai_kernel::{
    BudgetPropagation, CancellationPropagation, DeadlinePropagation, Digest, EffectDeferred,
    EffectInput, EffectOutputContract, EffectOutputKind, ErrorCategory, ErrorDescriptor, Id, IdTag,
    PendingModelEffect, PrincipalPropagation, PrincipalRef, RawJson, RetryClassification,
    RetrySafety, RetryScheduled, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
    RunSecurityContext,
};
use std::collections::BTreeSet;

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn effect_id(ordinal: u64) -> EffectId {
    id(ordinal)
}

fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

fn requested(safety: RetrySafety) -> EffectRequested {
    EffectRequested::try_new(
        effect_id(3),
        EffectKind::Model,
        None,
        None,
        None,
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"model-response"),
        },
        EffectInput::Model {
            request: RawJson::parse("{}").expect("json"),
        },
        safety,
        None,
    )
    .expect("requested")
}

fn capabilities(idempotent: bool) -> ModelCapabilities {
    ModelCapabilities {
        input: InputCapabilities {
            text: true,
            json: true,
            images: false,
            audio: false,
            files: false,
        },
        context_profile: ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("scripted-1").expect("model"),
            hard_input_bytes: 2_000,
            context_window_tokens: 2_000,
            max_output_tokens: 16,
            reserved_output_tokens: 16,
            provider_overhead_tokens: 0,
            estimator: TokenEstimatorRef {
                id: Arc::from("scripted"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        },
        native_tool_calls: false,
        parallel_tool_calls: false,
        structured_output: StructuredOutputCapability::Unsupported,
        reasoning: false,
        prompt_cache: false,
        resumable_stream: false,
        idempotent_requests: idempotent,
        native_capabilities: BTreeSet::new(),
    }
}

fn accepted_with_retries(max_retries: Option<u32>) -> RunAccepted {
    let run_id = id(9);
    let mut limits = RunLimits::empty();
    limits.max_retries = max_retries;
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        None,
        limits,
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(b"agent"),
        None,
    )
    .expect("accepted")
}

#[test]
fn classify_wait_empty_is_still_running() {
    assert!(classify_wait(&KernelState::default()).is_none());
}

#[test]
fn classify_wait_timer_from_retry_pending() {
    let mut state = KernelState::default();
    state.retry.pending = Some(
        RetryScheduled::try_new(
            0,
            1,
            RetryClassification::Model,
            "retry-v1",
            effect_id(8),
            timestamp(5_000),
            ErrorDescriptor::new("model_failed", "failed", ErrorCategory::Model, true)
                .expect("error"),
        )
        .expect("timer"),
    );
    assert_eq!(
        classify_wait(&state),
        Some(WorkflowWait::Timer {
            effect_id: effect_id(8),
            due_at: timestamp(5_000),
        })
    );
}

#[test]
fn classify_wait_deferred_model() {
    let mut state = KernelState::default();
    let requested = requested(RetrySafety::SafeToRetry);
    state.pending_model_effect = Some(PendingModelEffect {
        cycle: 0,
        turn_id: id(4),
        model_request_id: id(5),
        requested: requested.clone(),
        deferred: Some(EffectDeferred {
            effect_id: requested.effect_id(),
            handle: ExternalHandleRef::try_new(
                finstack_ai_kernel::ComponentId::parse("finstack.model.scripted")
                    .expect("component"),
                "handle-1",
                RawJson::parse("{}").expect("metadata"),
            )
            .expect("handle"),
            reconciliation: finstack_ai_kernel::ReconciliationPolicy::CallbackOnly,
            next_poll_at: None,
            expires_at: None,
            output_contract: requested.output_contract().clone(),
        }),
    });
    let Some(WorkflowWait::DeferredEffect { effect_id, .. }) = classify_wait(&state) else {
        panic!("expected deferred");
    };
    assert_eq!(effect_id, requested.effect_id());
}

#[test]
fn retry_decision_missing_effect_denies() {
    assert_eq!(
        retry_decision(&KernelState::default(), effect_id(3), None, None),
        WorkflowRetryDecision::Deny {
            code: EFFECT_NOT_OUTSTANDING,
        }
    );
}

#[test]
fn retry_decision_unsafe_and_limit() {
    let mut state = KernelState::default();
    state.pending_model_effect = Some(PendingModelEffect {
        cycle: 0,
        turn_id: id(4),
        model_request_id: id(5),
        requested: requested(RetrySafety::AtMostOnce),
        deferred: None,
    });
    assert_eq!(
        retry_decision(&state, effect_id(3), Some(&capabilities(true)), None),
        WorkflowRetryDecision::Deny {
            code: RETRY_NOT_SAFE,
        }
    );

    state.pending_model_effect = Some(PendingModelEffect {
        cycle: 0,
        turn_id: id(4),
        model_request_id: id(5),
        requested: requested(RetrySafety::SafeToRetry),
        deferred: None,
    });
    state.accepted = Some(accepted_with_retries(Some(1)));
    state.limit_usage.retries = 1;
    assert_eq!(
        retry_decision(&state, effect_id(3), Some(&capabilities(true)), None),
        WorkflowRetryDecision::Deny {
            code: RETRY_LIMIT_REACHED,
        }
    );
    state.limit_usage.retries = 0;
    assert_eq!(
        retry_decision(&state, effect_id(3), Some(&capabilities(true)), None),
        WorkflowRetryDecision::Allow { remaining: Some(1) }
    );
}

#[test]
fn checkpoint_hint_never_overrides_journal() {
    assert_eq!(resolve_checkpoint_sequence(12, Some(99)), 12);
    assert_eq!(resolve_checkpoint_sequence(12, None), 12);
}

#[test]
fn spawn_code_preserves_fault_code() {
    let error = crate::RunHandleError::Faulted {
        code: Arc::from("tool_reconciliation_unsupported"),
    };

    assert_eq!(
        spawn_code(&error).as_ref(),
        "tool_reconciliation_unsupported"
    );
}
