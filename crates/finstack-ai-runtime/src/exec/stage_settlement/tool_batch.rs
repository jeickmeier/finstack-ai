use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{ErrorDescriptor, Stage, StageCursor, ToolCallBlock, ToolId};

use crate::ResolvedToolCatalog;
use crate::coordinator::CommitCoordinator;
use crate::middleware::{BeforeToolBatchInput, StageInput};
use crate::middleware_driver::{
    MIDDLEWARE_STAGE_UNLANDABLE, StageDriver, StageFold, StageTerminal,
};
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;
use crate::{Clock, RandomSource, SideEffectClass};

use super::driver::run_stage_chain;
use super::stage_error;

/// The `BeforeToolBatch` chain's aggregate fold, reduced to what tool planning
/// can actually consume.
///
/// Unlike every other stage, `BeforeToolBatch` does not land an aggregate
/// [`ReducerStageOutcome`] of its own: the batch settles exactly once, as the
/// `ToolBatchPrepared` that `settlement::prepare_tool_batch_if_ready` builds
/// from the per-call catalog decisions. This enum is therefore the whole
/// vocabulary the fold can express at that cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolBatchPolicy {
    /// No component narrowed the tool set and none failed the stage: plan the
    /// batch exactly as it would be planned with no chain installed at all.
    Unchanged,
    /// Only these tool ids may execute. Every source call whose tool is absent
    /// is denied — and therefore becomes a
    /// [`finstack_ai_kernel::ToolCallPlan::SyntheticClosure`], never a dropped
    /// call.
    Retain(BTreeSet<ToolId>),
    /// A component failed the stage. The caller settles
    /// `ReducerStageOutcome::Fail` at the cursor instead of opening a batch.
    Fail(Box<ErrorDescriptor>),
}

/// Run the `BeforeToolBatch` chain over the batch's source calls.
///
/// The `BeforeToolBatch` peer of [`settle_facade_stage`]. Read-only in the
/// coordinator, and a strict passthrough — the source calls are not even
/// canonicalized — whenever no component is registered for the stage, so a run
/// with no `BeforeToolBatch` middleware plans its batch byte for byte as it did
/// before this hook existed.
///
/// # Errors
///
/// Forwards [`run_stage_chain`]'s errors, or returns
/// [`MIDDLEWARE_STAGE_UNLANDABLE`] for a fold carrying a contribution that has
/// no expression in [`ToolBatchPolicy`] — rejected rather than silently
/// dropped, exactly as [`apply_fold`] rejects one it cannot land.
pub(crate) async fn run_tool_batch_chain<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    catalog: &ResolvedToolCatalog,
    driver: Option<&StageDriver>,
    sources: &SettlementSources<C, R>,
    cursor: StageCursor,
    calls: &[ToolCallBlock],
) -> Result<ToolBatchPolicy, RunHandleError> {
    debug_assert_eq!(
        cursor.stage,
        Stage::BeforeToolBatch,
        "the tool-batch chain runs only at the BeforeToolBatch cursor"
    );
    let Some(driver) = driver.filter(|driver| driver.is_active(Stage::BeforeToolBatch)) else {
        return Ok(ToolBatchPolicy::Unchanged);
    };
    let input = StageInput::BeforeToolBatch(Box::new(BeforeToolBatchInput {
        prior_write_tool_calls: prior_write_tool_calls(coordinator, catalog),
        calls: calls.to_vec().into(),
        tools: catalog
            .tools()
            .map(|tool| tool.spec.clone())
            .collect::<Vec<_>>()
            .into(),
    }));
    let fold = run_stage_chain(coordinator, Some(driver), sources, cursor, input).await?;
    tool_batch_policy(&fold)
}

fn prior_write_tool_calls(coordinator: &CommitCoordinator, catalog: &ResolvedToolCatalog) -> u64 {
    let read_only = catalog
        .tools()
        .filter(|tool| tool.spec.side_effect == SideEffectClass::ReadOnly)
        .map(|tool| Arc::clone(&tool.spec.model_name))
        .collect::<BTreeSet<_>>();
    u64::try_from(
        coordinator
            .state()
            .tool_calls
            .values()
            .filter(|identity| !read_only.contains(identity.call.tool_name()))
            .count(),
    )
    .unwrap_or(u64::MAX)
}

/// Reduce a `BeforeToolBatch` fold to the policy tool planning consumes.
///
/// `validate_stage_outcome` already refuses `AddInstructions`/`AddContext`,
/// `CompactContext`, `RequestCompactionModel`, `Complete`, and `Retry` at this
/// stage, and [`StageFold::accumulate`] refuses `Replace`, `Suspend`, and
/// `RequestInteraction` here, so only `FilterTools` and `Fail` can reach this
/// function. The remaining arms are therefore unreachable through the port —
/// but they are rejected rather than ignored, because a fold field this
/// reducer silently skipped would be a component's contribution vanishing
/// without trace.
pub(super) fn tool_batch_policy(fold: &StageFold) -> Result<ToolBatchPolicy, RunHandleError> {
    if let Some(terminal) = fold.terminal.as_ref() {
        return match terminal {
            StageTerminal::Fail(descriptor) => Ok(ToolBatchPolicy::Fail(descriptor.clone())),
            StageTerminal::Retry(_) => Err(stage_error(MIDDLEWARE_STAGE_UNLANDABLE)),
        };
    }
    if !fold.instructions.is_empty()
        || !fold.context.is_empty()
        || fold.replacement.is_some()
        || fold.compaction.is_some()
    {
        return Err(stage_error(MIDDLEWARE_STAGE_UNLANDABLE));
    }
    Ok(match fold.retained_tools.as_ref() {
        Some(retained) => ToolBatchPolicy::Retain(retained.clone()),
        None => ToolBatchPolicy::Unchanged,
    })
}
