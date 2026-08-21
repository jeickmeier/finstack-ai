use std::sync::Arc;

use finstack_ai_kernel::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, ArtifactRef, AssignedToolCall,
    ComponentId, Digest, EffectDeferred, EffectInput, EffectKind, EffectOutputContract,
    EffectOutputKind, EffectRequested, ErrorCategory, ErrorDescriptor, ExternalHandleRef, Id,
    IdTag, KernelState, Metadata, OperationLocator, PrincipalRef, RawJson, ReconciliationPolicy,
    RetrySafety, RunId, Sensitivity, SessionId, SyntheticToolClosure, Timestamp,
    ToolBatchContinuation, ToolBatchOpened, ToolCallBlock, ToolCallPlan, ToolExecutionMode,
    ToolFailurePolicy, ToolId, ToolSettlementFingerprint, ToolSettlementKind, ValidatedToolCall,
};
use futures_util::stream;

use crate::{
    ApprovalMetadata, ApprovalRequirement, ArtifactMetadata, ArtifactScope, ArtifactStoreLimits,
    AuthorizationContext, CancellationSignal, RunCallContext, SideEffectClass, ToolCallContext,
    ToolDeferralSupport, ToolSpec, build_artifact_ref,
};

use super::error::TOOL_STREAM_LIMIT_EXCEEDED;
use super::stream::MAX_TOOL_RESULT_ARTIFACTS;
use super::*;

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn tool_deferral(next_poll_at: Option<Timestamp>, expires_at: Option<Timestamp>) -> ToolDeferral {
    ToolDeferral {
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tools.scripted").expect("component"),
            "handle-1",
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at,
        expires_at,
    }
}

fn tool_stream(items: Vec<Result<ToolStreamItem, ToolError>>) -> ToolEventStream {
    Box::pin(stream::iter(items))
}

fn artifact(ordinal: u8) -> ArtifactRef {
    build_artifact_ref(
        &ArtifactScope {
            tenant_scope: Arc::from("tenant-a"),
            session_id: SessionId::from_bytes([1; 16]),
            run_id: Some(RunId::from_bytes([2; 16])),
            sensitivity: Sensitivity::Internal,
        },
        &[ordinal],
        &ArtifactMetadata {
            kind: Arc::from("tool-output"),
            media_type: Arc::from("application/octet-stream"),
            name: None,
            attributes: Metadata::empty(),
        },
        &ArtifactStoreLimits::default(),
    )
    .expect("artifact")
}

fn tool_call() -> ToolCallBlock {
    ToolCallBlock::try_new(
        id(10),
        "echo",
        RawJson::parse(br#"{"value":1}"#).expect("arguments"),
    )
    .expect("tool call")
}

fn validated(retry_safety: RetrySafety) -> ValidatedToolCall {
    ValidatedToolCall {
        call: tool_call(),
        tool_id: ToolId::parse("finstack.tools.echo").expect("tool id"),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"tool-result"),
        },
        retry_safety,
        deadline: None,
        execution: ToolExecutionMode::Parallel,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    }
}

fn requested(effect_ordinal: u64, retry_safety: RetrySafety) -> EffectRequested {
    let call = validated(retry_safety);
    EffectRequested::try_new(
        id(effect_ordinal),
        EffectKind::Tool,
        None,
        None,
        None,
        call.output_contract.clone(),
        EffectInput::Tool { call: call.call },
        retry_safety,
        None,
    )
    .expect("requested")
}

fn deferred(effect_ordinal: u64, policy: ReconciliationPolicy) -> EffectDeferred {
    EffectDeferred {
        effect_id: id(effect_ordinal),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.tools.scripted").expect("component"),
            "handle-1",
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: policy,
        next_poll_at: None,
        expires_at: None,
        output_contract: requested(effect_ordinal, RetrySafety::SafeToRetry)
            .output_contract()
            .clone(),
    }
}

fn execute_call(
    source_index: u32,
    effect_ordinal: u64,
    status: ActiveToolCallStatus,
) -> ActiveToolCall {
    let validated = validated(RetrySafety::SafeToRetry);
    ActiveToolCall {
        assigned: AssignedToolCall {
            source_index,
            group_index: 0,
            effect_id: id(effect_ordinal),
            plan: ToolCallPlan::Execute(validated),
        },
        status,
    }
}

fn state_with(calls: Vec<ActiveToolCall>, settled: &[u64]) -> KernelState {
    let assigned = calls
        .iter()
        .map(|call| call.assigned.clone())
        .collect::<Vec<_>>();
    let mut state = KernelState {
        active_tool_batch: Some(ActiveToolBatch::new(
            ToolBatchOpened {
                cycle: 0,
                turn_id: id(7),
                tool_batch_id: id(8),
                source_message_id: id(9),
                calls: assigned.into(),
                continuation: ToolBatchContinuation::ContinueModel,
                plan_digest: Digest::raw_json(b"tool-batch-plan"),
            },
            calls,
            0,
            0,
            Arc::from([]),
            None,
        )),
        ..KernelState::default()
    };
    for ordinal in settled {
        state.tool_settlements.insert(
            id(*ordinal),
            ToolSettlementFingerprint {
                kind: ToolSettlementKind::Completed,
                digest: Digest::raw_json(b"settled"),
            },
        );
    }
    state
}

fn spec(side_effect: SideEffectClass, retry_safety: RetrySafety) -> ToolSpec {
    ToolSpec {
        id: ToolId::parse("finstack.tools.echo").expect("tool id"),
        model_name: Arc::from("echo"),
        title: Arc::from("echo"),
        description: Arc::from("scripted"),
        input_schema: RawJson::parse(b"{}").expect("schema"),
        output_schema: None,
        execution: ToolExecutionMode::Parallel,
        side_effect,
        retry_safety,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 1_024,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    }
}

#[tokio::test]
async fn tool_stream_rejects_undeclared_deferral() {
    let error = ToolStreamAssembler::default()
        .assemble(
            tool_stream(vec![Ok(ToolStreamItem::Deferred(tool_deferral(
                None, None,
            )))]),
            None,
            1_024,
            ToolDeferralSupport::Never,
        )
        .await
        .expect_err("undeclared deferral");

    assert_eq!(error.code(), TOOL_DEFERRAL_NOT_DECLARED);
}

#[tokio::test]
async fn tool_stream_rejects_deferral_with_poll_after_expiry() {
    let error = ToolStreamAssembler::default()
        .assemble(
            tool_stream(vec![Ok(ToolStreamItem::Deferred(tool_deferral(
                Some(Timestamp::from_unix_ms(2).expect("next poll")),
                Some(Timestamp::from_unix_ms(1).expect("expiry")),
            )))]),
            None,
            1_024,
            ToolDeferralSupport::Supported,
        )
        .await
        .expect_err("invalid deferral");

    assert_eq!(error.code(), TOOL_DEFERRAL_INVALID);
}

#[tokio::test]
async fn tool_stream_rejects_items_after_deferral() {
    let error = ToolStreamAssembler::new(ToolStreamLimits {
        max_items: 1,
        max_stream_bytes: 1_024,
    })
    .assemble(
        tool_stream(vec![
            Ok(ToolStreamItem::Deferred(tool_deferral(None, None))),
            Ok(ToolStreamItem::Completed(ToolResult {
                output: RawJson::parse(br#"{"ok":true}"#).expect("output"),
                is_error: false,
            })),
        ]),
        None,
        1_024,
        ToolDeferralSupport::Supported,
    )
    .await
    .expect_err("post-terminal item");

    assert_eq!(error.code(), TOOL_STREAM_INVALID);
}

#[tokio::test]
async fn tool_stream_accepts_declared_deferral() {
    let deferral = tool_deferral(None, None);
    let assembled = ToolStreamAssembler::default()
        .assemble(
            tool_stream(vec![Ok(ToolStreamItem::Deferred(deferral.clone()))]),
            None,
            1_024,
            ToolDeferralSupport::Supported,
        )
        .await
        .expect("declared deferral");

    assert_eq!(assembled.terminal, ToolTerminal::Deferred(deferral));
}

#[tokio::test]
async fn tool_stream_preserves_unique_artifact_references() {
    let first = artifact(1);
    let second = artifact(2);
    let assembled = ToolStreamAssembler::default()
        .assemble(
            tool_stream(vec![
                Ok(ToolStreamItem::Artifact(first.clone())),
                Ok(ToolStreamItem::Artifact(first.clone())),
                Ok(ToolStreamItem::Artifact(second.clone())),
                Ok(ToolStreamItem::Completed(ToolResult {
                    output: RawJson::parse(br#"{"ok":true}"#).expect("output"),
                    is_error: false,
                })),
            ]),
            None,
            1_024,
            ToolDeferralSupport::Never,
        )
        .await
        .expect("assembled stream");

    assert_eq!(assembled.artifacts.as_ref(), &[first, second]);
}

#[tokio::test]
async fn tool_stream_rejects_too_many_unique_artifacts() {
    let mut items = (0..=u8::try_from(MAX_TOOL_RESULT_ARTIFACTS).expect("bounded"))
        .map(|ordinal| Ok(ToolStreamItem::Artifact(artifact(ordinal))))
        .collect::<Vec<_>>();
    items.push(Ok(ToolStreamItem::Completed(ToolResult {
        output: RawJson::parse(br#"{"ok":true}"#).expect("output"),
        is_error: false,
    })));
    let error = ToolStreamAssembler::default()
        .assemble(tool_stream(items), None, 1_024, ToolDeferralSupport::Never)
        .await
        .expect_err("artifact limit");

    assert_eq!(error.code(), TOOL_STREAM_LIMIT_EXCEEDED);
}

#[test]
fn reserved_tool_deferral_codes_enforce_category_and_retryability() {
    let error = ToolError::try_new(
        TOOL_DEFERRAL_NOT_DECLARED,
        ErrorCategory::Deadline,
        false,
        "wrong category",
        Metadata::empty(),
    )
    .expect_err("reserved category");
    assert_eq!(ToolError::from(error).code(), TOOL_REGISTRATION_INVALID);

    let error = ToolError::try_new(
        TOOL_DEFERRAL_INVALID,
        ErrorCategory::Validation,
        true,
        "wrong retryability",
        Metadata::empty(),
    )
    .expect_err("reserved retryability");
    assert_eq!(ToolError::from(error).code(), TOOL_REGISTRATION_INVALID);

    let not_declared = ToolError::try_new(
        TOOL_DEFERRAL_NOT_DECLARED,
        ErrorCategory::Validation,
        false,
        "bounded",
        Metadata::empty(),
    )
    .expect("reserved classification");
    assert_eq!(not_declared.category(), ErrorCategory::Validation);
    assert!(!not_declared.retryable());

    let invalid = ToolError::try_new(
        TOOL_DEFERRAL_INVALID,
        ErrorCategory::Validation,
        false,
        "bounded",
        Metadata::empty(),
    )
    .expect("reserved classification");
    assert_eq!(invalid.category(), ErrorCategory::Validation);
    assert!(!invalid.retryable());

    let expired = ToolError::try_new(
        TOOL_DEFERRAL_EXPIRED,
        ErrorCategory::Deadline,
        false,
        "bounded",
        Metadata::empty(),
    )
    .expect("reserved classification");
    assert_eq!(expired.category(), ErrorCategory::Deadline);
    assert!(!expired.retryable());
}

#[test]
fn tool_spec_defaults_to_never_deferred_and_validates() {
    assert_eq!(ToolDeferralSupport::default(), ToolDeferralSupport::Never);
    let spec = spec(SideEffectClass::ReadOnly, RetrySafety::SafeToRetry);
    let mut serialized = serde_json::to_value(spec).expect("serialize tool spec");
    serialized
        .as_object_mut()
        .expect("tool spec object")
        .remove("deferral");
    let deserialized: ToolSpec =
        serde_json::from_value(serialized).expect("deserialize legacy tool spec");

    assert_eq!(deserialized.deferral, ToolDeferralSupport::Never);
    assert!(deserialized.validate().is_ok());
}

#[test]
fn tool_resume_action_classifies_journal_only_states() {
    let effect = id(4);
    assert_eq!(
        tool_resume_action(&KernelState::default(), effect),
        ToolResumeAction::NoOutstanding
    );
    assert_eq!(
        tool_resume_action(
            &state_with(
                vec![execute_call(
                    0,
                    4,
                    ActiveToolCallStatus::Requested {
                        requested: requested(4, RetrySafety::SafeToRetry),
                        deferred: None,
                    },
                )],
                &[],
            ),
            effect,
        ),
        ToolResumeAction::Reconcile
    );
    assert_eq!(
        tool_resume_action(
            &state_with(
                vec![execute_call(
                    0,
                    4,
                    ActiveToolCallStatus::Requested {
                        requested: requested(4, RetrySafety::SafeToRetry),
                        deferred: None,
                    },
                )],
                &[4],
            ),
            effect,
        ),
        ToolResumeAction::UseRecorded
    );
    assert_eq!(
        tool_resume_action(
            &state_with(
                vec![execute_call(
                    0,
                    4,
                    ActiveToolCallStatus::Requested {
                        requested: requested(4, RetrySafety::SafeToRetry),
                        deferred: Some(deferred(4, ReconciliationPolicy::CallbackOnly)),
                    },
                )],
                &[],
            ),
            effect,
        ),
        ToolResumeAction::WaitExternal
    );
    assert_eq!(
        tool_resume_action(
            &state_with(
                vec![execute_call(
                    0,
                    4,
                    ActiveToolCallStatus::Requested {
                        requested: requested(4, RetrySafety::SafeToRetry),
                        deferred: Some(deferred(4, ReconciliationPolicy::Poll)),
                    },
                )],
                &[],
            ),
            effect,
        ),
        ToolResumeAction::Reconcile
    );
    assert_eq!(
        tool_resume_action(
            &state_with(
                vec![execute_call(0, 4, ActiveToolCallStatus::Undispatched)],
                &[],
            ),
            effect,
        ),
        ToolResumeAction::NoOutstanding
    );
    assert_ne!(
        tool_resume_action(
            &state_with(
                vec![execute_call(
                    0,
                    4,
                    ActiveToolCallStatus::Requested {
                        requested: requested(4, RetrySafety::SafeToRetry),
                        deferred: None,
                    },
                )],
                &[],
            ),
            effect,
        ),
        ToolResumeAction::Retry
    );
}

#[test]
fn tool_resume_action_keeps_completed_subset_and_deferred_sibling_independent() {
    let state = state_with(
        vec![
            execute_call(
                0,
                4,
                ActiveToolCallStatus::Settled {
                    result_message_id: id(20),
                    settlement_digest: Digest::raw_json(b"settled"),
                },
            ),
            execute_call(
                1,
                5,
                ActiveToolCallStatus::Requested {
                    requested: requested(5, RetrySafety::SafeToRetry),
                    deferred: None,
                },
            ),
            execute_call(
                2,
                6,
                ActiveToolCallStatus::Requested {
                    requested: requested(6, RetrySafety::SafeToRetry),
                    deferred: Some(deferred(6, ReconciliationPolicy::CallbackOnly)),
                },
            ),
        ],
        &[4],
    );
    assert_eq!(
        tool_resume_action(&state, id(4)),
        ToolResumeAction::UseRecorded
    );
    assert_eq!(
        tool_resume_action(&state, id(5)),
        ToolResumeAction::Reconcile
    );
    assert_eq!(
        tool_resume_action(&state, id(6)),
        ToolResumeAction::WaitExternal
    );
}

#[test]
fn tool_resume_maps_reconcile_results_to_documented_actions() {
    let direct = state_with(
        vec![execute_call(
            0,
            4,
            ActiveToolCallStatus::Requested {
                requested: requested(4, RetrySafety::SafeToRetry),
                deferred: None,
            },
        )],
        &[],
    );
    let deferred_state = state_with(
        vec![execute_call(
            0,
            4,
            ActiveToolCallStatus::Requested {
                requested: requested(4, RetrySafety::SafeToRetry),
                deferred: Some(deferred(4, ReconciliationPolicy::CallbackOrPoll)),
            },
        )],
        &[],
    );
    let completed = ToolReconcileResult::Completed(ToolResult {
        output: RawJson::parse(br#"{"ok":true}"#).expect("output"),
        is_error: false,
    });
    assert_eq!(
        map_tool_reconcile_result(&direct, id(4), &completed, true),
        ToolResumeAction::UseRecorded
    );
    assert_eq!(
        map_tool_reconcile_result(
            &direct,
            id(4),
            &ToolReconcileResult::StillRunning(ToolDeferral {
                handle: deferred(4, ReconciliationPolicy::CallbackOrPoll).handle,
                reconciliation: ReconciliationPolicy::CallbackOrPoll,
                next_poll_at: None,
                expires_at: None,
            }),
            true,
        ),
        ToolResumeAction::WaitExternal
    );
    assert_eq!(
        map_tool_reconcile_result(&direct, id(4), &ToolReconcileResult::NotStarted, true),
        ToolResumeAction::Retry
    );
    assert_eq!(
        map_tool_reconcile_result(&direct, id(4), &ToolReconcileResult::Unknown, true),
        ToolResumeAction::Retry
    );
    assert_eq!(
        map_tool_reconcile_result(&direct, id(4), &ToolReconcileResult::Unknown, false),
        ToolResumeAction::SuspendUncertain
    );
    assert_eq!(
        map_tool_reconcile_result(&direct, id(4), &ToolReconcileResult::NonRepeatable, true),
        ToolResumeAction::SuspendUncertain
    );
    assert_eq!(
        map_tool_reconcile_result(
            &deferred_state,
            id(4),
            &ToolReconcileResult::NotStarted,
            true
        ),
        ToolResumeAction::SuspendUncertain
    );
    assert!(!tool_retry_allowed(
        &requested(4, RetrySafety::AtMostOnce),
        &spec(SideEffectClass::ReadOnly, RetrySafety::AtMostOnce),
    ));
    assert!(!tool_retry_allowed(
        &requested(4, RetrySafety::SafeToRetry),
        &spec(
            SideEffectClass::NonIdempotentWrite,
            RetrySafety::SafeToRetry
        ),
    ));
    assert!(tool_retry_allowed(
        &requested(4, RetrySafety::IdempotentWithKey),
        &spec(
            SideEffectClass::IdempotentWrite,
            RetrySafety::IdempotentWithKey
        ),
    ));
}

#[test]
fn synthetic_closure_is_recorded_and_never_reconciled() {
    let error = ErrorDescriptor::new("unknown_tool", "unknown", ErrorCategory::Validation, false)
        .expect("error");
    let call = ActiveToolCall {
        assigned: AssignedToolCall {
            source_index: 0,
            group_index: 0,
            effect_id: id(4),
            plan: ToolCallPlan::SyntheticClosure(SyntheticToolClosure {
                call: tool_call(),
                execution: ToolExecutionMode::Parallel,
                failure_policy: ToolFailurePolicy::ReturnToModel,
                error,
            }),
        },
        status: ActiveToolCallStatus::Undispatched,
    };
    assert_eq!(
        tool_resume_action(&state_with(vec![call], &[]), id(4)),
        ToolResumeAction::UseRecorded
    );
}

fn tool_ctx(principal_tenant: Option<&str>, locator_tenant: &str) -> ToolCallContext {
    ToolCallContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(locator_tenant, id(1), id(2), id(3))
                .expect("locator"),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", principal_tenant)
                    .expect("principal"),
                authentication_method: Arc::from("fixture"),
                assurance_level: Arc::from("high"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from(locator_tenant)]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("v1"),
                decision_id: Arc::from("decision-1"),
            },
            effect_id: id(4),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
            relation_depth: 0,
        },
        tool_batch_id: id(5),
        tool_call_id: id(6),
    }
}

#[test]
fn verify_authority_accepts_matching_or_unscoped_principal() {
    assert!(verify_authority(&tool_ctx(Some("tenant-a"), "tenant-a")).is_ok());
    assert!(verify_authority(&tool_ctx(None, "tenant-a")).is_ok());
}

#[test]
fn verify_authority_rejects_mismatched_principal_scope() {
    let error = verify_authority(&tool_ctx(Some("tenant-b"), "tenant-a")).expect_err("denied");
    assert_eq!(error.code(), TOOL_POLICY_DENIED);
    assert_eq!(
        error.message(),
        "principal scope does not match the committed effect"
    );
}
