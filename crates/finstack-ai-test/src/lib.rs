//! Scripted models, fakes, and conformance utilities for `finstack-ai`.
//!
//! Phase 0 (golden-trace baseline) provides the golden-trace fixture language, a target-neutral
//! conformance runner, and Criterion benchmark helpers. public-API fixture baseline adds deterministic
//! clock/random fakes and the `public-rust-api` fixture runner. content fixture baseline extends that
//! runner with content-block, blob-ref, and message subjects. record-and-event fixture baseline adds
//! run/effect/record/event subjects. model-only reducer baseline adds a real reducer-backed adapter.
//!
//! # Module map
//!
//! - `paths` — repository, schema, and compatibility fixture paths
//! - `crash_prefix` — legal restore classification
//! - `fakes` — deterministic clocks, random sources, and `ManualGate`
//! - `scripted` — scripted model, toolset, and extension doubles
//! - `fixtures` — golden traces, public-API subjects, and journal corpus
//! - `conformance` — target-neutral runner, port checks, compaction, reducer

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

mod conformance;
mod crash_prefix;
mod fakes;
mod fixtures;
mod paths;
mod scripted;

pub use conformance::compaction::{
    CompactionConformanceCase, CompactionConformanceReport, SharedCompactionProjection,
    check_compaction_conformance,
};
pub use conformance::ports::{
    ContextConformanceCase, JournalStoreConformanceCase, MiddlewareConformanceCase,
    ModelConformanceCase, PORT_CONFORMANCE_SUITE_VERSION, PortConformanceFailure,
    ToolsetConformanceCase, check_context_conformance, check_journal_store_conformance,
    check_middleware_conformance, check_model_conformance, check_observer_conformance,
    check_toolset_conformance,
};
pub use conformance::reducer::{
    ReducerExecution, ReducerRustAdapter, ReducerTerminalProjection, execute_reducer_trace,
    project_reducer_terminal,
};
pub use conformance::runner::{
    AdapterCapability, AdapterOutcome, ConformanceAdapter, ConformanceReport, ConformanceRunner,
    DeferredBindingAdapter, NoOpRustAdapter, TargetKind,
};
pub use crash_prefix::{LegalRestore, classify_phase};
pub use fakes::{FixedClock, ManualClock, ManualGate, PatternRandomSource};
pub use fixtures::golden_scenarios::{
    GoldenScenario, GoldenScenarioId, GoldenScenarioSuite, load_golden_scenarios,
};
pub use fixtures::journal::bodies::{all_activated_record_bodies, draft_for_body};
pub use fixtures::journal::generate::write_journal_v1_fixtures;
pub use fixtures::journal::runner::{
    JournalExpect, JournalFixture, JournalFixtureError, JournalRecipe,
    discover_journal_v1_fixtures, known_answer_for_body, known_answer_for_envelope,
    load_journal_fixture, run_journal_fixture, run_journal_v1_corpus,
};
pub use fixtures::public_api::{
    Expect, PublicApiFixture, PublicApiFixtureError, Recipe, discover_public_api_fixtures,
    load_public_api_fixture, run_all_public_api_fixtures, run_public_api_fixture,
};
pub use fixtures::store as store_fixtures;
pub use fixtures::trace::{
    DurabilityClass, EffectExpectation, ExpectedTrace, GoldenTrace, NormalizedEvent,
    PayloadDeclaration, TraceError, TraceRecord, TransitionEnv, compare_normalized_bytes,
    load_golden_trace, load_noop_trace, normalize_json_value, validate_against_schema,
};
pub use paths::{compatibility_fixture, repo_root, schema_path};
pub use scripted::extensions::{
    AmbiguousAckAfterCommitStore, FaultJournalStore, ScriptedContextAction,
    ScriptedContextProvider, ScriptedMiddleware, ScriptedMiddlewareAction, ScriptedObserver,
    ScriptedObserverAction, StoreOperation,
};
pub use scripted::model::{
    ScriptedInput, ScriptedModel, ScriptedModelAction, ScriptedModelControl, ScriptedModelPlan,
    ScriptedStep, ScriptedStepKind,
};
pub use scripted::toolset::{
    ScriptedToolAction, ScriptedToolPlan, ScriptedToolset, ScriptedToolsetControl,
};
