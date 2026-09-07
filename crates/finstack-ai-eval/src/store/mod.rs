//! One-experiment stores with append-only mutations and a single runner lease.
mod graders;
mod memory;
#[cfg(feature = "sqlite")]
pub mod sqlite;
mod state;

use crate::{
    AttemptRecord, AttemptReservation, Cell, EvalError, EvalSpec, ExecutionIdentity, ScoreSet,
};
use finstack_ai_kernel::Digest;
pub use memory::MemoryEvalStore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Immutable experiment identity and engine provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenExperiment {
    /// Validated data-only specification. Exporters omit task bodies.
    pub spec: EvalSpec,
    /// Domain-separated canonical specification digest.
    pub digest: Digest,
    /// Engine version at first freeze.
    pub engine_version: Arc<str>,
}

/// Reconstructed store state; mutable copies cannot change persisted evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreSnapshot {
    /// Frozen spec, absent in a new empty store.
    pub frozen: Option<FrozenExperiment>,
    /// First resolved credential-free lock for every subject.
    pub subject_locks: BTreeMap<Arc<str>, Digest>,
    /// Durable admissions in per-cell sequence order.
    pub reservations: BTreeMap<Arc<str>, Vec<AttemptReservation>>,
    /// Results, including reconciliation projections and appended scoring passes.
    pub attempts: BTreeMap<Arc<str>, Vec<AttemptRecord>>,
    /// Grader admissions and spending, including failed or interrupted scoring passes.
    #[serde(default)]
    pub graders: BTreeMap<Arc<str>, crate::GraderRecord>,
}

/// RAII runner ownership. Dropping the guard releases memory or OS file ownership.
/// It does not cancel admitted executions; the runner settles them first.
pub trait RunnerLease: Send {}

/// Append-only experiment storage, not a kernel port.
/// Implementations acknowledge only validated mutations; `SQLite` uses FULL durability.
pub trait EvalStore: Send + Sync {
    /// Acquire exclusive runner ownership across the store's complete run/rescore operation.
    /// # Errors
    /// Returns `eval_runner_busy` if another owner is active.
    fn acquire_runner(&self) -> Result<Box<dyn RunnerLease>, EvalError>;
    /// Freeze or verify an equal immutable specification.
    /// # Errors
    /// Returns invalid/diverged specification or storage errors.
    fn freeze(&self, spec: &EvalSpec) -> Result<FrozenExperiment, EvalError>;
    /// Pin a subject lock; an equal repeated binding is idempotent.
    /// # Errors
    /// Returns unbound or mismatched subject errors.
    fn bind_subject(&self, subject: &str, digest: Digest) -> Result<(), EvalError>;
    /// Reserve the next sequence before preparation. No replacement of unresolved work.
    /// # Errors
    /// Returns sequence conflict, finalized, unresolved, or bound errors.
    fn reserve(
        &self,
        cell: &Cell,
        sequence: u32,
        started_at_ms: u64,
    ) -> Result<AttemptReservation, EvalError>;
    /// Persist the actual fresh session/lane before subject dispatch, exactly once.
    /// # Errors
    /// Returns conflicting identity, duplicate session or missing reservation.
    fn bind_execution(
        &self,
        cell: &str,
        sequence: u32,
        identity: ExecutionIdentity,
    ) -> Result<(), EvalError>;
    /// Settle a reservation; an unresolved result may later receive reconciliation.
    /// # Errors
    /// Returns invalid sequence, contradictory classification, or immutable result conflict.
    fn settle(&self, record: &AttemptRecord) -> Result<(), EvalError>;
    /// Append an independent immutable scoring pass after subject settlement.
    /// # Errors
    /// Returns missing result or invalid/oversized scorer output.
    fn append_scores(&self, cell: &str, sequence: u32, scores: &ScoreSet) -> Result<(), EvalError>;
    /// Reserve one grader before session creation or external dispatch.
    /// # Errors
    /// Returns missing/final pass conflicts, invalid binding or storage errors.
    fn reserve_grader(&self, reservation: &crate::GraderReservation) -> Result<(), EvalError>;
    /// Bind the actual fresh grader session/lane before dispatch.
    /// # Errors
    /// Returns missing or conflicting identity or reused execution session.
    fn bind_grader_execution(
        &self,
        key: &str,
        identity: ExecutionIdentity,
    ) -> Result<(), EvalError>;
    /// Append grader outcome, retaining spend even if its scoring pass fails.
    /// # Errors
    /// Returns conflicting outcome, missing reservation or invalid reconciliation.
    fn settle_grader(&self, key: &str, outcome: &crate::GraderOutcome) -> Result<(), EvalError>;
    /// Load a bounded reconstruction of every acknowledged mutation.
    /// # Errors
    /// Returns store unavailability or corruption.
    fn snapshot(&self) -> Result<StoreSnapshot, EvalError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Mutation {
    Freeze {
        experiment: FrozenExperiment,
    },
    BindSubject {
        subject: Arc<str>,
        digest: Digest,
    },
    Reserve {
        reservation: AttemptReservation,
    },
    BindExecution {
        cell: Arc<str>,
        sequence: u32,
        identity: ExecutionIdentity,
    },
    Settle {
        record: AttemptRecord,
    },
    GraderReserve {
        reservation: crate::GraderReservation,
    },
    GraderBind {
        key: Arc<str>,
        identity: ExecutionIdentity,
    },
    GraderSettle {
        key: Arc<str>,
        outcome: crate::GraderOutcome,
    },
    Scores {
        cell: Arc<str>,
        sequence: u32,
        scores: ScoreSet,
    },
}

pub(crate) fn freeze_mutation(spec: &EvalSpec) -> Result<Mutation, EvalError> {
    Ok(Mutation::Freeze {
        experiment: FrozenExperiment {
            spec: spec.clone(),
            digest: spec.digest()?,
            engine_version: Arc::from(env!("CARGO_PKG_VERSION")),
        },
    })
}

// Each concrete store implements one checked append path. Forwarding the public
// mutations keeps validation and replay identical across memory and SQLite.
macro_rules! mutation_methods {
    () => {
        fn freeze(
            &self,
            spec: &crate::EvalSpec,
        ) -> Result<super::FrozenExperiment, crate::EvalError> {
            let mutation = super::freeze_mutation(spec)?;
            self.append(mutation)?;
            self.snapshot()?
                .frozen
                .ok_or_else(crate::error::unavailable)
        }
        fn bind_subject(
            &self,
            subject: &str,
            digest: finstack_ai_kernel::Digest,
        ) -> Result<(), crate::EvalError> {
            self.append(super::Mutation::BindSubject {
                subject: std::sync::Arc::from(subject),
                digest,
            })
        }
        fn reserve(
            &self,
            cell: &crate::Cell,
            sequence: u32,
            started_at_ms: u64,
        ) -> Result<crate::AttemptReservation, crate::EvalError> {
            let reservation = crate::AttemptReservation {
                cell: cell.clone(),
                sequence,
                started_at_ms,
                execution: None,
            };
            self.append(super::Mutation::Reserve {
                reservation: reservation.clone(),
            })?;
            Ok(reservation)
        }
        fn bind_execution(
            &self,
            cell: &str,
            sequence: u32,
            identity: crate::ExecutionIdentity,
        ) -> Result<(), crate::EvalError> {
            self.append(super::Mutation::BindExecution {
                cell: std::sync::Arc::from(cell),
                sequence,
                identity,
            })
        }
        fn settle(&self, record: &crate::AttemptRecord) -> Result<(), crate::EvalError> {
            self.append(super::Mutation::Settle {
                record: record.clone(),
            })
        }
        fn reserve_grader(
            &self,
            reservation: &crate::GraderReservation,
        ) -> Result<(), crate::EvalError> {
            self.append(super::Mutation::GraderReserve {
                reservation: reservation.clone(),
            })
        }
        fn bind_grader_execution(
            &self,
            key: &str,
            identity: crate::ExecutionIdentity,
        ) -> Result<(), crate::EvalError> {
            self.append(super::Mutation::GraderBind {
                key: std::sync::Arc::from(key),
                identity,
            })
        }
        fn settle_grader(
            &self,
            key: &str,
            outcome: &crate::GraderOutcome,
        ) -> Result<(), crate::EvalError> {
            self.append(super::Mutation::GraderSettle {
                key: std::sync::Arc::from(key),
                outcome: outcome.clone(),
            })
        }
        fn append_scores(
            &self,
            cell: &str,
            sequence: u32,
            scores: &crate::ScoreSet,
        ) -> Result<(), crate::EvalError> {
            self.append(super::Mutation::Scores {
                cell: std::sync::Arc::from(cell),
                sequence,
                scores: scores.clone(),
            })
        }
    };
}
pub(crate) use mutation_methods;
