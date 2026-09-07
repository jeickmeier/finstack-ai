//! Exact persisted scores; statistical floating point is confined to reports.
use crate::{EVAL_SCORE_OUT_OF_RANGE, EvalError};
use finstack_ai_kernel::RawJson;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A validated score in `0..=1_000_000`, equivalent to the unit interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct ScoreMicros(u32);
impl ScoreMicros {
    /// Zero credit.
    pub const ZERO: Self = Self(0);
    /// Full credit.
    pub const ONE: Self = Self(1_000_000);
    /// Construct a bounded score.
    /// # Errors
    /// Returns `eval_score_out_of_range` above one million.
    pub fn try_new(value: u32) -> Result<Self, EvalError> {
        Self::try_from(value)
    }
    /// Integer millionths of full credit.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}
impl TryFrom<u32> for ScoreMicros {
    type Error = EvalError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value > 1_000_000 {
            return Err(EvalError::new(
                EVAL_SCORE_OUT_OF_RANGE,
                "score exceeds one million micros",
            ));
        }
        Ok(Self(value))
    }
}
impl From<ScoreMicros> for u32 {
    fn from(value: ScoreMicros) -> Self {
        value.0
    }
}

/// One named grade produced by a versioned scorer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Score {
    /// Stable scorer identity.
    pub scorer: Arc<str>,
    /// Explicit scoring behavior version.
    pub scorer_version: u32,
    /// Result name; structured scorers emit one name per field.
    pub name: Arc<str>,
    /// Exact normalized score.
    pub value: ScoreMicros,
    /// Threshold outcome when the scorer supplies one.
    pub passed: Option<bool>,
    /// Safe bounded explanation, never a transcript.
    pub explanation: Option<Arc<str>>,
    /// Optional bounded data-only scorer metadata.
    pub metadata: Option<RawJson>,
}

/// An immutable scoring pass; errors do not change the subject outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreSet {
    /// Scorer identity, including on failure.
    pub scorer: Arc<str>,
    /// Behavior version, including on failure.
    pub scorer_version: u32,
    /// Named grades; empty when scoring failed.
    pub scores: Vec<Score>,
    /// Stable scoring failure code, separate from subject failure.
    pub failure_code: Option<Arc<str>>,
}

impl ScoreSet {
    /// Validate bounded, internally consistent scorer output before persistence.
    /// # Errors
    /// Returns invalid score-set identity, duplicate names, explanations or failure shape.
    pub fn validate(&self) -> Result<(), EvalError> {
        if !crate::config::valid_id(&self.scorer)
            || self.scorer_version == 0
            || self.scores.len() > 256
            || self
                .failure_code
                .as_ref()
                .is_some_and(|code| !crate::config::valid_id(code))
            || (self.failure_code.is_some() && !self.scores.is_empty())
            || (self.failure_code.is_none() && self.scores.is_empty())
        {
            return Err(crate::error::invalid());
        }
        let mut names = std::collections::BTreeSet::new();
        for score in &self.scores {
            if score.scorer != self.scorer
                || score.scorer_version != self.scorer_version
                || score.name.is_empty()
                || score.name.len() > 256
                || !names.insert(&score.name)
                || score
                    .explanation
                    .as_ref()
                    .is_some_and(|text| text.len() > 4096)
            {
                return Err(crate::error::invalid());
            }
        }
        Ok(())
    }
}
