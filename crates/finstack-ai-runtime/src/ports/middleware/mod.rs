//! Middleware port, deterministic ordering, durable invocation guards, and compaction validation.

mod chain;
mod committed;
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
/// Stable missing committed-invocation error.
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
pub use committed::{CommittedMiddlewareCall, RecordedMiddlewareOutcome, middleware_resume_action};
pub use digest::{
    compaction_checkpoint_compatible, compaction_projection_digest,
    compaction_protected_set_digest, compaction_source_digest, compaction_summary_digest,
};
pub use error::MiddlewareError;
pub use port::Middleware;
pub(crate) use types::parse_stage;
pub use types::{
    BeforeModelInput, CompactedSummary, CompactionCheckpoint, CompactionEvidence,
    CompactionModelRequest, CompactionModelResume, CompactionResult, CompactionSourceEntry,
    MiddlewareContext, MiddlewareDescriptor, MiddlewareOrder, MiddlewareReconcileResult,
    MiddlewareRole, OrderTier, PendingMiddlewareEffect, PromptCacheImpact, Stage, StageInput,
    StageMask, StageOutcome, stage_name,
};
pub use validate::{
    validate_compaction_model_effect, validate_compaction_result, validate_stage_outcome,
};
