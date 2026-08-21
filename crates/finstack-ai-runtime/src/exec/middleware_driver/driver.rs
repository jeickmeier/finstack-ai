use std::sync::Arc;

use finstack_ai_kernel::Stage;

use crate::middleware::ResolvedMiddlewareChain;
use crate::model::CancellationSignal;

/// Handle for one resolved middleware chain's stage boundaries.
///
/// Holds the locked chain and the run-scoped cancellation signal shared by
/// every component invocation in the run. [`StageDriver::is_active`] answers
/// whether a stage has any component to run at all.
#[derive(Clone)]
pub struct StageDriver {
    chain: Arc<ResolvedMiddlewareChain>,
    cancellation: CancellationSignal,
}

impl StageDriver {
    /// Construct a driver over a locked chain and its run's cancellation signal.
    #[must_use]
    pub fn new(chain: Arc<ResolvedMiddlewareChain>, cancellation: CancellationSignal) -> Self {
        Self {
            chain,
            cancellation,
        }
    }

    /// The locked, resolved middleware chain.
    #[must_use]
    pub fn chain(&self) -> &Arc<ResolvedMiddlewareChain> {
        &self.chain
    }

    /// Whether any component is registered for `stage`.
    ///
    /// The passthrough gate: `false` means the caller should skip the
    /// aggregate fold entirely and proceed as if no middleware were
    /// configured for this stage.
    #[must_use]
    pub fn is_active(&self, stage: Stage) -> bool {
        !self.chain.stage(stage).is_empty()
    }

    /// The run-scoped cancellation signal shared by every component invocation.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationSignal {
        &self.cancellation
    }
}
