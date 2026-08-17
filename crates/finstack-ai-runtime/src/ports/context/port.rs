use finstack_ai_kernel::{ComponentInvocation, Digest, Metadata, PipelinePosition};
use serde::{Deserialize, Serialize};

use crate::{PortFuture, PortObject, ReconcileContext, RunCallContext};

use super::error::ContextError;
use super::types::{ContextContribution, ContextRequest};

pub(super) const CONTEXT_STAGE: &str = "prepare_context";

/// Immutable context-provider descriptor locked at agent resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProviderDescriptor {
    /// Exact component/version/configuration and recovery class.
    pub invocation: ComponentInvocation,
    /// Whether this provider may contribute trusted application instructions.
    pub trusted_application_instructions: bool,
    /// Non-secret descriptor metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

/// Context-specific committed call context.
#[derive(Debug, Clone)]
pub struct ContextCallContext {
    /// Shared identity, authority, deadline, budget, and cancellation context.
    pub run: RunCallContext,
    /// Locked provider order in the resolved chain.
    pub provider_index: u32,
    /// Locked chain digest.
    pub chain_digest: Digest,
}

/// Outstanding context effect supplied to `reconcile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingContextEffect {
    /// Frozen request.
    pub request: ContextRequest,
    /// Committed component invocation.
    pub invocation: ComponentInvocation,
    /// Committed pipeline position.
    pub pipeline: PipelinePosition,
}

/// Context reconciliation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextReconcileResult {
    /// Normalized output is available.
    Completed(ContextContribution),
    /// The original call provably did not start.
    NotStarted,
    /// Reusing the same effect identity is safe.
    RetrySafe,
    /// The provider cannot classify the effect.
    Unknown,
    /// A non-repeatable external action may have occurred.
    NonRepeatable,
}

/// Object-safe context-provider port.
///
/// Collects one bounded contribution after the matching request record
/// commits. Contributions are data, never authority.
pub trait ContextProvider: PortObject {
    /// Immutable descriptor.
    fn descriptor(&self) -> ContextProviderDescriptor;

    /// Collect one committed bounded contribution.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Invocation locator, cancellation, and deadline.
    /// * `request` - Normalized query plus budget ceilings.
    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>>;

    /// Reconcile an outstanding committed invocation.
    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingContextEffect,
    ) -> PortFuture<Result<ContextReconcileResult, ContextError>> {
        Box::pin(async { Ok(ContextReconcileResult::Unknown) })
    }
}
