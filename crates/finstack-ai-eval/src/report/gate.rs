//! Quality thresholds never conceal infrastructure, scoring or coverage failures.
use super::{EvalReport, MetricKey};
use crate::{EvalError, ScoreMicros};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Threshold policy for one subject metric; baseline comparisons use the report's reference arm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThresholdGate {
    /// Candidate subject identifier.
    pub subject: Arc<str>,
    /// Exact scorer behavior and result field.
    pub metric: MetricKey,
    /// Optional minimum task-reduced mean, inclusive.
    pub minimum_mean: Option<ScoreMicros>,
    /// Optional maximum permitted paired mean regression, inclusive.
    pub maximum_regression: Option<ScoreMicros>,
}
/// Independent gate reasons. Multiple reasons may be true simultaneously.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent failure dimensions can occur together; no exclusive state describes them"
)]
pub struct GateResult {
    /// True only with complete coverage and no failure reasons.
    pub passed: bool,
    /// A measured quality threshold was missed.
    pub quality_failed: bool,
    /// Current infrastructure failures or unresolved execution exist in the experiment.
    pub infrastructure_failed: bool,
    /// Latest scorer passes failed in the experiment.
    pub scoring_failed: bool,
    /// Missing execution, scores, requested metric version or paired coverage.
    pub incomplete: bool,
}
impl ThresholdGate {
    /// Evaluate a conservative experiment-wide health gate and the selected quality metric.
    /// # Errors
    /// Rejects a policy with neither an absolute nor a baseline threshold.
    pub fn evaluate(&self, report: &EvalReport) -> Result<GateResult, EvalError> {
        if self.minimum_mean.is_none() && self.maximum_regression.is_none() {
            return Err(crate::error::invalid());
        }
        let counts = &report.counts;
        let infrastructure_failed = counts.infrastructure_failed
            + counts.indeterminate
            + counts.pending
            + counts.graders_pending
            != 0;
        let scoring_failed = counts.scoring_failed != 0;
        let mut incomplete = counts.unattempted
            + counts.pending
            + counts.indeterminate
            + counts.scoring_pending
            + counts.graders_pending
            != 0;
        let aggregate = report
            .aggregates
            .iter()
            .find(|a| a.subject == self.subject && a.metric == self.metric);
        incomplete |= aggregate.is_none_or(|a| {
            a.complete_tasks != a.expected_tasks || a.scored_cells != a.expected_cells
        });
        let mut quality_failed = false;
        if let Some(minimum) = self.minimum_mean {
            if let Some(mean) = aggregate.and_then(|a| a.statistics.mean) {
                quality_failed |= mean < f64::from(minimum.get()) / 1_000_000.0;
            } else {
                incomplete = true;
            }
        }
        if let Some(regression) = self.maximum_regression {
            let paired = report
                .comparisons
                .iter()
                .find(|p| p.candidate == self.subject)
                .and_then(|p| p.metrics.iter().find(|m| m.metric == self.metric));
            incomplete |= paired.is_none_or(|p| p.missing_pairs != 0);
            if let Some(mean) = paired.and_then(|p| p.delta.mean) {
                quality_failed |= mean < -f64::from(regression.get()) / 1_000_000.0;
            } else {
                incomplete = true;
            }
        }
        Ok(GateResult {
            passed: !quality_failed && !infrastructure_failed && !scoring_failed && !incomplete,
            quality_failed,
            infrastructure_failed,
            scoring_failed,
            incomplete,
        })
    }
}
