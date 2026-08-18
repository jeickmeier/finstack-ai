use std::sync::Arc;

use finstack_ai_kernel::ValidatedToolCall;

use crate::{PortFuture, PortObject, ToolSpec};

use super::error::ToolError;
use super::types::{
    NestedSample, PendingToolEffect, ToolCallContext, ToolEventStream, ToolReconcileResult,
    ToolsetDescriptor,
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

    /// Resume a committed tool after the runtime fulfills nested sampling.
    ///
    /// Default implementations reject with [`super::error::MCP_SAMPLING_UNSUPPORTED`].
    /// MCP sends the sample back to the server and finishes `tools/call`.
    ///
    /// # Errors
    ///
    /// Returns a stable tool error when the Toolset cannot complete the sample
    /// or the follow-up `tools/call` result is invalid.
    fn complete_nested_sample(
        &self,
        _ctx: ToolCallContext,
        _call: ValidatedToolCall,
        _sample: NestedSample,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        Box::pin(async {
            Err(ToolError::stable(
                super::error::MCP_SAMPLING_UNSUPPORTED,
                "nested sampling is not supported",
            ))
        })
    }
}
