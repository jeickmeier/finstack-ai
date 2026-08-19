use std::sync::Arc;

use finstack_ai_kernel::Stage;

use crate::middleware::{MiddlewareError, ResolvedMiddlewareChain, StageInput, StageOutcome};
use crate::model::CancellationSignal;

use super::{MiddlewareStageContext, invoke_middleware_stage};

/// Handle for one resolved middleware chain's stage boundaries.
///
/// Holds the locked chain and the run-scoped cancellation signal shared by
/// every component invocation in the run. [`StageDriver::is_active`] answers
/// whether a stage has any component to run at all; [`StageDriver::run_stage_masked`]
/// runs one.
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

    /// Invoke `stage` while skipping components the capability mask hides.
    ///
    /// Thin binding of [`invoke_middleware_stage`] to the chain and
    /// cancellation this driver owns, plus one behaviour of its own: a run that
    /// is already cancelled runs **no** component and returns an empty outcome
    /// list, which folds to the identity and leaves the caller's base outcome
    /// byte-for-byte unchanged. Cancellation must be able to skip optional
    /// work; it must never be able to *change* what a stage settles, because
    /// the kernel — not the driver — owns cancellation's effect on the run.
    ///
    /// # Errors
    ///
    /// Returns the component's own `MiddlewareError`, or a stable
    /// `middleware_outcome_not_allowed` when an outcome fails the stage matrix.
    pub async fn run_stage_masked(
        &self,
        ctx: &MiddlewareStageContext,
        input: StageInput,
        live: impl Fn(&finstack_ai_kernel::ComponentId) -> bool,
    ) -> Result<Vec<StageOutcome>, MiddlewareError> {
        if self.cancellation.is_cancelled() {
            return Ok(Vec::new());
        }
        invoke_middleware_stage(&self.chain, ctx, input, live).await
    }
}
