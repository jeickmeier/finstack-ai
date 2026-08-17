use crate::{PortFuture, PortObject, ReconcileContext};

use super::error::MiddlewareError;
use super::types::{
    MiddlewareContext, MiddlewareDescriptor, MiddlewareReconcileResult, PendingMiddlewareEffect,
    StageInput, StageMask, StageOutcome,
};

/// Object-safe single-invocation middleware port.
///
/// Stages are coarse boundaries (`before_model`, `before_finalize`, …).
/// There is no per-token hook. At most one late-tier `before_model`
/// compaction owner may be active per resolved agent.
pub trait Middleware: PortObject {
    /// Immutable descriptor.
    fn descriptor(&self) -> MiddlewareDescriptor;

    /// Declared stage mask.
    fn stages(&self) -> StageMask {
        self.descriptor().stages
    }

    /// Invoke one committed stage boundary.
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

    /// Reconcile an outstanding committed invocation.
    ///
    /// # Deprecated in place: never called
    ///
    /// The runtime has no caller for this method and will not grow one under
    /// the aggregate-fold design: a middleware invocation is never a committed
    /// effect, so nothing is ever outstanding. Recovery re-runs the whole chain
    /// instead (section 2 of the [`crate::middleware_driver`] module contract),
    /// which is why implementations must be pure with respect to external
    /// state. Overriding this method has no effect on any run; do not rely on
    /// it for crash safety.
    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingMiddlewareEffect,
    ) -> PortFuture<Result<MiddlewareReconcileResult, MiddlewareError>> {
        Box::pin(async { Ok(MiddlewareReconcileResult::Unknown) })
    }
}
