//! Compiler-exhaustive drift guard for the candidate-v1 semantic reference.

use finstack_ai_kernel::{
    EffectInput, KernelInput, PostCommitAction, RecordBody, RunEventBody, RunPhase,
};

const REFERENCE: &str =
    include_str!("../../../docs/implementation/kernel-semantics-candidate-v1.md");

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

#[test]
fn reference_contains_every_stable_vocabulary_and_invariant() {
    for name in PHASE_NAMES
        .iter()
        .chain(INPUT_NAMES)
        .chain(RECORD_NAMES)
        .chain(EVENT_NAMES)
        .chain(EFFECT_NAMES)
        .chain(ACTION_NAMES)
    {
        assert!(
            REFERENCE.contains(name),
            "semantic reference missing {name}"
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
