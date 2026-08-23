//! Shared run-handle types used by both native Tokio and wasm-host owners.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::EffectId;
use thiserror::Error;

use crate::coordinator::{CommitCoordinatorError, CommitOutcome};
use crate::events::EventHubConfig;
use crate::ports::model::{ApprovalGrantMode, ModelError, ModelStreamAssembler, ModelStreamLimits};
use crate::ports::tool::ToolStreamLimits;

pub(crate) fn same_identity_retryable(error: &ModelError) -> bool {
    error.retryable()
        && !matches!(
            error.category(),
            finstack_ai_kernel::ErrorCategory::Validation
                | finstack_ai_kernel::ErrorCategory::Limit
        )
}

pub(crate) fn provider_retry_after(error: &ModelError) -> Option<Duration> {
    let value: serde_json::Value = serde_json::from_str(error.metadata().as_str()).ok()?;
    let raw = value
        .as_object()?
        .get("retry_after")
        .or_else(|| value.as_object()?.get("Retry-After"))?;
    let seconds = match raw {
        serde_json::Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().and_then(finite_seconds_to_u64)),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }?;
    Some(Duration::from_secs(seconds))
}

fn finite_seconds_to_u64(seconds: f64) -> Option<u64> {
    if !seconds.is_finite() || seconds.is_sign_negative() {
        return None;
    }
    let ceiled = seconds.ceil();
    if ceiled >= 9_007_199_254_740_992.0 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "retry-after is clamped to a non-negative finite second count"
    )]
    Some(ceiled as u64)
}

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

/// Lifecycle-status transitions shared by the native and wasm-host run owners.
///
/// The native owner reads status through a `watch` channel and the host owner
/// through a poison-aware `Mutex`, but the transition *rules* must not diverge
/// across targets: a [`RunStatus::Faulted`] status is terminal and outranks a
/// normal drain, so a worker that ends after faulting must never overwrite the
/// fault with [`RunStatus::Stopped`]. Implementors supply the two primitive
/// accessors; the decision lives here so it has exactly one definition.
pub(crate) trait RunLifecycle {
    /// Current observable run status.
    ///
    /// A poisoned status lock is reported as [`RunStatus::Faulted`] so the
    /// drain rule below fails closed and preserves the fault rather than
    /// clearing it.
    fn lifecycle_status(&self) -> RunStatus;

    /// Replace the observable status and mirror it into live state.
    fn set_lifecycle(&self, status: RunStatus);

    /// Publish [`RunStatus::Stopped`] only when no fault has been recorded.
    fn publish_stopped_unless_faulted(&self) {
        if !matches!(self.lifecycle_status(), RunStatus::Faulted { .. }) {
            self.set_lifecycle(RunStatus::Stopped);
        }
    }
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
    /// How paid-tool approvals are parked and released.
    pub approval_grant: ApprovalGrantMode,
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
    /// Portable deterministic delay policy between attempts.
    pub backoff: RetryBackoffPolicy,
}

/// Target-neutral same-identity retry delay policy.
///
/// Jitter is derived from the committed effect identity and retry ordinal. It
/// is therefore stable across restart and browser/native targets and never
/// depends on ambient OS entropy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryBackoffPolicy {
    /// Delay before the first retry.
    pub initial_delay: Duration,
    /// Inclusive ceiling for exponential delay plus jitter.
    pub maximum_delay: Duration,
    /// Inclusive deterministic jitter ceiling.
    pub maximum_jitter: Duration,
}

impl Default for RetryBackoffPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(250),
            maximum_delay: Duration::from_secs(5),
            maximum_jitter: Duration::from_millis(100),
        }
    }
}

impl RetryBackoffPolicy {
    fn validate(self) -> Result<(), RunHandleError> {
        if self.initial_delay > self.maximum_delay || self.maximum_jitter > self.maximum_delay {
            return Err(RunHandleError::InvalidConfiguration);
        }
        Ok(())
    }

    /// Derive the bounded delay for a one-based retry ordinal.
    #[must_use]
    pub fn delay(self, effect_id: EffectId, retry_ordinal: u32) -> Duration {
        let shift = retry_ordinal.saturating_sub(1).min(31);
        let multiplier = 1_u32 << shift;
        let base = self
            .initial_delay
            .checked_mul(multiplier)
            .unwrap_or(self.maximum_delay)
            .min(self.maximum_delay);
        let jitter_ceiling_ms = u64::try_from(self.maximum_jitter.as_millis()).unwrap_or(u64::MAX);
        if jitter_ceiling_ms == 0 {
            return base;
        }
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in effect_id
            .as_bytes()
            .iter()
            .copied()
            .chain(retry_ordinal.to_le_bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let jitter_ms = hash % jitter_ceiling_ms.saturating_add(1);
        base.checked_add(Duration::from_millis(jitter_ms))
            .unwrap_or(self.maximum_delay)
            .min(self.maximum_delay)
    }
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
    /// Same-identity provider retry. Default is no retry.
    pub same_identity_retry: SameIdentityRetryPolicy,
}

impl ModelTaskConfig {
    pub(crate) fn validate(&self) -> Result<ModelStreamAssembler, RunHandleError> {
        if self.job_capacity == 0 || self.result_capacity == 0 {
            return Err(RunHandleError::InvalidConfiguration);
        }
        self.same_identity_retry.backoff.validate()?;
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
///   (`GOVERNANCE.md`) defines the breaking
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
    /// Committed artifact ownership could not be reconciled.
    #[error("artifact ownership reconciliation failed: {code}")]
    Artifact {
        /// Stable artifact error code.
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

/// Stable code for a run rejected before it started.
pub const RUN_INVALID_CONFIGURATION: &str = "run_invalid_configuration";
/// Stable code for a run refused because the owner is shutting down.
pub const RUN_SHUTTING_DOWN: &str = "run_shutting_down";
/// Stable code for a run whose owner stopped before it completed.
pub const RUN_STOPPED: &str = "run_stopped";
/// Stable code for a run whose intake closed before submission.
pub const RUN_INTAKE_CLOSED: &str = "run_intake_closed";

impl RunHandleError {
    /// Stable machine-readable code for this failure.
    ///
    /// Every variant reports one: those carrying a `code` field return it,
    /// and the four state variants report a named constant.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidConfiguration => RUN_INVALID_CONFIGURATION,
            Self::ShuttingDown => RUN_SHUTTING_DOWN,
            Self::Stopped => RUN_STOPPED,
            Self::IntakeClosed => RUN_INTAKE_CLOSED,
            Self::Faulted { code }
            | Self::ModelSettlement { code }
            | Self::ToolSettlement { code }
            | Self::Artifact { code }
            | Self::InteractionSettlement { code }
            | Self::Timer { code }
            | Self::CancellationSettlement { code }
            | Self::EventDelivery { code } => code,
            Self::Model { code } | Self::Tool { code } | Self::Middleware { code } => code,
            Self::Coordinator(error) => error.code(),
        }
    }
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
            | RunHandleError::Artifact { code }
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
    use std::time::Duration;

    use finstack_ai_kernel::{EffectTag, Id};

    use super::{RetryBackoffPolicy, RunHandleError, result_fault_code};

    fn effect(ordinal: u64) -> Id<EffectTag> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    #[test]
    fn retry_backoff_is_deterministic_bounded_and_effect_scoped() {
        let policy = RetryBackoffPolicy {
            initial_delay: Duration::from_millis(100),
            maximum_delay: Duration::from_millis(450),
            maximum_jitter: Duration::from_millis(50),
        };
        let first = policy.delay(effect(1), 1);
        assert_eq!(first, policy.delay(effect(1), 1));
        assert!((Duration::from_millis(100)..=Duration::from_millis(150)).contains(&first));
        assert_ne!(first, policy.delay(effect(2), 1));
        assert!(policy.delay(effect(1), 32) <= policy.maximum_delay);
    }

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
