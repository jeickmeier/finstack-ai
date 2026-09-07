//! Explicit export allowlist: no inputs, targets, transcripts, explanations or metadata.
use super::{EvalReport, authoritative};
use crate::{EvalError, MeasuredUsage, StoreSnapshot};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Write};

fn usage(value: &MeasuredUsage) -> Value {
    json!({
        "input_tokens": value.input_tokens.map(|v| v.to_string()),
        "output_tokens": value.output_tokens.map(|v| v.to_string()),
        "total_tokens": value.total_tokens.map(|v| v.to_string()),
        "cost_by_unit": value.cost_by_unit.iter().map(|(k,v)| (k, v.to_string())).collect::<BTreeMap<_,_>>(),
        "cost": value.cost.as_ref().map(|c| json!({"unit": c.unit, "micros": c.micros.to_string()})),
        "effects": value.effects.to_string(), "model_effects": value.model_effects.to_string(),
        "uncosted_effects": value.uncosted_effects.to_string(), "complete": value.complete,
    })
}
/// Write one JSONL row per authoritative subject attempt, in frozen cell order.
/// Values with arbitrary callback/task content are omitted; integer measurements are decimal strings.
/// Historical non-authoritative failures remain in the store and summary counters/spending.
/// # Errors
/// Returns invalid snapshots, checked accounting overflow or writer failure.
pub fn export_jsonl(snapshot: &StoreSnapshot, mut writer: impl Write) -> Result<(), EvalError> {
    // Validate the same projection used by summary/gates before publishing any rows.
    EvalReport::from_snapshot(snapshot)?;
    let frozen = snapshot.frozen.as_ref().ok_or_else(crate::error::invalid)?;
    let mut graders = BTreeMap::<&str, Vec<&crate::GraderRecord>>::new();
    for record in snapshot.graders.values() {
        graders
            .entry(&record.reservation.cell)
            .or_default()
            .push(record);
    }
    for cell in frozen.spec.cells()? {
        let Some(record) = snapshot
            .attempts
            .get(&cell.id)
            .and_then(|records| authoritative(records))
        else {
            continue;
        };
        let scores: Vec<_> = record.scores.iter().map(|pass| json!({
            "scorer": pass.scorer, "scorer_version": pass.scorer_version.to_string(), "failure_code": pass.failure_code,
            "scores": pass.scores.iter().map(|score| json!({"name": score.name, "value_micros": score.value.get().to_string(), "passed": score.passed})).collect::<Vec<_>>()
        })).collect();
        let graders: Vec<_> = graders.get(cell.id.as_ref()).into_iter().flatten().filter(|g| g.reservation.attempt_sequence == record.sequence).map(|g| json!({
            "scorer": g.reservation.scorer, "scorer_version": g.reservation.scorer_version.to_string(), "pass": g.reservation.pass.to_string(), "lock_digest": g.reservation.lock_digest,
            "outcome": g.outcome.as_ref().map(|o| json!({"status": o.status, "reconciliation": o.reconciliation, "failure_code": o.failure_code, "locator": o.locator, "usage": usage(&o.usage)}))
        })).collect();
        let row = json!({
            "schema_version": "finstack.eval.export.v1", "spec_digest": frozen.digest,
            "cell": cell.id, "task_id": cell.task_id, "subject_id": cell.subject_id, "repetition": cell.repetition.to_string(),
            "sequence": record.sequence.to_string(), "status": record.status, "reconciliation": record.reconciliation,
            "failure_code": record.failure_code, "locator": record.locator, "usage": usage(&record.usage),
            "duration_ms": record.duration_ms.map(|v| v.to_string()), "started_at_ms": record.started_at_ms.to_string(), "completed_at_ms": record.completed_at_ms.to_string(),
            "score_passes": scores, "graders": graders,
        });
        serde_json::to_writer(&mut writer, &row).map_err(|_| crate::error::unavailable())?;
        writer
            .write_all(b"\n")
            .map_err(|_| crate::error::unavailable())?;
    }
    Ok(())
}
impl EvalReport {
    /// Write the body-free summary JSON; statistical values and exact costs use decimal strings.
    /// # Errors
    /// Returns serialization or writer failure.
    pub fn write_json(&self, writer: impl Write) -> Result<(), EvalError> {
        serde_json::to_writer_pretty(writer, self).map_err(|_| crate::error::unavailable())
    }
}
