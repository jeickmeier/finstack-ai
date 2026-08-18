use finstack_ai_kernel::{Digest, EffectId, OperationLocator, Stage, StageCursor};

use crate::RunCallContext;
use crate::middleware::{
    CompactionModelResume, MiddlewareError, ResolvedMiddleware, ResolvedMiddlewareChain,
    StageInput, StageOutcome, stage_name, validate_stage_outcome,
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
    /// `(cycle, stage)` cursor this invocation settles.
    ///
    /// Replaces the `parent: EffectRequested` field this module carried one
    /// commit ago. That field had no producer: no `KernelInput` ever commits
    /// an `EffectKind::Middleware` `EffectRequested` (stage settlement emits
    /// only `StageOutcomeRecorded`), so there is never a committed parent
    /// effect to reference. The cursor is what the driver actually has at a
    /// stage boundary, and it is also the input [`derived_stage_effect_id`]
    /// needs to fabricate `run.effect_id`.
    pub cursor: StageCursor,
    /// Optional child-model result for the same BeforeModel chain re-entry.
    pub compaction_resume: Option<CompactionModelResume>,
}

impl MiddlewareStageContext {
    /// Construct a stage context from its run-call context, locked chain
    /// digest, and `(cycle, stage)` cursor.
    #[must_use]
    pub const fn new(run: RunCallContext, chain_digest: Digest, cursor: StageCursor) -> Self {
        Self {
            run,
            chain_digest,
            cursor,
            compaction_resume: None,
        }
    }

    /// Attach a compaction-summary resume for chain re-entry.
    #[must_use]
    pub fn with_compaction_resume(mut self, resume: CompactionModelResume) -> Self {
        self.compaction_resume = Some(resume);
        self
    }

    /// The stage this invocation settles.
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.cursor.stage
    }

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
            compaction_resume: self.compaction_resume.clone(),
        })
    }
}

/// Fixed digest domain for [`derived_stage_effect_id`].
const DOMAIN_MIDDLEWARE_STAGE_INVOCATION: &str = "middleware-stage-invocation";

/// Derive a stable, deterministic, **non-committed** correlation id for one
/// stage's chain invocation.
///
/// # This is not a committed effect identity
///
/// No `KernelInput` ever commits an `EffectKind::Middleware`
/// `EffectRequested` — stage settlement emits only `StageOutcomeRecorded`
/// (see the kernel reducer) — so there is no faithful `EffectId` available at
/// a stage boundary in either the runtime or the facade layer. This function
/// fabricates one instead, deterministically from `(locator, cycle, stage)`,
/// so [`RunCallContext`]'s non-`Option` `effect_id` field can be filled with
/// *something* stable rather than left with no faithful value at all.
///
/// Two properties matter and are both required, not incidental:
/// - **Derived, not random**: the same `(locator, cycle, stage)` always
///   produces the same id, so a stage that re-runs its whole chain after a
///   crash (this driver never journals individual invocations) observes the
///   identical correlation id both times.
/// - **Domain-separated**: hashing under a fixed, dedicated digest domain
///   keeps this id from ever colliding with a UuidV7-generated *committed*
///   effect id minted elsewhere in the system.
///
/// Callers must not treat the returned value as a journal key, look it up as
/// a committed effect, or otherwise assume it is backed by any durable
/// record — it is a correlation id only. This distinction has a concrete
/// consequence: `sanitize_call_context`
/// (`plugins/finstack-ai-wit/src/mapping.rs`) copies `RunCallContext.effect_id`
/// verbatim into the guest-visible WIT call context, so a plugin author who
/// assumes that value names a committed effect is wrong, and any future code
/// that tries to correlate this id against the journal is wrong too.
///
/// # Panics
///
/// Does not panic for any real input. The internal canonicalization step is
/// fallible only for values that cannot be represented as JSON (this
/// function's inputs — a bounded `OperationLocator`, a `u64`, and a fixed
/// stage name — always can be), and the digest domain is a fixed non-empty,
/// NUL-free literal, so [`Digest::domain_separated`] cannot reject it either.
#[must_use]
pub fn derived_stage_effect_id(locator: &OperationLocator, cycle: u64, stage: Stage) -> EffectId {
    let canonical = serde_json_canonicalizer::to_vec(&(locator, cycle, stage_name(stage)))
        .expect("OperationLocator/cycle/stage-name are always canonically serializable");
    let digest = Digest::domain_separated(DOMAIN_MIDDLEWARE_STAGE_INVOCATION, 1, &canonical)
        .expect("the fixed middleware-stage-invocation domain is always valid");
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    EffectId::from_bytes(bytes)
}

/// Invoke every component registered for `stage`, in resolved order.
///
/// Each outcome is validated against the stage matrix before it is returned.
/// The caller folds the ordered results into one `ReducerStageOutcome`.
///
/// `ctx.stage()` and `input.stage()` are two independent sources of truth for
/// the same stage — the cursor this invocation settles, and the variant of the
/// payload it hands each component. A mismatched pair would correlate one
/// stage's components under another stage's derived id, so it is asserted in
/// debug builds rather than left to chance.
///
/// # Errors
///
/// Returns the component's own `MiddlewareError`, or a stable
/// `middleware_outcome_not_allowed` when an outcome fails the matrix.
pub async fn invoke_middleware_stage(
    chain: &ResolvedMiddlewareChain,
    ctx: &MiddlewareStageContext,
    input: StageInput,
    live: impl Fn(&finstack_ai_kernel::ComponentId) -> bool,
) -> Result<Vec<StageOutcome>, MiddlewareError> {
    debug_assert_eq!(
        ctx.stage(),
        input.stage(),
        "stage context cursor and stage input must describe the same stage"
    );
    let stage = input.stage();
    let components: &[ResolvedMiddleware] = chain.stage(stage);
    let mut outcomes = Vec::with_capacity(components.len());
    for (index, resolved) in components.iter().enumerate() {
        if !live(&resolved.descriptor.invocation.component) {
            continue;
        }
        let mw_ctx = ctx.middleware_context(index)?;
        let outcome = resolved.middleware.invoke(mw_ctx, input.clone()).await?;
        validate_stage_outcome(&resolved.descriptor, &input, &outcome)?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}
