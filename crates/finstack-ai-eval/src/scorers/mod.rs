//! Bounded deterministic built-in text, numeric, structured and journal scorers.
mod structured;
pub use crate::numeric::{NumberUnit, ParsedNumber, ToleranceBands};
use crate::scoring::{boolean, implement_scorer, target_invalid, value};
use crate::{EvalError, Score, ScoreContext, ScoreMicros, ScorerId};
use regex::{Regex, RegexBuilder};
use std::{collections::BTreeSet, sync::Arc};
pub use structured::{FieldSpec, FieldTolerance, StructuredFieldScorer};

fn identity(id: impl Into<ScorerId>, version: u32) -> Result<ScorerId, EvalError> {
    let id = id.into();
    if !crate::config::valid_id(&id) || version == 0 {
        return Err(crate::error::invalid());
    }
    Ok(id)
}
fn normalize(text: &str, trim: bool, case_sensitive: bool) -> String {
    let text = if trim { text.trim() } else { text };
    if case_sensitive {
        text.to_owned()
    } else {
        text.to_lowercase()
    }
}

pub(crate) fn answer_text(ctx: &ScoreContext<'_>) -> Result<Option<String>, EvalError> {
    let Some(output) = ctx.output else {
        return Ok(None);
    };
    let mut bytes = 0_usize;
    for block in output.message.content() {
        if let finstack_ai_kernel::ContentBlock::Text(text) = block {
            bytes = bytes
                .checked_add(text.text().len())
                .ok_or_else(crate::error::invalid)?;
            if bytes > 1_048_576 {
                return Err(EvalError::new(
                    crate::EVAL_SCORER_FAILED,
                    "answer exceeds scoring text limit",
                ));
            }
        }
    }
    Ok(Some(output.text()))
}

/// Compare the whole final answer with the task target, with explicit normalization.
pub struct ExactMatchScorer {
    id: ScorerId,
    version: u32,
    trim: bool,
    case_sensitive: bool,
}
impl ExactMatchScorer {
    /// Configure whole-answer equality. Missing output receives zero credit.
    /// # Errors
    /// Returns invalid scorer identity/version.
    pub fn new(
        id: impl Into<ScorerId>,
        version: u32,
        trim: bool,
        case_sensitive: bool,
    ) -> Result<Self, EvalError> {
        Ok(Self {
            id: identity(id, version)?,
            version,
            trim,
            case_sensitive,
        })
    }
    fn evaluate(&self, ctx: &ScoreContext<'_>) -> Result<Vec<Score>, EvalError> {
        let passed = answer_text(ctx)?.is_some_and(|output| {
            normalize(&output, self.trim, self.case_sensitive)
                == normalize(&ctx.sample.target, self.trim, self.case_sensitive)
        });
        Ok(vec![boolean(self, "exact_match", passed)])
    }
}
implement_scorer!(ExactMatchScorer);

/// Check for a literal nonempty target inside the final answer.
pub struct IncludesScorer {
    id: ScorerId,
    version: u32,
    case_sensitive: bool,
}
impl IncludesScorer {
    /// Configure literal substring matching; target strings are never regexes.
    /// # Errors
    /// Returns invalid scorer identity/version.
    pub fn new(
        id: impl Into<ScorerId>,
        version: u32,
        case_sensitive: bool,
    ) -> Result<Self, EvalError> {
        Ok(Self {
            id: identity(id, version)?,
            version,
            case_sensitive,
        })
    }
    fn evaluate(&self, ctx: &ScoreContext<'_>) -> Result<Vec<Score>, EvalError> {
        if ctx.sample.target.is_empty() {
            return Err(target_invalid());
        }
        let target = normalize(&ctx.sample.target, false, self.case_sensitive);
        let passed = answer_text(ctx)?
            .is_some_and(|output| normalize(&output, false, self.case_sensitive).contains(&target));
        Ok(vec![boolean(self, "includes", passed)])
    }
}
implement_scorer!(IncludesScorer);

/// Search final text with a precompiled bounded Rust regex; no backtracking engine.
pub struct RegexScorer {
    id: ScorerId,
    version: u32,
    pattern: Regex,
}
impl RegexScorer {
    /// Compile an explicit pattern, at most 4096 bytes and 2 MiB compiled size.
    /// The pattern is scorer configuration; this scorer does not interpret task targets.
    /// # Errors
    /// Returns invalid identity/version, syntax or compiled-size bounds.
    pub fn new(id: impl Into<ScorerId>, version: u32, pattern: &str) -> Result<Self, EvalError> {
        if pattern.len() > 4096 {
            return Err(crate::error::invalid());
        }
        let pattern = RegexBuilder::new(pattern)
            .size_limit(2 * 1024 * 1024)
            .dfa_size_limit(512 * 1024)
            .build()
            .map_err(|_| crate::error::invalid())?;
        Ok(Self {
            id: identity(id, version)?,
            version,
            pattern,
        })
    }
    fn evaluate(&self, ctx: &ScoreContext<'_>) -> Result<Vec<Score>, EvalError> {
        Ok(vec![boolean(
            self,
            "regex",
            answer_text(ctx)?.is_some_and(|output| self.pattern.is_match(&output)),
        )])
    }
}
implement_scorer!(RegexScorer);

/// Score one exact decimal answer against a unit-aware full/partial tolerance band.
pub struct NumericToleranceScorer {
    id: ScorerId,
    version: u32,
    bands: ToleranceBands,
}
impl NumericToleranceScorer {
    /// Construct validated decimal tolerance bands.
    /// # Errors
    /// Returns invalid identity, version or band configuration.
    pub fn new(
        id: impl Into<ScorerId>,
        version: u32,
        bands: ToleranceBands,
    ) -> Result<Self, EvalError> {
        bands.validate()?;
        Ok(Self {
            id: identity(id, version)?,
            version,
            bands,
        })
    }
    fn evaluate(&self, ctx: &ScoreContext<'_>) -> Result<Vec<Score>, EvalError> {
        let target = ParsedNumber::parse(&ctx.sample.target)?;
        let answer = answer_text(ctx)?.and_then(|output| ParsedNumber::parse(&output).ok());
        let grade = answer.as_ref().map_or(Ok(ScoreMicros::ZERO), |answer| {
            self.bands.grade(answer, &target)
        })?;
        Ok(vec![value(self, "numeric_tolerance", grade)])
    }
}
implement_scorer!(NumericToleranceScorer);

/// Require and forbid committed record kinds across root and measured child history.
pub struct RecordKindsScorer {
    id: ScorerId,
    version: u32,
    required: BTreeSet<Arc<str>>,
    forbidden: BTreeSet<Arc<str>>,
}
impl RecordKindsScorer {
    /// Configure at most 64 required/forbidden kinds each; overlapping sets are invalid.
    /// # Errors
    /// Returns invalid names, duplicate/overlapping kinds, or configured bounds.
    pub fn new(
        id: impl Into<ScorerId>,
        version: u32,
        required: Vec<Arc<str>>,
        forbidden: Vec<Arc<str>>,
    ) -> Result<Self, EvalError> {
        let validate = |items: Vec<Arc<str>>| -> Result<BTreeSet<Arc<str>>, EvalError> {
            if items.len() > 64 || items.iter().any(|item| !crate::config::valid_id(item)) {
                return Err(crate::error::invalid());
            }
            let len = items.len();
            let set: BTreeSet<_> = items.into_iter().collect();
            if set.len() != len {
                return Err(crate::error::invalid());
            }
            Ok(set)
        };
        let required = validate(required)?;
        let forbidden = validate(forbidden)?;
        if !required.is_disjoint(&forbidden) {
            return Err(crate::error::invalid());
        }
        Ok(Self {
            id: identity(id, version)?,
            version,
            required,
            forbidden,
        })
    }
    fn evaluate(&self, ctx: &ScoreContext<'_>) -> Result<Vec<Score>, EvalError> {
        if !ctx.record.usage.complete {
            return Err(EvalError::new(
                crate::EVAL_HISTORY_INCOMPLETE,
                "record-kind scoring requires complete journal history",
            ));
        }
        let observed: BTreeSet<_> = ctx.record.record_kinds.iter().cloned().collect();
        Ok(vec![boolean(
            self,
            "record_kinds",
            self.required.is_subset(&observed) && self.forbidden.is_disjoint(&observed),
        )])
    }
}
implement_scorer!(RecordKindsScorer);
