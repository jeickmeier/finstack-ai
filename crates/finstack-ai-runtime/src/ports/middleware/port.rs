use crate::ports::{PortFuture, PortObject};

use super::error::MiddlewareError;
use super::types::{MiddlewareContext, MiddlewareDescriptor, StageInput, StageMask, StageOutcome};

/// Object-safe single-invocation middleware port.
///
/// Stages are coarse boundaries (`before_model`, `before_finalize`, …).
/// There is no per-token hook. At most one late-tier `before_model`
/// compaction owner may be active per resolved agent.
///
/// A middleware invocation is never a committed effect. Recovery re-runs
/// the whole chain when a cursor's `StageOutcomeRecorded` is absent, so
/// implementations must be pure with respect to external state.
pub trait Middleware: PortObject {
    /// Immutable descriptor.
    fn descriptor(&self) -> MiddlewareDescriptor;

    /// Declared stage mask.
    fn stages(&self) -> StageMask {
        self.descriptor().stages
    }

    /// Invoke one stage boundary.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Stage identity, locator, and cancellation.
    /// * `input` - Immutable stage payload. Return a replacement or continue.
    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>>;
}
