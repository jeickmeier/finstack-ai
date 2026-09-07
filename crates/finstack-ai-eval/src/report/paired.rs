//! Pair exact task/repetition coordinates; unmatched observations remain explicit.
use super::{ExecutionIndex, MetricIndex, MetricKey, Statistics};
use crate::EvalError;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Raw per-cell candidate-minus-baseline grade differences (no repetition reducer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedMetric {
    /// Scorer/version/name shared by both arms.
    pub metric: MetricKey,
    /// Delta statistics in normalized score units.
    pub delta: Statistics,
    /// Pairs with a strictly higher candidate grade.
    pub wins: u32,
    /// Pairs with exactly equal integer grades.
    pub ties: u32,
    /// Pairs with a strictly lower candidate grade.
    pub losses: u32,
    /// Grades present only on the baseline arm.
    pub baseline_only: u32,
    /// Grades present only on the candidate arm.
    pub candidate_only: u32,
    /// Expected pairs minus actual matched grades, including both arms missing.
    pub missing_pairs: u32,
}
/// Cost differences where both executions have complete cost in the same unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedCost {
    /// Common cost unit for this group.
    pub unit: Arc<str>,
    /// Exact signed sum of candidate-minus-baseline micros, as a decimal string.
    pub total_delta_micros: String,
    /// Approximate statistics in micros; exact individual costs remain in the export/store.
    pub delta_micros: Statistics,
}
/// Paired arm comparison. Usage refers to subjects (including children), excluding graders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairedComparison {
    /// Reference subject arm.
    pub baseline: Arc<str>,
    /// Subject arm compared against the reference (candidate minus baseline).
    pub candidate: Arc<str>,
    /// Frozen task × repetition pairs.
    pub expected_pairs: u32,
    /// Pairs with authoritative subject outcomes on both arms.
    pub execution_pairs: u32,
    /// Quality comparisons by exact metric identity.
    pub metrics: Vec<PairedMetric>,
    /// Known input-token deltas; n exposes coverage.
    pub input_token_delta: Statistics,
    /// Known output-token deltas; n exposes coverage.
    pub output_token_delta: Statistics,
    /// Known total-token deltas; n exposes coverage.
    pub total_token_delta: Statistics,
    /// Unit-separated costs with matched complete coverage.
    pub costs: Vec<PairedCost>,
    /// Expected pairs without a compatible, complete cost pair.
    pub missing_cost_pairs: u32,
}

pub(super) fn compare(
    baseline: &Arc<str>,
    candidate: &Arc<str>,
    metrics: &MetricIndex,
    executions: &ExecutionIndex<'_>,
    expected_pairs: u32,
) -> Result<PairedComparison, EvalError> {
    let paired_metrics = compare_metrics(baseline, candidate, metrics, expected_pairs)?;
    let mut execution_pairs = 0;
    let (mut input, mut output, mut tokens) = (Vec::new(), Vec::new(), Vec::new());
    let mut costs: BTreeMap<Arc<str>, (i128, Vec<f64>)> = BTreeMap::new();
    let mut known_cost_pairs = 0;
    if let (Some(left), Some(right)) = (executions.get(baseline), executions.get(candidate)) {
        for (coordinate, left) in left {
            let Some(right) = right.get(coordinate) else {
                continue;
            };
            execution_pairs += 1;
            if !left.usage.complete || !right.usage.complete {
                continue;
            }
            for (a, b, deltas) in [
                (
                    left.usage.input_tokens,
                    right.usage.input_tokens,
                    &mut input,
                ),
                (
                    left.usage.output_tokens,
                    right.usage.output_tokens,
                    &mut output,
                ),
                (
                    left.usage.total_tokens,
                    right.usage.total_tokens,
                    &mut tokens,
                ),
            ] {
                if let (Some(a), Some(b)) = (a, b) {
                    deltas.push(approximate(i128::from(b) - i128::from(a)));
                }
            }
            if let (Some(a), Some(b)) = (&left.usage.cost, &right.usage.cost)
                && a.unit == b.unit
            {
                let delta = i128::from(b.micros) - i128::from(a.micros);
                let (sum, values) = costs.entry(Arc::clone(&a.unit)).or_default();
                *sum += delta; // At most 100,000 pairs of u64 values, safely within i128.
                values.push(approximate(delta));
                known_cost_pairs += 1;
            }
        }
    }
    let costs = costs
        .into_iter()
        .map(|(unit, (sum, values))| {
            Ok(PairedCost {
                unit,
                total_delta_micros: sum.to_string(),
                delta_micros: Statistics::from_values(values)?,
            })
        })
        .collect::<Result<_, EvalError>>()?;
    Ok(PairedComparison {
        baseline: Arc::clone(baseline),
        candidate: Arc::clone(candidate),
        expected_pairs,
        execution_pairs,
        metrics: paired_metrics,
        input_token_delta: Statistics::from_values(input)?,
        output_token_delta: Statistics::from_values(output)?,
        total_token_delta: Statistics::from_values(tokens)?,
        costs,
        missing_cost_pairs: expected_pairs - known_cost_pairs,
    })
}

fn compare_metrics(
    baseline: &Arc<str>,
    candidate: &Arc<str>,
    metrics: &MetricIndex,
    expected_pairs: u32,
) -> Result<Vec<PairedMetric>, EvalError> {
    let keys: BTreeSet<_> = metrics
        .keys()
        .filter(|(subject, _)| subject == baseline || subject == candidate)
        .map(|(_, key)| key)
        .collect();
    let empty = BTreeMap::new();
    let mut paired_metrics = Vec::new();
    for key in keys {
        let left = metrics
            .get(&(Arc::clone(baseline), key.clone()))
            .unwrap_or(&empty);
        let right = metrics
            .get(&(Arc::clone(candidate), key.clone()))
            .unwrap_or(&empty);
        let (mut wins, mut ties, mut losses) = (0, 0, 0);
        let mut deltas = Vec::new();
        for (coordinate, value) in left {
            if let Some(other) = right.get(coordinate) {
                match other.cmp(value) {
                    std::cmp::Ordering::Greater => wins += 1,
                    std::cmp::Ordering::Equal => ties += 1,
                    std::cmp::Ordering::Less => losses += 1,
                }
                deltas.push((f64::from(other.get()) - f64::from(value.get())) / 1_000_000.0);
            }
        }
        let delta = Statistics::from_values(deltas)?;
        paired_metrics.push(PairedMetric {
            metric: key.clone(),
            wins,
            ties,
            losses,
            baseline_only: super::count(left.len())? - delta.n,
            candidate_only: super::count(right.len())? - delta.n,
            missing_pairs: expected_pairs - delta.n,
            delta,
        });
    }
    Ok(paired_metrics)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "Report statistics are approximate; the exact signed cost sum is exported separately"
)]
fn approximate(value: i128) -> f64 {
    value as f64
}
