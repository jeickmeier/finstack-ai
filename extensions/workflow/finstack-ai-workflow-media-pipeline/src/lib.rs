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

mod driver;
mod plan;
mod state;
mod subtitles;

pub use driver::{
    MEDIA_PIPELINE_BUDGET_EXCEEDED, MEDIA_PIPELINE_CONFIG_INVALID,
    MEDIA_PIPELINE_INVALID_ARGUMENTS, MEDIA_PIPELINE_NOT_FOUND, MEDIA_PIPELINE_PLAN_INVALID,
    MEDIA_PIPELINE_STAGE_FAILED, MEDIA_PIPELINE_STORE_FAILURE, MediaPipelineConfig,
    MediaPipelineDriver, PipelineError,
};
pub use plan::{
    CaptionCue, CaptionsMode, FrameSource, MoviePlan, PlanAudio, PlanBudget, PlanDefaults,
    PlanLimits, PlanOutput, PlanTransition, SceneOverrides, SceneSpec, TransitionKindName,
    validate_plan,
};
pub use state::{
    MemoryRenderStateStore, RenderState, RenderStateStore, RenderStatus, SceneStage, SceneState,
    SqliteRenderStateStore, StateError,
};
