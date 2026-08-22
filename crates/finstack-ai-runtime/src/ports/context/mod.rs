//! Context-provider port, committed invocation guard, and deterministic assembly.

mod assembly;
mod committed;
mod error;
mod port;
#[cfg(all(test, feature = "native-tokio"))]
mod projection;
mod types;

#[cfg(all(test, feature = "native-tokio"))]
mod tests;

pub use assembly::{AssembledContext, ContextTruncationDiagnostic, assemble_context};
#[cfg(all(test, feature = "native-tokio"))]
pub(crate) use committed::context_resume_action;
#[cfg(all(test, any(feature = "native-tokio", feature = "wasm-host")))]
pub(crate) use committed::map_context_reconcile_result;
pub use committed::{CommittedContextCall, InvocationResumeAction, RecordedContextContribution};
pub use error::{
    CONTEXT_BUDGET_EXCEEDED, CONTEXT_COMMIT_REQUIRED, CONTEXT_CONFIGURATION_INVALID,
    CONTEXT_CONTRIBUTION_INVALID, CONTEXT_RECOVERY_UNCERTAIN, ContextError,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use port::CONTEXT_STAGE;
pub use port::{
    ContextCallContext, ContextProvider, ContextProviderDescriptor, ContextReconcileResult,
    PendingContextEffect,
};
#[cfg(all(test, feature = "native-tokio"))]
pub(crate) use projection::{
    CapabilityContext, ContextProjectionInput, ContextProjectionItem, ContextProjectionSource,
    assemble_context_projection,
};
pub use types::{
    ContextAuthority, ContextBudget, ContextContribution, ContextItem, ContextItemKind,
    ContextOverflowPolicy, ContextProvenance, ContextRequest,
};
