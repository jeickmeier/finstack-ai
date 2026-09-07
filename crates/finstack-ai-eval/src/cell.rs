//! Deterministic task × repetition × subject coordinates.
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Stable cell key: `{task_id}::{repetition}::{subject_id}`.
pub type CellId = Arc<str>;
/// Stable declared subject identity.
pub type SubjectId = Arc<str>;
/// Stable declared scorer identity.
pub type ScorerId = Arc<str>;

/// One independent sample coordinate; repetition is zero-based and is not retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    /// Unambiguous stable key.
    pub id: CellId,
    /// Task identity from the frozen specification.
    pub task_id: Arc<str>,
    /// Zero-based repetition number.
    pub repetition: u32,
    /// Subject arm from the frozen specification.
    pub subject_id: SubjectId,
}
