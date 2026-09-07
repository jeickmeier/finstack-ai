//! Deterministic body-free reports projected from acknowledged experiment state.
mod export;
mod gate;
mod paired;
mod statistics;
use crate::{
    AttemptRecord, AttemptStatus, EvalError, MeasuredUsage, Reconciliation, ScoreMicros,
    StoreSnapshot,
};
pub use export::export_jsonl;
use finstack_ai_kernel::Digest;
pub use gate::{GateResult, ThresholdGate};
pub use paired::{PairedComparison, PairedCost, PairedMetric};
use serde::{Deserialize, Serialize};
pub use statistics::{Statistics, reduce_repetitions};
use std::{collections::BTreeMap, sync::Arc};

/// Stable metric identity; versions and result names never mix.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricKey {
    /// Declared scorer identifier.
    pub scorer: Arc<str>,
    /// Scoring behavior version.
    pub scorer_version: u32,
    /// Named score (for example a structured field).
    pub name: Arc<str>,
}
/// Task-reduced quality and explicit repetition/task coverage for one subject metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    /// Subject arm.
    pub subject: Arc<str>,
    /// Exact scorer/version/name identity.
    pub metric: MetricKey,
    /// Statistics across task reductions, in normalized score units.
    pub statistics: Statistics,
    /// Expected tasks in the frozen dataset.
    pub expected_tasks: u32,
    /// Tasks with every repetition scored by this exact metric version.
    pub complete_tasks: u32,
    /// Number of observed repetition grades, including partial tasks.
    pub scored_cells: u32,
    /// Expected repetitions across all tasks for this arm.
    pub expected_cells: u32,
}
/// Current cell classifications plus historical retries and independent scoring failures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeCounts {
    /// Total frozen cells.
    pub expected: u32,
    /// Successful authoritative subjects.
    pub completed: u32,
    /// Terminal failed subjects, which scorers may grade as quality failures.
    pub subject_failed: u32,
    /// Cells currently ending in a proven non-admission infrastructure failure.
    pub infrastructure_failed: u32,
    /// Cells whose admitted work or effects remain unresolved.
    pub indeterminate: u32,
    /// Reservations without an outcome.
    pub pending: u32,
    /// Cells with no reservation yet.
    pub unattempted: u32,
    /// All admitted attempt reservations, including replacements.
    pub attempts: u32,
    /// Historical proven non-admission failures, including superseded attempts.
    pub infrastructure_attempts: u32,
    /// Cells with at least one failed latest scorer pass.
    pub scoring_failed: u32,
    /// Final cells missing a pass for at least one declared scorer.
    pub scoring_pending: u32,
    /// Unsettled or indeterminate grader admissions.
    pub graders_pending: u32,
}
/// Experiment spending including historical failed attempts, in exact decimal micros.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spending {
    /// Known subject subtotals per unit. Strings retain exact u64 values in JSON readers.
    pub subject_micros: BTreeMap<Arc<str>, String>,
    /// Known grader subtotals per unit, including failed scoring passes.
    pub grader_micros: BTreeMap<Arc<str>, String>,
    /// Combined known subtotals; distinct units are never added together.
    pub total_micros: BTreeMap<Arc<str>, String>,
    /// Number of admitted subject executions lacking complete cost measurement.
    pub subject_unknown: u32,
    /// Number of admitted grader executions lacking complete cost measurement.
    pub grader_unknown: u32,
}
/// Body-free experiment report; paired comparisons use the first declared subject as baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalReport {
    /// Frozen specification digest.
    pub spec_digest: Digest,
    /// Engine provenance from first freeze.
    pub engine_version: Arc<str>,
    /// Bound credential-free subject locks.
    pub subject_locks: BTreeMap<Arc<str>, Digest>,
    /// Current execution/scoring coverage.
    pub counts: OutcomeCounts,
    /// Separate subject and grader spending.
    pub spending: Spending,
    /// Quality grouped by exact version and field.
    pub aggregates: Vec<Aggregate>,
    /// Candidate-minus-baseline paired quality, token and cost differences.
    pub comparisons: Vec<PairedComparison>,
}

type Coordinate = (Arc<str>, u32);
type Observations = BTreeMap<Coordinate, ScoreMicros>;
type MetricIndex = BTreeMap<(Arc<str>, MetricKey), Observations>;
type ExecutionIndex<'a> = BTreeMap<Arc<str>, BTreeMap<Coordinate, &'a AttemptRecord>>;

pub(super) fn authoritative(records: &[AttemptRecord]) -> Option<&AttemptRecord> {
    records.iter().find(|record| record.status.is_final())
}
pub(super) fn latest_scores(record: &AttemptRecord) -> BTreeMap<&str, &crate::ScoreSet> {
    record
        .scores
        .iter()
        .map(|pass| (pass.scorer.as_ref(), pass))
        .collect()
}
impl EvalReport {
    /// Project stored evidence without reading journals or invoking any subject/scorer.
    /// Partial data remains visible; it cannot silently become complete coverage.
    /// # Errors
    /// Returns invalid/unfrozen snapshots or overflow of exact cost sums.
    pub fn from_snapshot(snapshot: &StoreSnapshot) -> Result<Self, EvalError> {
        let frozen = snapshot.frozen.as_ref().ok_or_else(crate::error::invalid)?;
        if frozen.spec.digest()? != frozen.digest {
            return Err(crate::error::invalid());
        }
        let spec = &frozen.spec;
        let cells = spec.cells()?;
        let mut counts = OutcomeCounts {
            expected: count(cells.len())?,
            ..OutcomeCounts::default()
        };
        let mut metrics = MetricIndex::new();
        let mut executions = ExecutionIndex::new();
        for cell in &cells {
            let records = snapshot
                .attempts
                .get(&cell.id)
                .map_or(&[][..], Vec::as_slice);
            let reservations = snapshot
                .reservations
                .get(&cell.id)
                .map_or(&[][..], Vec::as_slice);
            if reservations.len() > 17 || records.len() > reservations.len() {
                return Err(crate::error::invalid());
            }
            counts.attempts += count(reservations.len())?;
            counts.infrastructure_attempts += count(
                records
                    .iter()
                    .filter(|r| r.status == AttemptStatus::InfraFailed)
                    .count(),
            )?;
            let current = authoritative(records).or_else(|| {
                reservations
                    .last()
                    .and_then(|r| records.iter().find(|record| record.sequence == r.sequence))
            });
            let Some(record) = current else {
                if reservations.is_empty() {
                    counts.unattempted += 1;
                } else {
                    counts.pending += 1;
                }
                continue;
            };
            match record.status {
                AttemptStatus::Completed => counts.completed += 1,
                AttemptStatus::SubjectFailed => counts.subject_failed += 1,
                AttemptStatus::InfraFailed => counts.infrastructure_failed += 1,
                AttemptStatus::Indeterminate => counts.indeterminate += 1,
            }
            if !record.status.is_final() {
                continue;
            }
            let coordinate = (Arc::clone(&cell.task_id), cell.repetition);
            executions
                .entry(Arc::clone(&cell.subject_id))
                .or_default()
                .insert(coordinate.clone(), record);
            if record.scores.len() > 256 {
                return Err(crate::error::invalid());
            }
            let latest = latest_scores(record);
            counts.scoring_failed +=
                u32::from(latest.values().any(|pass| pass.failure_code.is_some()));
            counts.scoring_pending += u32::from(
                spec.scorers
                    .iter()
                    .any(|id| !latest.contains_key(id.as_ref())),
            );
            for pass in latest.values() {
                pass.validate()?;
                for score in &pass.scores {
                    let key = MetricKey {
                        scorer: Arc::clone(&pass.scorer),
                        scorer_version: pass.scorer_version,
                        name: Arc::clone(&score.name),
                    };
                    metrics
                        .entry((Arc::clone(&cell.subject_id), key))
                        .or_default()
                        .insert(coordinate.clone(), score.value);
                }
            }
        }
        counts.graders_pending =
            u32::try_from(snapshot.graders.values().filter(|g| g.unresolved()).count())
                .map_err(|_| crate::error::invalid())?;
        let aggregates = aggregate_metrics(&metrics, spec)?;
        let comparisons = compare_baseline(spec, &metrics, &executions)?;
        Ok(Self {
            spec_digest: frozen.digest,
            engine_version: Arc::clone(&frozen.engine_version),
            subject_locks: snapshot.subject_locks.clone(),
            counts,
            spending: spending(snapshot)?,
            aggregates,
            comparisons,
        })
    }
}

fn spending(snapshot: &StoreSnapshot) -> Result<Spending, EvalError> {
    let (mut subject, mut grader) = (BTreeMap::new(), BTreeMap::new());
    let (mut subject_unknown, mut grader_unknown) = (0, 0);
    for reservations in snapshot.reservations.values() {
        for reservation in reservations {
            let record = snapshot
                .attempts
                .get(&reservation.cell.id)
                .and_then(|rs| rs.iter().find(|r| r.sequence == reservation.sequence));
            match record {
                Some(record) if record.reconciliation == Reconciliation::NoAdmission => {}
                Some(record) => {
                    fold_cost(&mut subject, &record.usage)?;
                    subject_unknown += u32::from(!known_cost(&record.usage));
                }
                None => subject_unknown += 1,
            }
        }
    }
    for record in snapshot.graders.values() {
        match &record.outcome {
            Some(outcome) if outcome.reconciliation == Reconciliation::NoAdmission => {}
            Some(outcome) => {
                fold_cost(&mut grader, &outcome.usage)?;
                grader_unknown += u32::from(!known_cost(&outcome.usage));
            }
            None => grader_unknown += 1,
        }
    }
    let mut total = subject.clone();
    for (unit, value) in &grader {
        add_cost(&mut total, unit, *value)?;
    }
    let strings = |values: BTreeMap<Arc<str>, u64>| {
        values
            .into_iter()
            .map(|(unit, value)| (unit, value.to_string()))
            .collect()
    };
    Ok(Spending {
        subject_micros: strings(subject),
        grader_micros: strings(grader),
        total_micros: strings(total),
        subject_unknown,
        grader_unknown,
    })
}
fn known_cost(usage: &MeasuredUsage) -> bool {
    usage.complete
        && usage.uncosted_effects == 0
        && (usage.cost.is_some() || (usage.effects == 0 && usage.cost_by_unit.is_empty()))
}
fn fold_cost(total: &mut BTreeMap<Arc<str>, u64>, usage: &MeasuredUsage) -> Result<(), EvalError> {
    usage.validate()?;
    for (unit, value) in &usage.cost_by_unit {
        add_cost(total, unit, *value)?;
    }
    Ok(())
}
fn add_cost(
    total: &mut BTreeMap<Arc<str>, u64>,
    unit: &Arc<str>,
    value: u64,
) -> Result<(), EvalError> {
    let amount = total.entry(Arc::clone(unit)).or_default();
    *amount = amount
        .checked_add(value)
        .ok_or_else(|| EvalError::new(crate::EVAL_ARITHMETIC_OVERFLOW, "report cost overflow"))?;
    Ok(())
}

fn aggregate_metrics(
    metrics: &MetricIndex,
    spec: &crate::EvalSpec,
) -> Result<Vec<Aggregate>, EvalError> {
    let mut aggregates = Vec::new();
    for ((subject, metric), observations) in metrics {
        let mut by_task: BTreeMap<&str, Vec<ScoreMicros>> = BTreeMap::new();
        for ((task, _), value) in observations {
            by_task.entry(task).or_default().push(*value);
        }
        let mut complete_tasks = 0;
        let mut reduced = Vec::new();
        for values in by_task.values() {
            complete_tasks += u32::from(values.len() == spec.repetitions as usize);
            if let Some(value) = reduce_repetitions(values, &spec.reducer)? {
                reduced.push(value);
            }
        }
        aggregates.push(Aggregate {
            subject: Arc::clone(subject),
            metric: metric.clone(),
            statistics: Statistics::from_values(reduced)?,
            expected_tasks: count(spec.tasks.len())?,
            complete_tasks,
            scored_cells: count(observations.len())?,
            expected_cells: count(spec.tasks.len())? * spec.repetitions,
        });
    }
    Ok(aggregates)
}

fn count(value: usize) -> Result<u32, EvalError> {
    u32::try_from(value).map_err(|_| crate::error::invalid())
}

fn compare_baseline(
    spec: &crate::EvalSpec,
    metrics: &MetricIndex,
    executions: &ExecutionIndex<'_>,
) -> Result<Vec<PairedComparison>, EvalError> {
    let mut comparisons = Vec::new();
    if let Some(baseline) = spec.subjects.first() {
        for candidate in spec.subjects.iter().skip(1) {
            comparisons.push(paired::compare(
                &baseline.subject_id,
                &candidate.subject_id,
                metrics,
                executions,
                count(spec.tasks.len())? * spec.repetitions,
            )?);
        }
    }
    Ok(comparisons)
}
