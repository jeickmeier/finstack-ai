//! Frozen, journal-backed evaluation with append-only experiment storage.

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

#[cfg(feature = "native-tokio")]
mod async_store;
mod attempt;
#[cfg(feature = "native-tokio")]
mod budget;
mod cell;
mod config;
mod error;
#[cfg(feature = "native-tokio")]
mod execution;
mod grader_record;
#[cfg(feature = "native-tokio")]
mod grading;
#[cfg(feature = "native-tokio")]
mod judge;
#[cfg(feature = "native-tokio")]
mod measurement;
#[path = "scorers/numeric.rs"]
pub mod numeric;
pub mod report;
#[cfg(feature = "native-tokio")]
mod runner;
mod score;
#[cfg(feature = "native-tokio")]
pub mod scorers;
#[cfg(feature = "native-tokio")]
mod scoring;
#[cfg(feature = "native-tokio")]
mod scoring_session;
pub mod store;
#[cfg(feature = "native-tokio")]
mod subject;

pub use attempt::*;
pub use cell::*;
pub use config::*;
pub use error::*;
pub use grader_record::{GraderOutcome, GraderRecord, GraderReservation};
#[cfg(feature = "native-tokio")]
pub use grading::GraderExecution;
#[cfg(feature = "native-tokio")]
pub use judge::{JudgeRubric, JudgeScorer};
#[cfg(feature = "native-tokio")]
pub use measurement::{ReconciledAttempt, reconcile_attempt};
pub use report::{
    Aggregate, EvalReport, GateResult, MetricKey, OutcomeCounts, PairedComparison, PairedCost,
    PairedMetric, Spending, Statistics, ThresholdGate, export_jsonl, reduce_repetitions,
};
#[cfg(feature = "native-tokio")]
pub use runner::{EvalRunReport, EvalRunner};
pub use score::{Score, ScoreMicros, ScoreSet};
#[cfg(feature = "native-tokio")]
pub use scorers::*;
#[cfg(feature = "native-tokio")]
pub use scoring::{ScoreContext, Scorer};
#[cfg(feature = "sqlite")]
pub use store::sqlite::SqliteEvalStore;
pub use store::{EvalStore, FrozenExperiment, MemoryEvalStore, StoreSnapshot};
#[cfg(feature = "native-tokio")]
pub use subject::{PreparedAttempt, SharedSubject, Subject, SubjectBinding};
