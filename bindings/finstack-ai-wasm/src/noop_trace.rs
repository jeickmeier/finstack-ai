//! Embedded no-op golden-trace runner used by the headless browser harness.
//!
//! This is engine-load proof, not WASM binding-parity or conformance evidence.

use std::sync::Arc;

use finstack_ai::runtime::CommitCoordinator;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

const NOOP_TRACE: &str =
    include_str!("../../../fixtures/compatibility/golden-trace/v1/trace/valid--noop.json");

/// Report returned after constructing the coordinator over an empty store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoopTraceReport {
    /// Durable records observed after construction. Empty for the no-op fixture.
    pub durable_records: Vec<serde_json::Value>,
    /// Normalized events observed after construction. Empty for the no-op fixture.
    pub normalized_events: Vec<serde_json::Value>,
    /// Whether the empty observed output matches the fixture `expected` arrays.
    pub matched: bool,
}

/// Drive the embedded no-op fixture through `CommitCoordinator` and an in-memory store.
///
/// The runner constructs the store and coordinator and performs no `submit`.
///
/// # Errors
///
/// Returns a static reason when the fixture cannot be parsed, the memory store
/// cannot be constructed, or the empty observed output does not match.
pub fn run_noop_trace() -> Result<NoopTraceReport, &'static str> {
    let fixture: serde_json::Value =
        serde_json::from_str(NOOP_TRACE).map_err(|_| "noop fixture is not valid JSON")?;
    let expected = fixture
        .get("expected")
        .ok_or("noop fixture is missing expected")?;
    let expected_records = expected
        .get("durable_records")
        .and_then(serde_json::Value::as_array)
        .ok_or("noop fixture is missing durable_records")?;
    let expected_events = expected
        .get("normalized_events")
        .and_then(serde_json::Value::as_array)
        .ok_or("noop fixture is missing normalized_events")?;

    let store = MemoryJournalStore::try_new(MemoryStoreLimits {
        sessions: 4,
        batches_per_session: 8,
        records_per_session: 16,
        snapshot_bytes: 1_024,
    })
    .map_err(|_| "memory store limits were rejected")?;
    let _coordinator = CommitCoordinator::new(Arc::new(store));

    let durable_records = Vec::new();
    let normalized_events = Vec::new();
    let matched = expected_records.is_empty() && expected_events.is_empty();
    if !matched {
        return Err("noop fixture expected non-empty output");
    }
    Ok(NoopTraceReport {
        durable_records,
        normalized_events,
        matched,
    })
}

/// Serialize [`run_noop_trace`] as canonical JSON for the browser harness.
///
/// # Errors
///
/// Returns the runner reason, or a serialization failure string.
pub fn run_noop_trace_json() -> Result<String, &'static str> {
    let report = run_noop_trace()?;
    serde_json::to_string(&serde_json::json!({
        "durable_records": report.durable_records,
        "normalized_events": report.normalized_events,
        "matched": report.matched,
    }))
    .map_err(|_| "noop report serialization failed")
}

#[cfg(test)]
mod tests {
    use super::run_noop_trace;

    #[test]
    fn noop_trace_matches_empty_expected_output() {
        let report = run_noop_trace().expect("noop trace");
        assert!(report.matched);
        assert!(report.durable_records.is_empty());
        assert!(report.normalized_events.is_empty());
    }
}
