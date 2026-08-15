//! Scripted models, fakes, and conformance utilities for `finstack-ai`.
//!
//! Phase 0 (PR-005) provides the golden-trace fixture language, a target-neutral
//! conformance runner, and Criterion benchmark helpers. PR-006 adds deterministic
//! clock/random fakes and the `public-rust-api` fixture runner. PR-007 extends that
//! runner with content-block, blob-ref, and message subjects. PR-008 adds
//! run/effect/record/event subjects. PR-009 adds a real reducer-backed adapter.

#![warn(missing_docs)]

mod compaction_conformance;
mod conformance;
mod crash_prefix;
mod fakes;
mod golden_scenarios;
mod journal_bodies;
mod journal_fixture;
mod message_fixture;
mod paths;
mod port_conformance;
mod pr008_fixture;
mod pr009_fixture;
mod public_api_fixture;
mod reducer_fixture;
mod scripted_extensions;
mod scripted_model;
mod scripted_toolset;
mod trace_fixture;

pub use compaction_conformance::{
    CompactionConformanceCase, CompactionConformanceReport, SharedCompactionProjection,
    check_compaction_conformance,
};
pub use conformance::{
    AdapterCapability, AdapterOutcome, ConformanceAdapter, ConformanceReport, ConformanceRunner,
    DeferredBindingAdapter, NoOpRustAdapter, TargetKind,
};
pub use crash_prefix::{LegalRestore, classify_phase};
pub use fakes::{
    DeterministicIdSource, FixedClock, ManualClock, ManualGate, PatternRandomSource,
    SequenceRandomSource,
};
pub use golden_scenarios::{
    GoldenScenario, GoldenScenarioId, GoldenScenarioSuite, load_golden_scenarios,
};
pub use journal_bodies::{all_activated_record_bodies, draft_for_body};
pub use journal_fixture::{
    JournalExpect, JournalFixture, JournalFixtureError, JournalRecipe,
    discover_journal_v1_fixtures, known_answer_for_body, known_answer_for_envelope,
    load_journal_fixture, run_journal_fixture, run_journal_v1_corpus, write_journal_v1_fixtures,
};
pub use paths::{compatibility_fixture, repo_root, schema_path};
pub use port_conformance::{
    ContextConformanceCase, JournalStoreConformanceCase, MiddlewareConformanceCase,
    ModelConformanceCase, PORT_CONFORMANCE_SUITE_VERSION, PortConformanceFailure,
    ToolsetConformanceCase, check_context_conformance, check_journal_store_conformance,
    check_middleware_conformance, check_model_conformance, check_observer_conformance,
    check_toolset_conformance,
};
pub use public_api_fixture::{
    Expect, PublicApiFixture, PublicApiFixtureError, Recipe, discover_public_api_fixtures,
    load_public_api_fixture, run_all_public_api_fixtures, run_public_api_fixture,
};
pub use reducer_fixture::{
    ReducerExecution, ReducerRustAdapter, ReducerTerminalProjection, execute_reducer_trace,
    project_reducer_terminal,
};
pub use scripted_extensions::{
    AmbiguousAckAfterCommitStore, FaultJournalStore, ScriptedContextAction,
    ScriptedContextProvider, ScriptedMiddleware, ScriptedMiddlewareAction, ScriptedObserver,
    ScriptedObserverAction, StoreOperation,
};
pub use scripted_model::{
    ScriptedInput, ScriptedModel, ScriptedModelAction, ScriptedModelControl, ScriptedModelPlan,
    ScriptedStep, ScriptedStepKind,
};
pub use scripted_toolset::{
    ScriptedToolAction, ScriptedToolPlan, ScriptedToolset, ScriptedToolsetControl,
};
pub use trace_fixture::{
    DurabilityClass, EffectExpectation, ExpectedTrace, GoldenTrace, NormalizedEvent,
    PayloadDeclaration, TraceError, TraceRecord, TransitionEnv, compare_normalized_bytes,
    load_golden_trace, load_noop_trace, normalize_json_value, validate_against_schema,
};
