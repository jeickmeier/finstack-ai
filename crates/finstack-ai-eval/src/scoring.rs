//! Versioned scorers over optional subject output and authoritative journal references.
use crate::{AttemptRecord, EvalError, Score, ScorerId, TaskSample};
use finstack_ai::{AgentRunOutput, runtime::ports::journal::JournalStore};
use std::{future::Future, pin::Pin, sync::Arc};

/// Read-only input for a scoring pass. A failed subject can have no final output.
pub struct ScoreContext<'a> {
    /// Frozen task input, target and bounded dataset metadata.
    pub sample: &'a TaskSample,
    /// Successful committed output, absent for subject failures.
    pub output: Option<&'a AgentRunOutput>,
    /// Authoritative subject classification, usage and record-kind evidence.
    pub record: &'a AttemptRecord,
    /// Host-authorized journal for optional full transcript inspection.
    pub journal: &'a Arc<dyn JournalStore>,
    /// Runner-owned, persisted grader execution; absent for pure scoring contexts.
    pub grader: Option<&'a crate::GraderExecution>,
}

/// Coarse scoring callback. Errors belong to scoring passes, never subject status.
/// Custom implementations must honor future cancellation and must not dispatch
/// untracked external work. Agent graders use the evaluator's journaled helper.
pub trait Scorer: Send + Sync {
    /// Stable binding identity declared by the frozen experiment.
    fn id(&self) -> ScorerId;
    /// Explicit behavior/configuration version stored beside every score.
    fn version(&self) -> u32;
    /// Validate configuration before subject admission.
    /// # Errors
    /// Returns invalid identity/version/configuration errors.
    fn validate(&self) -> Result<(), EvalError> {
        if !crate::config::valid_id(&self.id()) || self.version() == 0 {
            return Err(crate::error::invalid());
        }
        Ok(())
    }
    /// Optional fixed grader agent, used for lock/budget preflight and recovery.
    /// The evaluator supplies its persisted execution helper in scoring context.
    fn grader_agent(&self) -> Option<finstack_ai::Agent> {
        None
    }
    /// Grade one terminal subject result. The runner imposes a bounded timeout.
    fn score<'a>(
        &'a self,
        context: &'a ScoreContext<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Score>, EvalError>> + Send + 'a>>;
}

pub(crate) fn value(scorer: &dyn Scorer, name: &str, grade: crate::ScoreMicros) -> Score {
    Score {
        scorer: scorer.id(),
        scorer_version: scorer.version(),
        name: Arc::from(name),
        value: grade,
        passed: Some(grade == crate::ScoreMicros::ONE),
        explanation: None,
        metadata: None,
    }
}
pub(crate) fn boolean(scorer: &dyn Scorer, name: &str, passed: bool) -> Score {
    value(
        scorer,
        name,
        if passed {
            crate::ScoreMicros::ONE
        } else {
            crate::ScoreMicros::ZERO
        },
    )
}
pub(crate) fn target_invalid() -> EvalError {
    EvalError::new(
        crate::EVAL_TARGET_INVALID,
        "target does not satisfy the scorer contract",
    )
}

// Pure scorers share only the mechanical async boundary. Their configuration,
// validation and scoring rules remain owned by their concrete types.
macro_rules! implement_scorer {
    ($type:ty) => {
        impl crate::Scorer for $type {
            fn id(&self) -> crate::ScorerId {
                std::sync::Arc::clone(&self.id)
            }
            fn version(&self) -> u32 {
                self.version
            }
            fn score<'a>(
                &'a self,
                context: &'a crate::ScoreContext<'a>,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<Vec<crate::Score>, crate::EvalError>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async move { self.evaluate(context) })
            }
        }
    };
}
pub(crate) use implement_scorer;
