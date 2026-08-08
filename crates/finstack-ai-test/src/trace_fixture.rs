//! Golden-trace load, validate, normalize, and compare helpers.
//!
//! Normalization is fixture-only deterministic JSON key ordering. It does not
//! claim ADR-016 JCS semantics and does not compute Phase 1 state hashes.

use std::fs;
use std::path::Path;

use jsonschema::Draft;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::{compatibility_fixture, schema_path};
use crate::scripted_model::ScriptedInput;

/// Errors produced while loading or comparing golden traces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    /// Filesystem or UTF-8 read failure.
    Io(String),
    /// JSON parse failure.
    Parse(String),
    /// JSON Schema validation failure.
    Schema(String),
    /// Byte-for-byte comparison mismatch after normalization.
    Mismatch {
        /// Expected normalized JSON bytes.
        expected: Vec<u8>,
        /// Observed normalized JSON bytes.
        actual: Vec<u8>,
    },
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) | Self::Parse(message) | Self::Schema(message) => {
                write!(f, "{message}")
            }
            Self::Mismatch { expected, actual } => write!(
                f,
                "normalized trace bytes differ (expected {} bytes, actual {} bytes)",
                expected.len(),
                actual.len()
            ),
        }
    }
}

impl std::error::Error for TraceError {}

/// Declared payload ceilings checked by fixture schemas (TDD §6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PayloadDeclaration {
    /// Canonical record envelope bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_envelope_bytes: Option<u64>,
    /// Atomic append batch bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_batch_bytes: Option<u64>,
    /// Atomic append batch record count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_batch_records: Option<u64>,
    /// Individual text or byte string size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_string_bytes: Option<u64>,
    /// Array item count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_array_items: Option<u64>,
    /// Map entry count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_map_entries: Option<u64>,
    /// Nesting depth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_nesting_depth: Option<u64>,
    /// Raw JSON bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_raw_json_bytes: Option<u64>,
    /// Metadata bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_metadata_bytes: Option<u64>,
}

/// Durability class for normalized public events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityClass {
    /// Replay-stable durable-derived event.
    Durable,
    /// Transient progress or diagnostic event.
    Transient,
}

/// One expected or observed durable record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceRecord {
    /// Record kind name.
    pub kind: String,
    /// Optional identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Opaque payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// Optional payload declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_declaration: Option<PayloadDeclaration>,
}

/// One expected or observed normalized event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedEvent {
    /// Event kind name.
    pub kind: String,
    /// Durable vs transient classification.
    pub durability: DurabilityClass,
    /// Optional identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Opaque payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// Optional payload declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_declaration: Option<PayloadDeclaration>,
}

/// One expected or observed effect request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectExpectation {
    /// Effect kind name.
    pub kind: String,
    /// Optional identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Opaque payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// Optional payload declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_declaration: Option<PayloadDeclaration>,
}

/// Expected durable records, events, effects, and final opaque outcomes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedTrace {
    /// Expected durable records.
    pub durable_records: Vec<TraceRecord>,
    /// Expected normalized public events with durability classification.
    pub normalized_events: Vec<NormalizedEvent>,
    /// Expected effects.
    pub effects: Vec<EffectExpectation>,
    /// Opaque expected final state.
    pub final_state: Value,
    /// Opaque expected final result.
    pub final_result: Value,
    /// Opaque expected state hash string (not computed by this harness).
    pub state_hash: String,
}

/// Injected transition timestamps and IDs for deterministic fixtures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionEnv {
    /// Semantic timestamps in milliseconds.
    pub timestamps_ms: Vec<u64>,
    /// Injected identifiers.
    pub ids: Vec<String>,
}

/// Complete golden-trace fixture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoldenTrace {
    /// Format major version.
    pub format_version: u32,
    /// Stable fixture identifier.
    pub trace_id: String,
    /// Initial agent specification fragment.
    pub initial_agent_spec: Value,
    /// Initial session records.
    pub initial_session_records: Vec<TraceRecord>,
    /// Scripted model/tool/timer outcomes.
    pub scripted_outcomes: ScriptedInput,
    /// Injected transition environment.
    pub transition_env: TransitionEnv,
    /// Expected outputs and opaque hashes.
    pub expected: ExpectedTrace,
}

impl GoldenTrace {
    /// Return durable events only.
    #[must_use]
    pub fn durable_events(&self) -> Vec<&NormalizedEvent> {
        self.expected
            .normalized_events
            .iter()
            .filter(|event| event.durability == DurabilityClass::Durable)
            .collect()
    }

    /// Return transient events only.
    #[must_use]
    pub fn transient_events(&self) -> Vec<&NormalizedEvent> {
        self.expected
            .normalized_events
            .iter()
            .filter(|event| event.durability == DurabilityClass::Transient)
            .collect()
    }

    /// Serialize this fixture to normalized JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns [`TraceError::Parse`] when serialization fails.
    pub fn to_normalized_bytes(&self) -> Result<Vec<u8>, TraceError> {
        let value =
            serde_json::to_value(self).map_err(|error| TraceError::Parse(error.to_string()))?;
        Ok(normalize_json_value(&value))
    }
}

/// Load, parse, and schema-validate a golden trace fixture.
///
/// # Errors
///
/// Returns I/O, parse, or schema validation errors.
pub fn load_golden_trace(path: impl AsRef<Path>) -> Result<GoldenTrace, TraceError> {
    let path = path.as_ref();
    let text = fs::read_to_string(path)
        .map_err(|error| TraceError::Io(format!("{}: {error}", path.display())))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| TraceError::Parse(format!("{}: {error}", path.display())))?;
    validate_against_schema("golden-trace", 1, "trace", &value)?;
    serde_json::from_value(value)
        .map_err(|error| TraceError::Parse(format!("{}: {error}", path.display())))
}

/// Load the repository no-op golden trace fixture.
///
/// # Errors
///
/// Propagates [`load_golden_trace`] failures.
pub fn load_noop_trace() -> Result<GoldenTrace, TraceError> {
    load_golden_trace(compatibility_fixture(
        "golden-trace/v1/trace/valid--noop.json",
    ))
}

/// Validate a JSON value against a repository schema family/kind.
///
/// # Errors
///
/// Returns schema load or validation failures.
pub fn validate_against_schema(
    family: &str,
    major: u32,
    kind: &str,
    instance: &Value,
) -> Result<(), TraceError> {
    let path = schema_path(family, major, kind);
    let text = fs::read_to_string(&path)
        .map_err(|error| TraceError::Io(format!("{}: {error}", path.display())))?;
    let schema: Value = serde_json::from_str(&text)
        .map_err(|error| TraceError::Parse(format!("{}: {error}", path.display())))?;
    let validator = jsonschema::options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .map_err(|error| TraceError::Schema(format!("{}: {error}", path.display())))?;
    if let Err(error) = validator.validate(instance) {
        return Err(TraceError::Schema(format!(
            "{} failed validation: {error}",
            path.display()
        )));
    }
    Ok(())
}

/// Deterministically normalize a JSON value to sorted-key UTF-8 bytes.
///
/// This is a fixture harness helper only. It is not ADR-016 JCS.
#[must_use]
pub fn normalize_json_value(value: &Value) -> Vec<u8> {
    let normalized = sort_value(value);
    // Compact separators keep comparisons stable across platforms.
    serde_json::to_vec(&normalized).unwrap_or_else(|_| b"null".to_vec())
}

/// Compare two JSON values after fixture-only normalization.
///
/// # Errors
///
/// Returns [`TraceError::Mismatch`] when normalized bytes differ.
pub fn compare_normalized_bytes(expected: &Value, actual: &Value) -> Result<(), TraceError> {
    let expected_bytes = normalize_json_value(expected);
    let actual_bytes = normalize_json_value(actual);
    if expected_bytes == actual_bytes {
        Ok(())
    } else {
        Err(TraceError::Mismatch {
            expected: expected_bytes,
            actual: actual_bytes,
        })
    }
}

fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(left, _)| *left);
            let mut object = serde_json::Map::with_capacity(entries.len());
            for (key, child) in entries {
                object.insert(key.clone(), sort_value(child));
            }
            Value::Object(object)
        }
        Value::Array(items) => Value::Array(items.iter().map(sort_value).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_sorts_object_keys() {
        let left = serde_json::json!({"b": 1, "a": {"d": 2, "c": 3}});
        let right = serde_json::json!({"a": {"c": 3, "d": 2}, "b": 1});
        assert_eq!(normalize_json_value(&left), normalize_json_value(&right));
    }
}
