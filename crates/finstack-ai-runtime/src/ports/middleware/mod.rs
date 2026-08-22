//! Middleware port, deterministic ordering, and compaction validation.

mod chain;
mod digest;
mod error;
mod port;
mod types;
mod validate;

#[cfg(test)]
mod tests;

/// Stable middleware resolution error.
pub const MIDDLEWARE_RESOLUTION_INVALID: &str = "middleware_resolution_invalid";
/// Stable middleware-cycle error.
pub const MIDDLEWARE_ORDER_CYCLE: &str = "middleware_order_cycle";
/// Stable stage-input normalization failure.
///
/// The code string is historical (`middleware_commit_required`) and stays
/// wire-stable. It is emitted when a stage input cannot be canonicalized,
/// not because a committed middleware effect is missing.
pub const MIDDLEWARE_COMMIT_REQUIRED: &str = "middleware_commit_required";
/// Stable stage/outcome matrix error.
pub const MIDDLEWARE_OUTCOME_NOT_ALLOWED: &str = "middleware_outcome_not_allowed";
/// Stable compaction-integrity error.
pub const COMPACTION_RESULT_INVALID: &str = "compaction_result_invalid";
/// Stable context hard-budget error.
pub const COMPACTION_BUDGET_EXCEEDED: &str = "context_budget_exceeded";
/// Stable unauthorized compaction-child-model error.
pub const COMPACTION_MODEL_NOT_AUTHORIZED: &str = "compaction_model_not_authorized";

pub use chain::{MiddlewareRegistration, ResolvedMiddleware, ResolvedMiddlewareChain};
pub use digest::{
    compaction_checkpoint_compatible, compaction_projection_digest,
    compaction_protected_set_digest, compaction_source_digest, compaction_summary_digest,
};
pub use error::MiddlewareError;
pub use error::{MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE};
pub use port::Middleware;
#[cfg(test)]
pub(crate) use types::parse_stage;
#[cfg(any(test, feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use types::stage_name;
pub use types::{
    BeforeModelInput, BeforeToolBatchInput, CompactedSummary, CompactionCheckpoint,
    CompactionEvidence, CompactionModelRequest, CompactionModelResume, CompactionResult,
    CompactionSourceEntry, MiddlewareContext, MiddlewareDescriptor, MiddlewareOrder,
    MiddlewareRole, OrderTier, PromptCacheImpact, StageInput, StageMask, StageOutcome,
};
#[cfg(test)]
pub(crate) use validate::validate_compaction_model_effect;
pub use validate::{
    authorize_compaction_model_request, validate_compaction_result, validate_stage_outcome,
};
