//! Stable failures without provider or transcript payloads.

/// Invalid frozen configuration or a configured bound was exceeded.
pub const EVAL_SPEC_INVALID: &str = "eval_spec_invalid";
/// Reopening would change an immutable experiment specification.
pub const EVAL_SPEC_DIVERGED: &str = "eval_spec_diverged";
/// A declared subject has no implementation.
pub const EVAL_SUBJECT_UNBOUND: &str = "eval_subject_unbound";
/// A subject's resolved lock changed.
pub const EVAL_SUBJECT_LOCK_MISMATCH: &str = "eval_subject_lock_mismatch";
/// A score exceeds one million micros.
pub const EVAL_SCORE_OUT_OF_RANGE: &str = "eval_score_out_of_range";
/// An attempt reservation or append does not follow the committed sequence.
pub const EVAL_ATTEMPT_SEQUENCE_CONFLICT: &str = "eval_attempt_sequence_conflict";
/// A final cell cannot admit another attempt.
pub const EVAL_CELL_FINALIZED: &str = "eval_cell_finalized";
/// Spending exhausted the admission threshold.
pub const EVAL_BUDGET_EXHAUSTED: &str = "eval_budget_exhausted";
/// Spending lacks compatible, complete cost coverage.
pub const EVAL_COST_UNKNOWN: &str = "eval_cost_unknown";
/// An unfinished attempt requires reconciliation before replacement.
pub const EVAL_ATTEMPT_UNRESOLVED: &str = "eval_attempt_unresolved";
/// Another runner owns this experiment.
pub const EVAL_RUNNER_BUSY: &str = "eval_runner_busy";
/// A constrained grader result is invalid.
pub const EVAL_JUDGE_OUTPUT_INVALID: &str = "eval_judge_output_invalid";
/// The task target does not satisfy the scorer's contract.
pub const EVAL_TARGET_INVALID: &str = "eval_target_invalid";
/// Storage could not acknowledge a durable operation.
pub const EVAL_STORE_UNAVAILABLE: &str = "eval_store_unavailable";
/// A persisted store schema is unsupported.
pub const EVAL_STORE_VERSION: &str = "eval_store_version";
/// A recorded execution cannot be reconstructed.
pub const EVAL_RESCORE_SESSION_MISSING: &str = "eval_rescore_session_missing";
/// Scoring requires journal records omitted by retention or unavailable history.
pub const EVAL_HISTORY_INCOMPLETE: &str = "eval_history_incomplete";
/// A scorer callback failed or exceeded its configured execution bound.
pub const EVAL_SCORER_FAILED: &str = "eval_scorer_failed";
/// Arithmetic exceeded its checked integer bounds.
pub const EVAL_ARITHMETIC_OVERFLOW: &str = "eval_arithmetic_overflow";

/// Stable host-level failure. Messages are bounded, safe static descriptions.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct EvalError {
    code: &'static str,
    message: &'static str,
}

impl EvalError {
    /// Construct a failure without serializing external diagnostics.
    #[must_use]
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
    /// Machine-readable failure code shared with bindings.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

/// Store failures use the same stable code vocabulary as runner failures.
pub type EvalStoreError = EvalError;

pub(crate) fn invalid() -> EvalError {
    EvalError::new(EVAL_SPEC_INVALID, "invalid evaluation configuration")
}
pub(crate) fn unavailable() -> EvalError {
    EvalError::new(EVAL_STORE_UNAVAILABLE, "evaluation store unavailable")
}
