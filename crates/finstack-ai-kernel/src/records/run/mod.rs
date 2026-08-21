//! Run lineage, security context, and `RunAccepted` (TDD §11.5).

mod accepted;
mod cancellation;
mod child;
mod error;
mod external;
mod propagation;
mod relation;
mod security;
mod validate;

#[cfg(test)]
mod tests;

/// V1 maximum `RunRelation.depth` (root is 0).
pub const MAX_RUN_RELATION_DEPTH: u16 = 16;

pub use accepted::RunAccepted;
pub use cancellation::{
    CancellationInitiator, CancellationReconciled, CancellationRequest, CancellationRequested,
};
pub use child::{ChildPlacement, ChildRunLocator, ChildRunPrepared, RemoteRouteRef};
pub use error::RunError;
pub use external::{
    ExternalCommandError, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    ExternalEffectCompletionCommand, InteractionResolutionCommand, OperationLocator,
    RecordExternalCommandRejected,
};
pub use propagation::{
    BudgetPropagation, CancellationPropagation, DeadlinePropagation, PrincipalPropagation,
    RunPropagationPolicy,
};
pub use relation::{RunRelation, RunRelationKind};
pub use security::{CompactionAuthorization, RunSecurityContext};
