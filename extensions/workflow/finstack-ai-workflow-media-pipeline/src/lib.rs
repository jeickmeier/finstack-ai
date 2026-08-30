//! MoviePlan pipeline driver: adapter-journaled, tick-based, resumable.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]
// The module doc above intentionally mirrors the brief's wording verbatim.
#![allow(clippy::doc_markdown)]

mod plan;
mod state;
mod subtitles;

pub use plan::{
    CaptionCue, CaptionsMode, FrameSource, MoviePlan, PlanAudio, PlanBudget, PlanDefaults,
    PlanLimits, PlanOutput, PlanTransition, SceneOverrides, SceneSpec, TransitionKindName,
    validate_plan,
};
pub use state::{
    MemoryRenderStateStore, RenderState, RenderStateStore, RenderStatus, SceneStage, SceneState,
    SqliteRenderStateStore, StateError,
};
