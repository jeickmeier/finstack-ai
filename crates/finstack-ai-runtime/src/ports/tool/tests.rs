use std::sync::Arc;

use finstack_ai_kernel::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, AssignedToolCall, ComponentId, Digest,
    EffectDeferred, EffectInput, EffectKind, EffectOutputContract, EffectOutputKind,
    EffectRequested, ErrorCategory, ErrorDescriptor, ExternalHandleRef, Id, IdTag, KernelState,
    Metadata, RawJson, ReconciliationPolicy, RetrySafety, SyntheticToolClosure,
    ToolBatchContinuation, ToolBatchOpened, ToolCallBlock, ToolCallPlan, ToolExecutionMode,
    ToolFailurePolicy, ToolId, ToolSettlementFingerprint, ToolSettlementKind, ValidatedToolCall,
};

use crate::{ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolSpec};

use super::*;

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
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
        active_tool_batch: Some(ActiveToolBatch {
            opened: ToolBatchOpened {
                cycle: 0,
                turn_id: id(7),
                tool_batch_id: id(8),
                source_message_id: id(9),
                calls: assigned.into(),
                continuation: ToolBatchContinuation::ContinueModel,
                plan_digest: Digest::raw_json(b"tool-batch-plan"),
            },
            calls: calls.into(),
            current_group: 0,
            next_source_index: 0,
            result_message_ids: Arc::from([]),
            fatal_error: None,
        }),
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
    }
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
