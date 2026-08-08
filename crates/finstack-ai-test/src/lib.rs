//! Scripted models, fakes, and conformance utilities for `finstack-ai`.
//!
//! Phase 0 (PR-005) provides the golden-trace fixture language, a target-neutral
//! conformance runner, and Criterion benchmark helpers. PR-006 adds deterministic
//! clock/random fakes and the `public-rust-api` fixture runner. PR-007 extends that
//! runner with content-block, blob-ref, and message subjects.

#![warn(missing_docs)]

mod conformance;
mod fakes;
mod message_fixture;
mod paths;
mod public_api_fixture;
mod scripted_model;
mod trace_fixture;

pub use conformance::{
    AdapterCapability, AdapterOutcome, ConformanceAdapter, ConformanceReport, ConformanceRunner,
    DeferredBindingAdapter, NoOpRustAdapter, TargetKind,
};
pub use fakes::{FixedClock, PatternRandomSource};
pub use paths::{compatibility_fixture, repo_root, schema_path};
pub use public_api_fixture::{
    Expect, PublicApiFixture, PublicApiFixtureError, Recipe, discover_public_api_fixtures,
    load_public_api_fixture, run_all_public_api_fixtures, run_public_api_fixture,
};
pub use scripted_model::{ScriptedInput, ScriptedStep, ScriptedStepKind};
pub use trace_fixture::{
    DurabilityClass, EffectExpectation, ExpectedTrace, GoldenTrace, NormalizedEvent,
    PayloadDeclaration, TraceError, TraceRecord, TransitionEnv, compare_normalized_bytes,
    load_golden_trace, load_noop_trace, normalize_json_value, validate_against_schema,
};
