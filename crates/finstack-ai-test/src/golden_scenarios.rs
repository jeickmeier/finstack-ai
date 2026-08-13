//! Strict language-neutral golden scenarios published by the public test kit.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{TraceError, compatibility_fixture, validate_against_schema};

/// Stable golden-scenario identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoldenScenarioId {
    /// Exact child locator and retry lineage.
    ChildRunLineage,
    /// Scoped deferred external completion and replay.
    DeferredExternalCompletion,
    /// Typed interaction schema validation.
    TypedInteractionSchema,
    /// Equal and conflicting duplicate completion.
    DuplicateCompletion,
    /// Before-finalize continuation before terminal commit.
    BeforeFinalizeContinuation,
    /// Replay-safe protected compaction projection.
    CompactionProjection,
}

/// One language-neutral scenario input and expected contract set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenScenario {
    /// Stable scenario identity.
    pub id: GoldenScenarioId,
    /// Strictly bounded scenario-specific input object.
    pub input: Value,
    /// Strictly bounded scenario-specific expected projection.
    pub expected: Value,
    /// Stable contracts exercised by the scenario.
    pub contracts: Vec<String>,
}

/// Versioned suite of the six required public test-kit scenarios.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenScenarioSuite {
    /// Format major version; exactly one for this schema.
    pub format_version: u32,
    /// Exactly one entry for every required scenario.
    pub scenarios: Vec<GoldenScenario>,
}

impl GoldenScenarioSuite {
    /// Find one scenario by stable identity.
    #[must_use]
    pub fn scenario(&self, id: GoldenScenarioId) -> Option<&GoldenScenario> {
        self.scenarios.iter().find(|scenario| scenario.id == id)
    }
}

/// Load and semantically validate the checked-in public test-kit scenario suite.
///
/// # Errors
///
/// Returns strict schema, parse, version, duplicate, or missing-scenario failures.
pub fn load_golden_scenarios() -> Result<GoldenScenarioSuite, TraceError> {
    let path = compatibility_fixture("golden-trace/v1/test-kit/valid--pr023-scenarios.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| TraceError::Io(format!("{}: {error}", path.display())))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| TraceError::Parse(format!("{}: {error}", path.display())))?;
    validate_against_schema("golden-trace", 1, "test-kit", &value)?;
    let suite: GoldenScenarioSuite = serde_json::from_value(value)
        .map_err(|error| TraceError::Parse(format!("{}: {error}", path.display())))?;
    if suite.format_version != 1 {
        return Err(TraceError::Parse(
            "test-kit scenario format_version must equal 1".into(),
        ));
    }
    let expected = BTreeSet::from([
        GoldenScenarioId::ChildRunLineage,
        GoldenScenarioId::DeferredExternalCompletion,
        GoldenScenarioId::TypedInteractionSchema,
        GoldenScenarioId::DuplicateCompletion,
        GoldenScenarioId::BeforeFinalizeContinuation,
        GoldenScenarioId::CompactionProjection,
    ]);
    let observed = suite
        .scenarios
        .iter()
        .map(|scenario| scenario.id)
        .collect::<BTreeSet<_>>();
    if suite.scenarios.len() != expected.len() || observed != expected {
        return Err(TraceError::Parse(
            "test-kit scenario suite must contain each required identity exactly once".into(),
        ));
    }
    Ok(suite)
}
