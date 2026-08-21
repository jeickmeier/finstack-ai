//! Compiler-exhaustive drift guard for the candidate-v1 semantic reference.

use finstack_ai_kernel::{
    CancellationInitiator, CapabilityActivationSource, ChildPlacement, EffectInput, EffectKind,
    EffectOutputKind, EffectPurpose, ExternalEffectOutcome, InteractionSettled, InvocationRecovery,
    JsonSchemaDraft, KernelInput, LimitDimension, ModelSettlement, NestedModelKind,
    OutputEndStrategy, OutputSpec, PostCommitAction, RecordBody, ReducerStageOutcome,
    RetryClassification, RetrySafety, RunEventBody, RunPhase, Stage, StageDisposition,
    StructuredResultSource, ToolBatchContinuation, ToolBatchOutcome, ToolCallPlan,
    ToolExecutionMode, ToolFailurePolicy, ToolSettlement, ValidationOutcome,
};

const REFERENCE: &str = include_str!("fixtures/candidate_v1_nested_vocabulary.md");

macro_rules! named_variants {
    ($fn:ident, $all:ident, $ty:ty, $($pat:pat => $label:literal),+ $(,)?) => {
        fn $fn(value: &$ty) -> &'static str {
            match value {
                $($pat => $label,)+
            }
        }
        const $all: &[&str] = &[$($label),+];
    };
}

macro_rules! documented_wire_variants {
    (
        $test:ident, $ty:ty, $type_name:literal, $serde_form:literal, $unknown_probe:literal,
        $($pat:pat => $variant:literal => $wire:literal),+ $(,)?
    ) => {
        #[test]
        fn $test() {
            fn vocabulary(value: &$ty) -> (&'static str, &'static str) {
                match value {
                    $($pat => ($variant, $wire),)+
                }
            }

            let _ = vocabulary as fn(&$ty) -> (&'static str, &'static str);
            let mappings = [$(format!(
                "`{}::{}` → `{}`",
                $type_name,
                $variant,
                $wire,
            )),+]
            .join("; ");
            let expected_row = format!(
                "| `{}` | {} | {} |",
                $type_name,
                $serde_form,
                mappings,
            );
            assert_eq!(
                REFERENCE
                    .lines()
                    .filter(|line| *line == expected_row)
                    .count(),
                1,
                "semantic reference must contain exactly one row: {expected_row}",
            );

            let error = match serde_json::from_str::<$ty>($unknown_probe) {
                Ok(_) => panic!(
                    "{} unexpectedly accepted the reserved unknown wire variant",
                    $type_name,
                ),
                Err(error) => error.to_string(),
            };
            assert!(
                error.contains("unknown variant `__unknown__`"),
                "{} no longer uses the documented serde form: {error}",
                $type_name,
            );
            for wire in [$($wire),+] {
                assert!(
                    error.contains(wire),
                    "{} serde vocabulary omitted `{wire}`: {error}",
                    $type_name,
                );
            }
        }
    };
}

documented_wire_variants! {
    stage_vocabulary, Stage, "Stage", "scalar `snake_case`", r#""__unknown__""#,
    Stage::BeforeRun => "BeforeRun" => "before_run",
    Stage::PrepareContext => "PrepareContext" => "prepare_context",
    Stage::BeforeModel => "BeforeModel" => "before_model",
    Stage::AfterModel => "AfterModel" => "after_model",
    Stage::BeforeToolBatch => "BeforeToolBatch" => "before_tool_batch",
    Stage::AfterToolBatch => "AfterToolBatch" => "after_tool_batch",
    Stage::BeforeFinalize => "BeforeFinalize" => "before_finalize",
}

documented_wire_variants! {
    reducer_stage_outcome_vocabulary, ReducerStageOutcome, "ReducerStageOutcome", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    ReducerStageOutcome::Continue => "Continue" => "continue",
    ReducerStageOutcome::ContextPrepared { .. } => "ContextPrepared" => "context_prepared",
    ReducerStageOutcome::ModelRequestPrepared { .. } => "ModelRequestPrepared" => "model_request_prepared",
    ReducerStageOutcome::ToolBatchPrepared { .. } => "ToolBatchPrepared" => "tool_batch_prepared",
    ReducerStageOutcome::FinalizeAccepted => "FinalizeAccepted" => "finalize_accepted",
    ReducerStageOutcome::ContinueModel { .. } => "ContinueModel" => "continue_model",
    ReducerStageOutcome::Retry(_) => "Retry" => "retry",
    ReducerStageOutcome::Fail(_) => "Fail" => "fail",
}

documented_wire_variants! {
    tool_settlement_vocabulary, ToolSettlement, "ToolSettlement", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    ToolSettlement::Completed(_) => "Completed" => "completed",
    ToolSettlement::Deferred(_) => "Deferred" => "deferred",
    ToolSettlement::Failed(_) => "Failed" => "failed",
}

documented_wire_variants! {
    model_settlement_vocabulary, ModelSettlement, "ModelSettlement", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    ModelSettlement::Completed { .. } => "Completed" => "completed",
    ModelSettlement::Deferred(_) => "Deferred" => "deferred",
    ModelSettlement::Failed(_) => "Failed" => "failed",
}

documented_wire_variants! {
    external_effect_outcome_vocabulary, ExternalEffectOutcome, "ExternalEffectOutcome", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    ExternalEffectOutcome::Completed { .. } => "Completed" => "completed",
    ExternalEffectOutcome::Failed { .. } => "Failed" => "failed",
}

documented_wire_variants! {
    interaction_settled_vocabulary, InteractionSettled, "InteractionSettled", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    InteractionSettled::Resolved(_) => "Resolved" => "resolved",
    InteractionSettled::Expired(_) => "Expired" => "expired",
    InteractionSettled::Cancelled(_) => "Cancelled" => "cancelled",
}

documented_wire_variants! {
    cancellation_initiator_vocabulary, CancellationInitiator, "CancellationInitiator", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    CancellationInitiator::Principal { .. } => "Principal" => "principal",
    CancellationInitiator::ParentRun { .. } => "ParentRun" => "parent_run",
    CancellationInitiator::Deadline => "Deadline" => "deadline",
    CancellationInitiator::RuntimeShutdown => "RuntimeShutdown" => "runtime_shutdown",
}

documented_wire_variants! {
    json_schema_draft_vocabulary, JsonSchemaDraft, "JsonSchemaDraft", "scalar `snake_case`", r#""__unknown__""#,
    JsonSchemaDraft::Draft202012 => "Draft202012" => "draft202012",
}

documented_wire_variants! {
    output_spec_vocabulary, OutputSpec, "OutputSpec", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    OutputSpec::PlainText => "PlainText" => "plain_text",
    OutputSpec::JsonSchema { .. } => "JsonSchema" => "json_schema",
}

documented_wire_variants! {
    output_end_strategy_vocabulary, OutputEndStrategy, "OutputEndStrategy", "scalar `snake_case`", r#""__unknown__""#,
    OutputEndStrategy::Early => "Early" => "early",
    OutputEndStrategy::Exhaustive => "Exhaustive" => "exhaustive",
}

documented_wire_variants! {
    validation_outcome_vocabulary, ValidationOutcome, "ValidationOutcome", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    ValidationOutcome::Valid => "Valid" => "valid",
    ValidationOutcome::Invalid { .. } => "Invalid" => "invalid",
}

documented_wire_variants! {
    stage_disposition_vocabulary, StageDisposition, "StageDisposition", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    StageDisposition::Continued => "Continued" => "continued",
    StageDisposition::ContextPrepared { .. } => "ContextPrepared" => "context_prepared",
    StageDisposition::ModelRequested { .. } => "ModelRequested" => "model_requested",
    StageDisposition::ToolBatchPrepared { .. } => "ToolBatchPrepared" => "tool_batch_prepared",
    StageDisposition::FinalizeAccepted => "FinalizeAccepted" => "finalize_accepted",
    StageDisposition::ContinueModel { .. } => "ContinueModel" => "continue_model",
    StageDisposition::RetryScheduled { .. } => "RetryScheduled" => "retry_scheduled",
    StageDisposition::Failed { .. } => "Failed" => "failed",
}

documented_wire_variants! {
    retry_classification_vocabulary, RetryClassification, "RetryClassification", "scalar `snake_case`", r#""__unknown__""#,
    RetryClassification::Model => "Model" => "model",
    RetryClassification::Tool => "Tool" => "tool",
    RetryClassification::Validation => "Validation" => "validation",
    RetryClassification::Framework => "Framework" => "framework",
}

documented_wire_variants! {
    child_placement_vocabulary, ChildPlacement, "ChildPlacement", "scalar `snake_case`", r#""__unknown__""#,
    ChildPlacement::CompatibleLaneInParentSession => "CompatibleLaneInParentSession" => "compatible_lane_in_parent_session",
    ChildPlacement::IsolatedChildSession => "IsolatedChildSession" => "isolated_child_session",
    ChildPlacement::RemoteChildSession => "RemoteChildSession" => "remote_child_session",
}

documented_wire_variants! {
    tool_execution_mode_vocabulary, ToolExecutionMode, "ToolExecutionMode", "scalar `snake_case`", r#""__unknown__""#,
    ToolExecutionMode::Parallel => "Parallel" => "parallel",
    ToolExecutionMode::Sequential => "Sequential" => "sequential",
    ToolExecutionMode::Barrier => "Barrier" => "barrier",
}

documented_wire_variants! {
    tool_failure_policy_vocabulary, ToolFailurePolicy, "ToolFailurePolicy", "scalar `snake_case`", r#""__unknown__""#,
    ToolFailurePolicy::ReturnToModel => "ReturnToModel" => "return_to_model",
    ToolFailurePolicy::FailRun => "FailRun" => "fail_run",
}

documented_wire_variants! {
    tool_batch_continuation_vocabulary, ToolBatchContinuation, "ToolBatchContinuation", "scalar `snake_case`", r#""__unknown__""#,
    ToolBatchContinuation::ContinueModel => "ContinueModel" => "continue_model",
    ToolBatchContinuation::Finalize => "Finalize" => "finalize",
}

documented_wire_variants! {
    tool_call_plan_vocabulary, ToolCallPlan, "ToolCallPlan", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    ToolCallPlan::Execute(_) => "Execute" => "execute",
    ToolCallPlan::SyntheticClosure(_) => "SyntheticClosure" => "synthetic_closure",
}

documented_wire_variants! {
    tool_batch_outcome_vocabulary, ToolBatchOutcome, "ToolBatchOutcome", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    ToolBatchOutcome::ContinueModel => "ContinueModel" => "continue_model",
    ToolBatchOutcome::Finalize => "Finalize" => "finalize",
    ToolBatchOutcome::Failed { .. } => "Failed" => "failed",
}

documented_wire_variants! {
    effect_kind_vocabulary, EffectKind, "EffectKind", "scalar `snake_case`", r#""__unknown__""#,
    EffectKind::Model => "Model" => "model",
    EffectKind::Tool => "Tool" => "tool",
    EffectKind::Context => "Context" => "context",
    EffectKind::Middleware => "Middleware" => "middleware",
    EffectKind::Interaction => "Interaction" => "interaction",
    EffectKind::Timer => "Timer" => "timer",
}

documented_wire_variants! {
    retry_safety_vocabulary, RetrySafety, "RetrySafety", "scalar `snake_case`", r#""__unknown__""#,
    RetrySafety::SafeToRetry => "SafeToRetry" => "safe_to_retry",
    RetrySafety::IdempotentWithKey => "IdempotentWithKey" => "idempotent_with_key",
    RetrySafety::AtMostOnce => "AtMostOnce" => "at_most_once",
    RetrySafety::Unknown => "Unknown" => "unknown",
}

documented_wire_variants! {
    effect_output_kind_vocabulary, EffectOutputKind, "EffectOutputKind", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    EffectOutputKind::ModelResponse => "ModelResponse" => "model_response",
    EffectOutputKind::ToolResult => "ToolResult" => "tool_result",
    EffectOutputKind::ContextContribution => "ContextContribution" => "context_contribution",
    EffectOutputKind::MiddlewareOutcome => "MiddlewareOutcome" => "middleware_outcome",
    EffectOutputKind::InteractionResolution => "InteractionResolution" => "interaction_resolution",
    EffectOutputKind::TimerFiring => "TimerFiring" => "timer_firing",
    EffectOutputKind::ArtifactReceipt => "ArtifactReceipt" => "artifact_receipt",
    EffectOutputKind::Custom { .. } => "Custom" => "custom",
}

documented_wire_variants! {
    invocation_recovery_vocabulary, InvocationRecovery, "InvocationRecovery", "scalar `snake_case`", r#""__unknown__""#,
    InvocationRecovery::RecomputeSafe => "RecomputeSafe" => "recompute_safe",
    InvocationRecovery::Reconcile => "Reconcile" => "reconcile",
    InvocationRecovery::NonRepeatable => "NonRepeatable" => "non_repeatable",
}

documented_wire_variants! {
    effect_purpose_vocabulary, EffectPurpose, "EffectPurpose", "internal `kind` tag", r#"{"kind":"__unknown__"}"#,
    EffectPurpose::CompactionSummary { .. } => "CompactionSummary" => "compaction_summary",
    EffectPurpose::NestedModel { .. } => "NestedModel" => "nested_model",
}

documented_wire_variants! {
    nested_model_kind_vocabulary, NestedModelKind, "NestedModelKind", "scalar `snake_case`", r#""__unknown__""#,
    NestedModelKind::McpSampling => "McpSampling" => "mcp_sampling",
}

documented_wire_variants! {
    limit_dimension_vocabulary, LimitDimension, "LimitDimension", "internal `kind` tag", r#"{"kind":"__unknown__"}"#,
    LimitDimension::ModelRequests => "ModelRequests" => "model_requests",
    LimitDimension::Turns => "Turns" => "turns",
    LimitDimension::ToolCalls => "ToolCalls" => "tool_calls",
    LimitDimension::ParallelTools => "ParallelTools" => "parallel_tools",
    LimitDimension::InputTokens => "InputTokens" => "input_tokens",
    LimitDimension::OutputTokens => "OutputTokens" => "output_tokens",
    LimitDimension::ContextBytes => "ContextBytes" => "context_bytes",
    LimitDimension::OutputBytes => "OutputBytes" => "output_bytes",
    LimitDimension::Retries => "Retries" => "retries",
    LimitDimension::WallTime => "WallTime" => "wall_time",
    LimitDimension::Cost => "Cost" => "cost",
    LimitDimension::Extension { .. } => "Extension" => "extension",
}

documented_wire_variants! {
    capability_activation_source_vocabulary, CapabilityActivationSource, "CapabilityActivationSource", "scalar `snake_case`", r#""__unknown__""#,
    CapabilityActivationSource::Always => "Always" => "always",
    CapabilityActivationSource::Application => "Application" => "application",
    CapabilityActivationSource::Model => "Model" => "model",
}

documented_wire_variants! {
    structured_result_source_vocabulary, StructuredResultSource, "StructuredResultSource", "external `snake_case` tag", r#"{"__unknown__":null}"#,
    StructuredResultSource::JsonBlock { .. } => "JsonBlock" => "json_block",
    StructuredResultSource::InternalTool { .. } => "InternalTool" => "internal_tool",
}

#[test]
fn top_level_vocabularies_remain_compiler_exhaustive() {
    assert_eq!(PHASE_NAMES.len(), 18);
    assert_eq!(INPUT_NAMES.len(), 17);
    assert_eq!(RECORD_NAMES.len(), 40);
    assert_eq!(EVENT_NAMES.len(), 22);
    assert_eq!(EFFECT_NAMES.len(), 6);
    assert_eq!(ACTION_NAMES.len(), 2);
    let _ = (
        phase_name as fn(RunPhase) -> &'static str,
        input_name as fn(&KernelInput) -> &'static str,
        record_name as fn(&RecordBody) -> &'static str,
        event_name as fn(&RunEventBody) -> &'static str,
        effect_name as fn(&EffectInput) -> &'static str,
        action_name as fn(&PostCommitAction) -> &'static str,
    );
}

const fn phase_name(value: RunPhase) -> &'static str {
    match value {
        RunPhase::Accepted => "Accepted",
        RunPhase::BeforeRun => "BeforeRun",
        RunPhase::PreparingContext => "PreparingContext",
        RunPhase::BeforeModel => "BeforeModel",
        RunPhase::AwaitingModel => "AwaitingModel",
        RunPhase::AfterModel => "AfterModel",
        RunPhase::BeforeToolBatch => "BeforeToolBatch",
        RunPhase::AwaitingTools => "AwaitingTools",
        RunPhase::AfterToolBatch => "AfterToolBatch",
        RunPhase::BeforeFinalize => "BeforeFinalize",
        RunPhase::AwaitingInteraction => "AwaitingInteraction",
        RunPhase::AwaitingExternal => "AwaitingExternal",
        RunPhase::Sleeping => "Sleeping",
        RunPhase::Cancelling => "Cancelling",
        RunPhase::Suspended => "Suspended",
        RunPhase::Completed => "Completed",
        RunPhase::Failed => "Failed",
        RunPhase::Cancelled => "Cancelled",
    }
}

const PHASE_NAMES: &[&str] = &[
    phase_name(RunPhase::Accepted),
    phase_name(RunPhase::BeforeRun),
    phase_name(RunPhase::PreparingContext),
    phase_name(RunPhase::BeforeModel),
    phase_name(RunPhase::AwaitingModel),
    phase_name(RunPhase::AfterModel),
    phase_name(RunPhase::BeforeToolBatch),
    phase_name(RunPhase::AwaitingTools),
    phase_name(RunPhase::AfterToolBatch),
    phase_name(RunPhase::BeforeFinalize),
    phase_name(RunPhase::AwaitingInteraction),
    phase_name(RunPhase::AwaitingExternal),
    phase_name(RunPhase::Sleeping),
    phase_name(RunPhase::Cancelling),
    phase_name(RunPhase::Suspended),
    phase_name(RunPhase::Completed),
    phase_name(RunPhase::Failed),
    phase_name(RunPhase::Cancelled),
];

named_variants! {
    input_name, INPUT_NAMES, KernelInput,
    KernelInput::AcceptRun(_) => "AcceptRun",
    KernelInput::StageSettled(_) => "StageSettled",
    KernelInput::ModelSettled(_) => "ModelSettled",
    KernelInput::ExternalEffectCompleted(_) => "ExternalEffectCompleted",
    KernelInput::ToolBatchSettled(_) => "ToolBatchSettled",
    KernelInput::CancelRequested(_) => "CancelRequested",
    KernelInput::CancellationReconciled(_) => "CancellationReconciled",
    KernelInput::TimerFired(_) => "TimerFired",
    KernelInput::ConfigureOutput(_) => "ConfigureOutput",
    KernelInput::CapabilitiesActivated(_) => "CapabilitiesActivated",
    KernelInput::OutputValidated(_) => "OutputValidated",
    KernelInput::RecordExternalCommandRejected(_) => "RecordExternalCommandRejected",
    KernelInput::RequestInteraction(_) => "RequestInteraction",
    KernelInput::InteractionSettled(_) => "InteractionSettled",
    KernelInput::RequestCompactionModel(_) => "RequestCompactionModel",
    KernelInput::RequestExtensionEffect(_) => "RequestExtensionEffect",
    KernelInput::ExtensionEffectSettled(_) => "ExtensionEffectSettled",
}

named_variants! {
    record_name, RECORD_NAMES, RecordBody,
    RecordBody::RunAccepted(_) => "RunAccepted",
    RecordBody::EffectRequested(_) => "EffectRequested",
    RecordBody::EffectDeferred(_) => "EffectDeferred",
    RecordBody::EffectCompleted(_) => "EffectCompleted",
    RecordBody::EffectFailed(_) => "EffectFailed",
    RecordBody::EffectCancelled(_) => "EffectCancelled",
    RecordBody::InteractionRequested(_) => "InteractionRequested",
    RecordBody::InteractionResolved(_) => "InteractionResolved",
    RecordBody::InteractionExpired(_) => "InteractionExpired",
    RecordBody::InteractionCancelled(_) => "InteractionCancelled",
    RecordBody::StageOutcomeRecorded(_) => "StageOutcomeRecorded",
    RecordBody::ContextPrepared(_) => "ContextPrepared",
    RecordBody::EntryAppended(_) => "EntryAppended",
    RecordBody::ToolBatchOpened(_) => "ToolBatchOpened",
    RecordBody::ToolCallSettled(_) => "ToolCallSettled",
    RecordBody::ToolBatchClosed(_) => "ToolBatchClosed",
    RecordBody::CancellationRequested(_) => "CancellationRequested",
    RecordBody::CancellationReconciled(_) => "CancellationReconciled",
    RecordBody::LimitReached(_) => "LimitReached",
    RecordBody::RetryScheduled(_) => "RetryScheduled",
    RecordBody::TimerFired(_) => "TimerFired",
    RecordBody::RunSuspended(_) => "RunSuspended",
    RecordBody::RunCompleted(_) => "RunCompleted",
    RecordBody::RunFailed(_) => "RunFailed",
    RecordBody::RunCancelled(_) => "RunCancelled",
    RecordBody::OutputConfigured(_) => "OutputConfigured",
    RecordBody::CapabilitiesActivated(_) => "CapabilitiesActivated",
    RecordBody::FinalResultRecorded(_) => "FinalResultRecorded",
    RecordBody::OutputValidationFailed(_) => "OutputValidationFailed",
    RecordBody::ExternalCommandRejected(_) => "ExternalCommandRejected",
    RecordBody::ChildRunPrepared(_) => "ChildRunPrepared",
    RecordBody::BudgetReservationRequested(_) => "BudgetReservationRequested",
    RecordBody::BudgetReservationSettled(_) => "BudgetReservationSettled",
    RecordBody::BudgetChargeRecorded(_) => "BudgetChargeRecorded",
    RecordBody::BudgetReservationReleased(_) => "BudgetReservationReleased",
    RecordBody::SessionCreated(_) => "SessionCreated",
    RecordBody::LaneCreated(_) => "LaneCreated",
    RecordBody::LaneMoved(_) => "LaneMoved",
    RecordBody::SnapshotWritten(_) => "SnapshotWritten",
    RecordBody::ConversationEntry(_) => "ConversationEntry",
}

named_variants! {
    event_name, EVENT_NAMES, RunEventBody,
    RunEventBody::RunAccepted(_) => "RunAccepted",
    RunEventBody::EffectRequested(_) => "EffectRequested",
    RunEventBody::EffectDeferred(_) => "EffectDeferred",
    RunEventBody::EffectCompleted(_) => "EffectCompleted",
    RunEventBody::EffectFailed(_) => "EffectFailed",
    RunEventBody::EffectCancelled(_) => "EffectCancelled",
    RunEventBody::InteractionRequested(_) => "InteractionRequested",
    RunEventBody::InteractionResolved(_) => "InteractionResolved",
    RunEventBody::InteractionExpired(_) => "InteractionExpired",
    RunEventBody::InteractionCancelled(_) => "InteractionCancelled",
    RunEventBody::MessageFinalized { .. } => "MessageFinalized",
    RunEventBody::ToolSettled { .. } => "ToolSettled",
    RunEventBody::LimitReached { .. } => "LimitReached",
    RunEventBody::RunSuspended { .. } => "RunSuspended",
    RunEventBody::RunCompleted { .. } => "RunCompleted",
    RunEventBody::RunFailed { .. } => "RunFailed",
    RunEventBody::RunCancelled { .. } => "RunCancelled",
    RunEventBody::ModelTextDelta(_) => "ModelTextDelta",
    RunEventBody::ReasoningDelta(_) => "ReasoningDelta",
    RunEventBody::ToolProgress(_) => "ToolProgress",
    RunEventBody::QueueDepthWarning(_) => "QueueDepthWarning",
    RunEventBody::ProviderHeartbeat(_) => "ProviderHeartbeat",
}

named_variants! {
    effect_name, EFFECT_NAMES, EffectInput,
    EffectInput::Model { .. } => "Model",
    EffectInput::Tool { .. } => "Tool",
    EffectInput::Context { .. } => "Context",
    EffectInput::Middleware { .. } => "Middleware",
    EffectInput::Interaction { .. } => "Interaction",
    EffectInput::Timer { .. } => "Timer",
}

named_variants! {
    action_name, ACTION_NAMES, PostCommitAction,
    PostCommitAction::ExecuteEffect { .. } => "ExecuteEffect",
    PostCommitAction::CancelEffect { .. } => "CancelEffect",
}
