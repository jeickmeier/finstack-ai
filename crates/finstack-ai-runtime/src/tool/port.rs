use std::sync::Arc;

use finstack_ai_kernel::ValidatedToolCall;

use crate::{PortFuture, PortObject, ToolSpec};

use super::error::ToolError;
use super::types::{
    PendingToolEffect, ToolCallContext, ToolEventStream, ToolReconcileResult, ToolsetDescriptor,
};

/// Object-safe executable Toolset port.
///
/// Implementors run only after the matching tool-call records commit.
/// Side-effecting tools still require a durable approval decision from the
/// host policy. Native objects are `Send + Sync`.
pub trait Toolset: PortObject {
    /// Immutable Toolset descriptor.
    fn descriptor(&self) -> ToolsetDescriptor;

    /// Data-only tool specifications owned by this Toolset.
    fn tools(&self) -> Arc<[ToolSpec]>;

    /// Start one committed direct tool call.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Invocation locator, cancellation, and deadline.
    /// * `call` - Validated tool name and arguments. Schemas are already checked.
    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>>;

    /// Reconcile one previously committed outstanding effect.
    fn reconcile(
        &self,
        _ctx: crate::ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ToolReconcileResult, ToolError>> {
        Box::pin(async { Ok(ToolReconcileResult::Unknown) })
    }
}
