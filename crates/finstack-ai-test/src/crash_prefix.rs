//! Shared crash-prefix restore classes for crash-recovery contract.

use finstack_ai_kernel::RunPhase;

/// Legal restored class after process loss (TDD §23.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalRestore {
    /// Terminal success.
    Completed,
    /// Terminal failure.
    Failed,
    /// Terminal cancellation.
    Cancelled,
    /// Explicit suspension, including interaction and external wait.
    Suspended,
    /// Safe to retry the same identity.
    Retryable,
    /// Documented uncertainty or corruption.
    Uncertain,
}

/// Map a reconstructed phase onto a legal restore class.
#[must_use]
pub fn classify_phase(phase: RunPhase) -> LegalRestore {
    match phase {
        RunPhase::Completed => LegalRestore::Completed,
        RunPhase::Failed => LegalRestore::Failed,
        RunPhase::Cancelled | RunPhase::Cancelling => LegalRestore::Cancelled,
        RunPhase::Suspended
        | RunPhase::AwaitingInteraction
        | RunPhase::AwaitingExternal
        | RunPhase::Sleeping => LegalRestore::Suspended,
        RunPhase::Accepted
        | RunPhase::BeforeRun
        | RunPhase::PreparingContext
        | RunPhase::BeforeModel
        | RunPhase::AwaitingModel
        | RunPhase::AfterModel
        | RunPhase::BeforeToolBatch
        | RunPhase::AwaitingTools
        | RunPhase::AfterToolBatch
        | RunPhase::BeforeFinalize => LegalRestore::Retryable,
    }
}
