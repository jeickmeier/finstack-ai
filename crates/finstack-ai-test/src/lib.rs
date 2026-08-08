//! Scripted models, fakes, and conformance utilities for `finstack-ai`.
//!
//! Phase 0 (PR-005) provides the golden-trace fixture language, a target-neutral
//! conformance runner, and Criterion benchmark helpers. Semantic reducer
//! execution and cross-language parity arrive in later phases.

#![warn(missing_docs)]

mod conformance;
mod paths;
mod scripted_model;
mod trace_fixture;

pub use conformance::{
    AdapterCapability, AdapterOutcome, ConformanceAdapter, ConformanceReport, ConformanceRunner,
    DeferredBindingAdapter, NoOpRustAdapter, TargetKind,
};
pub use paths::{compatibility_fixture, repo_root, schema_path};
pub use scripted_model::{ScriptedInput, ScriptedStep, ScriptedStepKind};
pub use trace_fixture::{
    DurabilityClass, EffectExpectation, ExpectedTrace, GoldenTrace, NormalizedEvent,
    PayloadDeclaration, TraceError, TraceRecord, TransitionEnv, compare_normalized_bytes,
    load_golden_trace, load_noop_trace, normalize_json_value, validate_against_schema,
};
