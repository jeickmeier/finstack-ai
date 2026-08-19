//! Redacted diagnostic JSONL projections. Canonical journal CBOR is unchanged.

use serde::Serialize;
use serde_json::Value;

use finstack_ai_kernel::RunEvent;

use super::{ObserverError, ObserverEventView, ObserverPayloadMode};

/// Serialize observer views as one JSON object per line.
///
/// # Errors
///
/// Returns `observer_unavailable` when a view cannot be encoded as a single line.
pub fn observer_events_jsonl(
    events: &[RunEvent],
    mode: ObserverPayloadMode,
) -> Result<String, ObserverError> {
    let mut out = String::new();
    for event in events {
        let view = ObserverEventView::from_event(event, mode);
        let line = serde_json::to_string(&view).map_err(|_| ObserverError::Unavailable)?;
        if line.contains('\n') {
            return Err(ObserverError::Unavailable);
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// Project journal records to diagnostic JSONL.
///
/// `MetadataOnly` and `Redacted` omit `body`. `Full` keeps the serialized
/// record, including bodies. This never rewrites store CBOR.
///
/// # Errors
///
/// Returns `observer_unavailable` when a record cannot be encoded as a single line.
pub fn journal_export_jsonl<T: Serialize>(
    records: &[T],
    mode: ObserverPayloadMode,
) -> Result<String, ObserverError> {
    let mut out = String::new();
    for record in records {
        let mut value = serde_json::to_value(record).map_err(|_| ObserverError::Unavailable)?;
        if mode != ObserverPayloadMode::Full
            && let Some(object) = value.as_object_mut()
        {
            object.remove("body");
        }
        let line = serde_json::to_string(&value).map_err(|_| ObserverError::Unavailable)?;
        if line.contains('\n') {
            return Err(ObserverError::Unavailable);
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// JSON object used by support-bundle version files.
#[must_use]
pub fn support_bundle_versions(engine: &str, observers: &[&str]) -> Value {
    serde_json::json!({
        "engine": engine,
        "observers": observers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn journal_export_strips_body_unless_full() {
        let records = [json!({
            "record_id": "rec",
            "body": { "secret": "CANARY_SECRET_VALUE" }
        })];
        let redacted =
            journal_export_jsonl(&records, ObserverPayloadMode::Redacted).expect("redacted");
        assert!(!redacted.contains("CANARY_SECRET_VALUE"));
        assert!(!redacted.contains("\"body\""));
        let full = journal_export_jsonl(&records, ObserverPayloadMode::Full).expect("full");
        assert!(full.contains("CANARY_SECRET_VALUE"));
    }
}
