//! Aggregate middleware fold at the stage-settlement choke point.
//!
//! Every facade stage settlement funnels through `RunHandle::submit` into one
//! `coordinator.submit(env, input)` call in a worker command loop. Intercepting
//! the [`KernelInput::StageSettled`] branch there is the single point at which
//! the six facade-authored stages can be routed through their middleware chain,
//! with no change to the facade's stage sequence and no kernel change: the `N`
//! [`crate::middleware::StageOutcome`]s a chain produces are folded into exactly
//! ONE [`ReducerStageOutcome`] per `(cycle, stage)` cursor, which is what
//! `KernelInput::StageSettled` already carries.
//!
//! # Governing invariant: passthrough when the chain is empty
//!
//! When no driver is installed, or the driver has no component registered for
//! the cursor's stage, or the base outcome is not one that stage can fold on top
//! of (see [`driver::foldable_base`]), or the chain's fold is the identity, the facade's
//! `env` and `input` are submitted **byte for byte unchanged** — the facade's
//! own pre-minted record/event/effect ids included. The feature is opt-in per
//! agent: an agent with no middleware must produce a journal identical to the
//! one it produced before this module existed, and no settlement the kernel
//! admits may stop landing merely because a component was registered.
//!
//! # Stage coverage
//!
//! [`settle_facade_stage`] folds all six facade-authored stages: `BeforeRun`,
//! `PrepareContext`, `BeforeModel`, `AfterModel`, `AfterToolBatch`, and
//! `BeforeFinalize`. `BeforeModel` is the one stage whose [`StageInput`] is not
//! a bare `RawJson` but the typed `StageInput::BeforeModel(Box<BeforeModelInput>)`,
//! which is why it needs the run's [`LockedModelContextProfile`] threaded down
//! from the spawn path — see [`input::before_model_input`].
//!
//! ## Compaction landing
//!
//! Two of the seven [`crate::middleware::StageOutcome`] variants a `BeforeModel`
//! component may legally return are compaction outcomes, and they diverge:
//!
//! - `RequestCompactionModel` is fulfilled by the runtime-owned compaction
//!   phase (ADR-042) before the fold: a child `EffectRequested(Model)` under
//!   `EffectPurpose::CompactionSummary`, then chain re-entry with
//!   `compaction_resume`. [`StageFold::accumulate`] still refuses it if that
//!   intercept does not run.
//! - `CompactContext` is landable once the last [`CompactionSourceEntry`] is a
//!   protected user. `protected` stays authoritative-from-the-context-port
//!   ([`crate::ContextItem::protected`]) plus the structural rule (system /
//!   developer messages and the trailing current user). The production
//!   `ContextProvider` driver projects that bit into
//!   [`input::before_model_input`]; a compactor still cannot set it.
//!
//! Every other `BeforeModel` outcome — `AddInstructions`, `AddContext`,
//! `FilterTools`, `Replace`, `Fail`, `Continue` — folds normally through
//! [`apply_model_draft`].
//!
//! `BeforeToolBatch` never reaches this choke point at all — the facade never
//! settles it; `settlement::prepare_tool_batch_if_ready` does, and it is the
//! only stage whose fold is *not* an aggregate `ReducerStageOutcome`. Its
//! chain runs through [`run_tool_batch_chain`], which reduces the fold to a
//! [`ToolBatchPolicy`] the tool-planning loop consumes through
//! `ResolvedToolCatalog::decide_plan`'s existing `middleware` parameter. That
//! keeps the *one settlement per cursor* invariant intact: the batch still
//! settles exactly once, as the `ToolBatchPrepared` the planning loop builds.

mod apply;
mod codec;
mod driver;
mod input;
mod submit;
mod tool_batch;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::middleware::MiddlewareError;
use crate::run_types::RunHandleError;

#[cfg(test)]
pub(crate) use driver::settle_facade_stage_with_model;
pub(crate) use driver::{stage_driver, submit_command};
pub(crate) use submit::submit_folded;
pub(crate) use tool_batch::{ToolBatchPolicy, run_tool_batch_chain};

/// The run has no dispatch identity (locator/authorization) for a stage
/// invocation. Only reachable before `AcceptRun` commits, which no stage
/// settlement can precede.
const MIDDLEWARE_STAGE_IDENTITY_MISSING: &str = "middleware_stage_identity_missing";

/// The kernel state carries no payload the cursor's [`StageInput`] can be built
/// from (an `AfterModel` cursor with no messages, a `BeforeFinalize` cursor with
/// no terminal candidate, a stage this choke point does not fold).
const MIDDLEWARE_STAGE_INPUT_INVALID: &str = "middleware_stage_input_invalid";

/// A folded stage payload could not be canonicalized, parsed back, or rebuilt
/// into a valid kernel [`Message`]/[`AllocatedIds`].
const MIDDLEWARE_STAGE_PAYLOAD_INVALID: &str = "middleware_stage_payload_invalid";

pub(super) fn stage_error(code: &'static str) -> RunHandleError {
    RunHandleError::Middleware {
        code: Arc::from(code),
    }
}

pub(super) fn middleware_error(error: &MiddlewareError) -> RunHandleError {
    RunHandleError::Middleware {
        code: Arc::from(error.code()),
    }
}
