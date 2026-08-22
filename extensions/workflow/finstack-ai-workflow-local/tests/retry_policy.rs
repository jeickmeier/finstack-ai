//! Runtime `retry_decision` parity (moved from the Temporal shim).

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{
    BudgetPropagation, CancellationPropagation, DeadlinePropagation, Digest, EffectInput,
    EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested, Id, IdTag,
    PendingModelEffect, PrincipalPropagation, PrincipalRef, RawJson, RetrySafety, RunAccepted,
    RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext,
};
use finstack_ai_kernel::{EffectId, KernelState};
use finstack_ai_runtime::ports::model::{
    InputCapabilities, ModelCapabilities, ModelContextProfile, ModelName,
    StructuredOutputCapability, TokenEstimatorRef, TokenEstimatorSource,
};
use finstack_ai_runtime::workflow::{WorkflowRetryDecision, retry_decision};

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

fn requested() -> EffectRequested {
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
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("requested")
}

fn capabilities() -> ModelCapabilities {
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
        idempotent_requests: true,
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

fn outstanding_state(used_retries: u32) -> KernelState {
    KernelState {
        pending_model_effect: Some(PendingModelEffect {
            cycle: 0,
            turn_id: id(4),
            model_request_id: id(5),
            requested: requested(),
            deferred: None,
        }),
        accepted: Some(accepted_with_retries(Some(1))),
        limit_usage: finstack_ai_kernel::LimitUsage {
            retries: used_retries,
            ..finstack_ai_kernel::LimitUsage::default()
        },
        ..KernelState::default()
    }
}

#[test]
fn retry_decision_cannot_exceed_kernel_max_retries() {
    let capabilities = capabilities();
    let first = retry_decision(
        &outstanding_state(0),
        effect_id(3),
        Some(&capabilities),
        None,
    );
    assert_eq!(first, WorkflowRetryDecision::Allow { remaining: Some(1) });

    let denied = retry_decision(
        &outstanding_state(1),
        effect_id(3),
        Some(&capabilities),
        None,
    );
    assert_eq!(
        denied,
        WorkflowRetryDecision::Deny {
            code: "retry_limit_reached",
        }
    );

    for _ in 0..5 {
        let again = retry_decision(
            &outstanding_state(1),
            effect_id(3),
            Some(&capabilities),
            None,
        );
        assert_eq!(
            again,
            WorkflowRetryDecision::Deny {
                code: "retry_limit_reached",
            }
        );
    }
}
