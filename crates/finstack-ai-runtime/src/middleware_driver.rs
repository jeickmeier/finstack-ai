//! Aggregate middleware chain driver.
//!
//! The kernel models the middleware boundary as exactly one
//! [`ReducerStageOutcome`] per `(cycle, stage)` cursor
//! ([`finstack_ai_kernel::StageOutcomeRecorded`]). This module runs the ordered
//! component chain for a stage in-process, validates each outcome against the
//! stage matrix, and folds the ordered results into that single aggregate
//! outcome. Individual invocations are not committed as effects.
//!
//! Middleware implementations must therefore be pure with respect to external
//! state: on recovery, a stage whose `StageOutcomeRecorded` is absent re-runs
//! its whole chain.

use finstack_ai_kernel::{Digest, EffectRequested};

use crate::RunCallContext;
use crate::middleware::{
    MiddlewareError, ResolvedMiddleware, ResolvedMiddlewareChain, StageInput, StageOutcome,
    validate_stage_outcome,
};

/// Shared identity for one stage's chain invocation.
///
/// Constructed once per `(cycle, stage)` boundary and reused for every component
/// in that stage, so all components in a stage observe the same run identity and
/// the same locked chain digest.
#[derive(Debug, Clone)]
pub struct MiddlewareStageContext {
    /// Run-scoped call context reused for every component in the stage.
    pub run: RunCallContext,
    /// Locked chain digest from the resolved agent.
    pub chain_digest: Digest,
    /// Committed parent effect this stage runs under, used to validate any
    /// child compaction-model effect.
    pub parent: EffectRequested,
}

impl MiddlewareStageContext {
    /// Per-component invocation context for the component at `index`.
    ///
    /// # Errors
    ///
    /// Returns a stable `middleware_resolution_invalid` when the chain index
    /// exceeds `u32`.
    pub fn middleware_context(
        &self,
        index: usize,
    ) -> Result<crate::middleware::MiddlewareContext, MiddlewareError> {
        Ok(crate::middleware::MiddlewareContext {
            run: self.run.clone(),
            chain_digest: self.chain_digest,
            chain_index: u32::try_from(index).map_err(|_| {
                MiddlewareError::stable(
                    crate::middleware::MIDDLEWARE_RESOLUTION_INVALID,
                    "middleware chain index exceeds u32",
                )
            })?,
            compaction_resume: None,
        })
    }
}

/// Invoke every component registered for `stage`, in resolved order.
///
/// Each outcome is validated against the stage matrix before it is returned.
/// The caller folds the ordered results into one `ReducerStageOutcome`.
///
/// # Errors
///
/// Returns the component's own `MiddlewareError`, or a stable
/// `middleware_outcome_not_allowed` when an outcome fails the matrix.
pub async fn invoke_middleware_stage(
    chain: &ResolvedMiddlewareChain,
    ctx: &MiddlewareStageContext,
    input: StageInput,
) -> Result<Vec<StageOutcome>, MiddlewareError> {
    let stage = input.stage();
    let components: &[ResolvedMiddleware] = chain.stage(stage);
    let mut outcomes = Vec::with_capacity(components.len());
    for (index, resolved) in components.iter().enumerate() {
        let mw_ctx = ctx.middleware_context(index)?;
        let outcome = resolved.middleware.invoke(mw_ctx, input.clone()).await?;
        validate_stage_outcome(&resolved.descriptor, &input, &outcome)?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::Stage;

    #[test]
    fn stage_names_round_trip() {
        for stage in [
            Stage::BeforeRun,
            Stage::PrepareContext,
            Stage::BeforeModel,
            Stage::AfterModel,
            Stage::BeforeToolBatch,
            Stage::AfterToolBatch,
            Stage::BeforeFinalize,
        ] {
            let name = crate::middleware::stage_name(stage);
            assert_eq!(crate::middleware::parse_stage(name), Some(stage));
        }
    }
}
