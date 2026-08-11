//! Compiler-exhaustive drift guard for the candidate-v1 semantic reference.

use finstack_ai_kernel::{
    EffectInput, KernelInput, PostCommitAction, RecordBody, RunEventBody, RunPhase,
};

const REFERENCE: &str =
    include_str!("../../../docs/implementation/kernel-semantics-candidate-v1.md");

#[test]
fn reference_contains_every_stable_vocabulary_and_invariant() {
    let required = [
        "Accepted",
        "BeforeRun",
        "PreparingContext",
        "BeforeModel",
        "AwaitingModel",
        "AfterModel",
        "BeforeToolBatch",
        "AwaitingTools",
        "AfterToolBatch",
        "BeforeFinalize",
        "AwaitingInteraction",
        "AwaitingExternal",
        "Sleeping",
        "Cancelling",
        "Suspended",
        "Completed",
        "Failed",
        "Cancelled",
        "AcceptRun",
        "StageSettled",
        "ModelSettled",
        "ExternalEffectCompleted",
        "ToolBatchSettled",
        "CancelRequested",
        "CancellationReconciled",
        "TimerFired",
        "ConfigureOutput",
        "CapabilitiesActivated",
        "OutputValidated",
        "RunAccepted",
        "EffectRequested",
        "EffectDeferred",
        "EffectCompleted",
        "EffectFailed",
        "EffectCancelled",
        "InteractionRequested",
        "InteractionResolved",
        "InteractionExpired",
        "InteractionCancelled",
        "StageOutcomeRecorded",
        "ContextPrepared",
        "EntryAppended",
        "ToolBatchOpened",
        "ToolCallSettled",
        "ToolBatchClosed",
        "CancellationRequested",
        "LimitReached",
        "RetryScheduled",
        "RunSuspended",
        "RunCompleted",
        "RunFailed",
        "RunCancelled",
        "OutputConfigured",
        "FinalResultRecorded",
        "OutputValidationFailed",
        "MessageFinalized",
        "ToolSettled",
        "ModelTextDelta",
        "ReasoningDelta",
        "ToolProgress",
        "QueueDepthWarning",
        "ProviderHeartbeat",
        "Model",
        "Tool",
        "Context",
        "Middleware",
        "Interaction",
        "Timer",
        "ExecuteEffect",
        "CancelEffect",
    ];
    for entry in required {
        assert!(
            REFERENCE.contains(entry),
            "semantic reference missing {entry}"
        );
    }
    for ordinal in 1..=12 {
        assert!(
            REFERENCE.contains(&format!("INV-{ordinal:03}")),
            "semantic reference missing INV-{ordinal:03}"
        );
    }
    for fixture_family in ["golden-trace/v1", "public-rust-api/v1", "conformance/v1"] {
        assert!(
            REFERENCE.contains(fixture_family),
            "missing fixture inventory {fixture_family}"
        );
    }
}

#[allow(dead_code)]
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

#[allow(dead_code)]
fn input_name(value: &KernelInput) -> &'static str {
    match value {
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
    }
}

#[allow(dead_code)]
fn record_name(value: &RecordBody) -> &'static str {
    match value {
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
    }
}

#[allow(dead_code)]
fn event_name(value: &RunEventBody) -> &'static str {
    match value {
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
}

#[allow(dead_code)]
fn effect_name(value: &EffectInput) -> &'static str {
    match value {
        EffectInput::Model { .. } => "Model",
        EffectInput::Tool { .. } => "Tool",
        EffectInput::Context { .. } => "Context",
        EffectInput::Middleware { .. } => "Middleware",
        EffectInput::Interaction { .. } => "Interaction",
        EffectInput::Timer { .. } => "Timer",
    }
}

#[allow(dead_code)]
const fn action_name(value: PostCommitAction) -> &'static str {
    match value {
        PostCommitAction::ExecuteEffect { .. } => "ExecuteEffect",
        PostCommitAction::CancelEffect { .. } => "CancelEffect",
    }
}
