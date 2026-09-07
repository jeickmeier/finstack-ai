//! Frozen bounded experiment inputs and deterministic expansion.
use crate::error::invalid;
use crate::{Cell, EvalError, ScoreMicros};
use finstack_ai_kernel::{ArtifactRef, Digest, RawJson};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Current experiment schema; incompatible formats fail closed.
pub const EVAL_SCHEMA_VERSION: &str = "finstack.eval.v1";
/// Maximum cells expanded by one experiment.
pub const MAX_CELLS: usize = 100_000;
/// Maximum serialized frozen spec bytes.
pub const MAX_SPEC_BYTES: usize = 16 * 1024 * 1024;

/// One prompt/target with scoped artifact references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSample {
    /// Unique identifier containing lowercase ASCII letters, numbers, `_` or `-`.
    pub task_id: Arc<str>,
    /// Bounded subject input; never included in result exports.
    pub input: Arc<str>,
    /// Scorer target; interpreted according to the scorer.
    pub target: Arc<str>,
    /// Optional bounded dataset metadata.
    pub metadata: Option<RawJson>,
    /// At most eight durably staged artifacts.
    #[serde(default)]
    pub attachments: Vec<ArtifactRef>,
}
/// Ordered task dataset.
pub type TaskSet = Vec<TaskSample>;

/// A subject declaration contains identity and an optional expected resolved lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectDecl {
    /// Unique binding identity.
    pub subject_id: Arc<str>,
    /// Expected credential-free lock fingerprint, if pinned in advance.
    pub lock_digest: Option<Digest>,
}

/// Repetition-level reduction before aggregating tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RepetitionReducer {
    /// Arithmetic mean of grades.
    Mean,
    /// Unbiased probability of at least one pass among `k` draws without replacement.
    PassAtK {
        /// Number of draws, at most the configured repetitions.
        k: u32,
        /// Inclusive passing score.
        pass_threshold_micros: ScoreMicros,
    },
    /// Whether at least `k` of the observed repetitions passed.
    AtLeastK {
        /// Required passes.
        k: u32,
        /// Inclusive passing score.
        pass_threshold_micros: ScoreMicros,
    },
}

/// Admission and execution bounds. Money is an admission threshold, not an in-flight cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalLimits {
    /// Maximum time per subject attempt, including settling cancellation.
    pub attempt_timeout_ms: u64,
    /// Maximum replacements after evidence proves a prior attempt replaceable.
    pub max_replacement_attempts: u32,
    /// Maximum concurrent subject attempts.
    pub max_concurrency: u32,
    /// Optional finite spending threshold in integer millionths.
    pub budget_micros: Option<u64>,
    /// Required compatible unit for a finite budget.
    pub budget_unit: Option<Arc<str>>,
}
impl Default for EvalLimits {
    fn default() -> Self {
        Self {
            attempt_timeout_ms: 120_000,
            max_replacement_attempts: 2,
            max_concurrency: 4,
            budget_micros: None,
            budget_unit: None,
        }
    }
}

/// Immutable data-only experiment specification. Validate again at every store boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalSpec {
    /// Must equal `finstack.eval.v1`.
    pub schema_version: Arc<str>,
    /// Bounded non-secret display name.
    pub name: Arc<str>,
    /// Unique tasks in stable dataset order.
    pub tasks: TaskSet,
    /// Unique subject arms in stable pairing order.
    pub subjects: Vec<SubjectDecl>,
    /// Independent executions per task/subject, from one to ten thousand.
    pub repetitions: u32,
    /// Unique scorer bindings, resolved before admission.
    pub scorers: Vec<Arc<str>>,
    /// Repetition reduction applied by reports.
    pub reducer: RepetitionReducer,
    /// Bounded scheduling and admission policy.
    pub limits: EvalLimits,
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

impl EvalSpec {
    /// Validate identities, cardinality, text, expansion and admission bounds.
    /// # Errors
    /// Returns `eval_spec_invalid` for invalid or oversized data.
    pub fn validate(&self) -> Result<(), EvalError> {
        if self.schema_version.as_ref() != EVAL_SCHEMA_VERSION
            || self.name.is_empty()
            || self.name.len() > 256
            || self.name.contains('\0')
            || self.tasks.is_empty()
            || self.subjects.is_empty()
            || self.scorers.is_empty()
            || self.scorers.len() > 64
            || self.subjects.len() > 128
            || self.repetitions == 0
            || self.repetitions > 10_000
        {
            return Err(invalid());
        }
        let cells = self
            .tasks
            .len()
            .checked_mul(self.subjects.len())
            .and_then(|value| value.checked_mul(self.repetitions as usize))
            .ok_or_else(invalid)?;
        if cells > MAX_CELLS {
            return Err(invalid());
        }
        let mut task_ids = BTreeSet::new();
        for task in &self.tasks {
            if !valid_id(&task.task_id)
                || !task_ids.insert(&task.task_id)
                || task.input.is_empty()
                || task.input.len() > 65_536
                || task.target.len() > 65_536
                || task.input.contains('\0')
                || task.attachments.len() > 8
            {
                return Err(invalid());
            }
        }
        for ids in [
            self.subjects
                .iter()
                .map(|subject| subject.subject_id.as_ref())
                .collect::<Vec<_>>(),
            self.scorers.iter().map(AsRef::as_ref).collect(),
        ] {
            let mut seen = BTreeSet::new();
            if ids.into_iter().any(|id| !valid_id(id) || !seen.insert(id)) {
                return Err(invalid());
            }
        }
        if let RepetitionReducer::PassAtK { k, .. } | RepetitionReducer::AtLeastK { k, .. } =
            &self.reducer
            && (*k == 0 || *k > self.repetitions)
        {
            return Err(invalid());
        }
        let limits = &self.limits;
        if limits.attempt_timeout_ms == 0
            || limits.attempt_timeout_ms > 86_400_000
            || limits.max_concurrency == 0
            || limits.max_concurrency > 256
            || limits.max_replacement_attempts > 16
            || limits.budget_micros.is_some_and(|_| {
                limits
                    .budget_unit
                    .as_ref()
                    .is_none_or(|unit| unit.is_empty() || unit.len() > 64 || unit.contains('\0'))
            })
        {
            return Err(invalid());
        }
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        if bytes.len() > MAX_SPEC_BYTES {
            return Err(invalid());
        }
        Ok(())
    }

    /// Digest the validated JCS representation with the `eval-spec` domain.
    /// # Errors
    /// Returns invalid configuration or canonicalization failure.
    pub fn digest(&self) -> Result<Digest, EvalError> {
        self.validate()?;
        let bytes = serde_json_canonicalizer::to_vec(self).map_err(|_| invalid())?;
        Digest::domain_separated("eval-spec", 1, &bytes).map_err(|_| invalid())
    }

    /// Expand task, repetition, then subject order without retries.
    /// # Errors
    /// Returns invalid configuration before allocating expanded cells.
    pub fn cells(&self) -> Result<Vec<Cell>, EvalError> {
        self.validate()?;
        Ok(self
            .tasks
            .iter()
            .flat_map(|task| {
                (0..self.repetitions).flat_map(move |repetition| {
                    self.subjects.iter().map(move |subject| Cell {
                        id: Arc::from(format!(
                            "{}::{repetition}::{}",
                            task.task_id, subject.subject_id
                        )),
                        task_id: Arc::clone(&task.task_id),
                        repetition,
                        subject_id: Arc::clone(&subject.subject_id),
                    })
                })
            })
            .collect())
    }
}
