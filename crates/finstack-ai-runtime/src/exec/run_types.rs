//! Shared run-handle types used by both native Tokio and wasm-host owners.

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::coordinator::{CommitCoordinatorError, CommitOutcome};
use crate::{EventHubConfig, Metadata, ModelStreamAssembler, ModelStreamLimits, ToolStreamLimits};

/// Observable lifecycle of one owned runtime task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Intake and commit processing are active.
    Running,
    /// New intake is closed while the current boundary finishes.
    ShuttingDown,
    /// The owned worker has stopped.
    Stopped,
    /// Runtime uncertainty faulted this run only.
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
}

/// How owned runtime tasks stopped during explicit shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// Every owned task joined within the configured grace period.
    Graceful,
    /// Remaining owned tasks were aborted after the grace period elapsed.
    Forced,
    /// Dropping the sole task owner forced immediate local termination.
    OwnerDropped,
}

/// Safe operational diagnostics for one completed shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Graceful or forced termination classification.
    pub outcome: ShutdownOutcome,
    /// Active effect/timer signals present when shutdown began.
    pub signalled_effects: usize,
    /// Owned tasks still present when forced termination began.
    pub aborted_tasks: usize,
}

/// Process-local diagnostics from durable wall-to-monotonic timer conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerDiagnostics {
    /// Timers already due when installed or restored.
    pub already_due: u64,
    /// Restored timers clamped after a backward wall-clock anomaly.
    pub backward_clock_clamped: u64,
}

/// Configuration for a bounded run task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTaskConfig {
    /// Bounded command queue capacity.
    pub command_capacity: usize,
    /// Bounded event-hub source and subscriber limits.
    pub event_hub: EventHubConfig,
    /// Maximum time the owner waits before aborting owned tasks.
    pub shutdown_deadline: Duration,
}

impl RunTaskConfig {
    /// Validate non-zero queue and shutdown bounds.
    ///
    /// # Errors
    ///
    /// Returns [`RunHandleError::InvalidConfiguration`] for a zero bound.
    pub fn validate(self) -> Result<Self, RunHandleError> {
        if self.command_capacity == 0
            || self.shutdown_deadline.is_zero()
            || self.event_hub.validate().is_err()
        {
            return Err(RunHandleError::InvalidConfiguration);
        }
        Ok(self)
    }
}

/// Same-`effect_id` HTTP/provider retry for one committed model effect.
///
/// This is not whole-run `BeforeFinalize` semantic retry
/// (`RecordBody::RetryScheduled` / `TimerFired`). Default is zero extra
/// attempts. Journals of first-attempt success and success after N retries
/// stay identical; observer events may differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SameIdentityRetryPolicy {
    /// Extra attempts after the first. Zero disables retry.
    pub max_retries: u32,
}

/// Configuration for the private bounded model job/result path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTaskConfig {
    /// Bounded committed model-job queue capacity.
    pub job_capacity: usize,
    /// Bounded incremental driver-message queue capacity.
    pub result_capacity: usize,
    /// Pure stream-assembler bounds.
    pub stream_limits: ModelStreamLimits,
    /// Optional construction warmup deadline.
    pub warmup_deadline: Option<finstack_ai_kernel::Timestamp>,
    /// Bounded non-secret warmup metadata.
    pub warmup_metadata: Metadata,
    /// Same-identity provider retry. Default is no retry.
    pub same_identity_retry: SameIdentityRetryPolicy,
}

impl ModelTaskConfig {
    pub(crate) fn validate(&self) -> Result<ModelStreamAssembler, RunHandleError> {
        if self.job_capacity == 0 || self.result_capacity == 0 {
            return Err(RunHandleError::InvalidConfiguration);
        }
        ModelStreamAssembler::new(self.stream_limits)
            .map_err(|_| RunHandleError::InvalidConfiguration)
    }
}

/// Configuration for bounded tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolTaskConfig {
    /// Bounded committed tool-job queue capacity.
    pub job_capacity: usize,
    /// Bounded incremental driver-message queue capacity.
    pub result_capacity: usize,
    /// Executor-wide active-call ceiling.
    pub global_max_concurrency: usize,
    /// Target-neutral stream normalization limits.
    pub stream_limits: ToolStreamLimits,
}

impl ToolTaskConfig {
    pub(crate) fn validate(self) -> Result<Self, &'static str> {
        if self.job_capacity == 0
            || self.result_capacity == 0
            || self.global_max_concurrency == 0
            || self.stream_limits.max_items == 0
            || self.stream_limits.max_stream_bytes == 0
        {
            return Err("tool_task_configuration_invalid");
        }
        Ok(self)
    }
}

/// Bounded run-handle failures.
///
/// # Deliberately not `#[non_exhaustive]`
///
/// Adding a variant (most recently [`Self::Middleware`]) breaks a downstream
/// exhaustive `match`, and `scripts/compat/public_items.py` tracks item names
/// rather than enum variants, so no gate catches it. That was reviewed and left
/// as is, for three reasons:
///
/// - The 1.0 promise for public Rust
///   (`docs/implementation/1.0-compatibility-policy.md`) defines the breaking
///   set as semantic renames and removals; variant addition is outside it, and
///   the checked-in public-item list implements exactly that policy.
/// - `#[non_exhaustive]` appears on no type in this workspace. Applying it to
///   one of roughly a dozen public error enums (`ModelError`, `ToolError`,
///   `ContextError`, `MiddlewareError`, `CommitCoordinatorError`, …) would be an
///   arbitrary split; the decision belongs workspace-wide, in an ADR.
/// - Adding it is itself a breaking change for any exhaustive match, so it
///   trades one certain break now for a hypothetical one later.
///
/// The window is not closed: nothing is published to crates.io, so no
/// downstream exhaustive match exists yet and the attribute can still be added
/// for free. It must be decided before first publication, after which it
/// becomes a major-version-only change. Anyone adding a variant should re-read
/// this note rather than assume the question was never asked.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunHandleError {
    /// Queue capacity or shutdown deadline was zero.
    #[error("invalid run task configuration")]
    InvalidConfiguration,
    /// Shutdown has closed command intake.
    #[error("run is shutting down")]
    ShuttingDown,
    /// Owned worker has stopped.
    #[error("run has stopped")]
    Stopped,
    /// Runtime uncertainty faulted the run.
    #[error("run faulted: {code}")]
    Faulted {
        /// Stable fault code.
        code: &'static str,
    },
    /// Worker intake or reply path closed unexpectedly.
    #[error("run intake closed")]
    IntakeClosed,
    /// Coordinator rejected or faulted the submission.
    #[error(transparent)]
    Coordinator(CommitCoordinatorError),
    /// Model warmup or execution adapter failed before a durable settlement.
    #[error("model adapter failed: {code}")]
    Model {
        /// Stable adapter code.
        code: Arc<str>,
    },
    /// Runtime could not construct a valid kernel settlement.
    #[error("model settlement construction failed: {code}")]
    ModelSettlement {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Tool execution adapter failed before a durable settlement.
    #[error("tool adapter failed: {code}")]
    Tool {
        /// Stable adapter code.
        code: Arc<str>,
    },
    /// Runtime could not construct a valid kernel tool settlement.
    #[error("tool settlement construction failed: {code}")]
    ToolSettlement {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Runtime could not construct a valid interaction request or settlement.
    #[error("interaction settlement construction failed: {code}")]
    InteractionSettlement {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Durable timer adapter failed before a firing could be committed.
    #[error("timer adapter failed: {code}")]
    Timer {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Cancellation reconciliation could not be normalized or committed.
    #[error("cancellation reconciliation failed: {code}")]
    CancellationSettlement {
        /// Stable runtime code.
        code: &'static str,
    },
    /// Runtime event publication failed after apply and before dispatch.
    #[error("event delivery failed: {code}")]
    EventDelivery {
        /// Stable event-delivery code.
        code: &'static str,
    },
    /// A middleware stage chain failed, or its aggregate fold had no kernel
    /// landing at the settled cursor.
    ///
    /// Peer of [`Self::Model`] and [`Self::Tool`]: an owned `Arc<str>` because
    /// the code can come from a component's own `MiddlewareError::code()`, not
    /// only from a runtime literal. Deliberately not a worker fault — a
    /// middleware failure aborts the run that submitted it, exactly like an
    /// invalid stage settlement, and leaves the run task healthy.
    #[error("middleware stage failed: {code}")]
    Middleware {
        /// Stable middleware or driver code.
        code: Arc<str>,
    },
}

/// Worker-tearing fault code from a commit result, if any.
///
/// [`RunHandleError::Middleware`] is excluded: a middleware failure aborts
/// only the submitting run and leaves the worker healthy.
pub(crate) fn result_fault_code(
    result: &Result<CommitOutcome, RunHandleError>,
) -> Option<&'static str> {
    match result {
        Ok(outcome) => outcome.fault.map(|fault| fault.code),
        Err(
            RunHandleError::Faulted { code }
            | RunHandleError::ModelSettlement { code }
            | RunHandleError::ToolSettlement { code }
            | RunHandleError::InteractionSettlement { code }
            | RunHandleError::EventDelivery { code }
            | RunHandleError::Coordinator(
                CommitCoordinatorError::BoundaryFault { code }
                | CommitCoordinatorError::Faulted { code }
                | CommitCoordinatorError::EventDelivery { code },
            ),
        ) => Some(*code),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{RunHandleError, result_fault_code};

    /// The other half of the folded-allocation fix
    /// (`stage_settlement`'s `folded_allocation_error`):
    /// [`RunHandleError::Middleware`] must never tear the worker down.
    /// [`RunHandleError::ToolSettlement`] deliberately still does.
    #[test]
    fn a_middleware_failure_is_not_a_worker_fault() {
        assert_eq!(
            result_fault_code(&Err(RunHandleError::Middleware {
                code: Arc::from("stage_allocation_model_request_contract_mismatch"),
            })),
            None,
            "a middleware fold failure must fail only the run, not the worker"
        );
        assert_eq!(
            result_fault_code(&Err(RunHandleError::ToolSettlement {
                code: "stage_allocation_model_request_contract_mismatch",
            })),
            Some("stage_allocation_model_request_contract_mismatch"),
            "tool settlement stays a worker fault, so the re-classification is load-bearing"
        );
    }
}
