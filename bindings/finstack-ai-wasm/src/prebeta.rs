//! Rust-owned pre-beta shape normalization matching the Python binding.

use finstack_ai_kernel::{
    ChildRunPrepared, ExternalEffectCompletionCommand, InteractionResolutionCommand,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Supported pre-beta command kinds.
pub const CHILD_RUN_PREPARED: &str = "child_run_prepared";
/// Interaction-resolution command kind.
pub const INTERACTION_RESOLUTION: &str = "interaction_resolution";
/// Authenticated external-effect completion kind.
pub const EXTERNAL_EFFECT_COMPLETION: &str = "external_effect_completion";

/// Normalize one pre-beta lineage or authenticated external-command shape.
///
/// # Errors
///
/// Returns a stable reason when `kind` is unsupported or `value` fails DTO validation.
pub fn normalize_prebeta_shape(kind: &str, encoded: &str) -> Result<String, PrebetaError> {
    match kind {
        CHILD_RUN_PREPARED => normalize_shape::<ChildRunPrepared>(encoded),
        INTERACTION_RESOLUTION => normalize_shape::<InteractionResolutionCommand>(encoded),
        EXTERNAL_EFFECT_COMPLETION => normalize_shape::<ExternalEffectCompletionCommand>(encoded),
        _ => Err(PrebetaError::UnsupportedKind),
    }
}

/// Apply one already-normalized command and return a semantic identity trace.
///
/// # Errors
///
/// Returns a stable reason when the command is invalid for its kind.
pub fn apply_prebeta_command(kind: &str, encoded: &str) -> Result<serde_json::Value, PrebetaError> {
    let normalized = normalize_prebeta_shape(kind, encoded)?;
    match kind {
        CHILD_RUN_PREPARED => {
            let prepared: ChildRunPrepared =
                serde_json::from_str(&normalized).map_err(|_| PrebetaError::InvalidShape)?;
            let tenant = prepared.child.operation.tenant_scope.clone();
            prepared
                .validate(&tenant)
                .map_err(|_| PrebetaError::InvalidShape)?;
            Ok(serde_json::json!({
                "kind": kind,
                "status": "applied",
                "parent_run_id": prepared.parent_run_id,
                "parent_effect_id": prepared.parent_effect_id,
                "child_run_id": prepared.child.operation.run_id,
            }))
        }
        INTERACTION_RESOLUTION => {
            let command: InteractionResolutionCommand =
                serde_json::from_str(&normalized).map_err(|_| PrebetaError::InvalidShape)?;
            Ok(serde_json::json!({
                "kind": kind,
                "status": "applied",
                "run_id": command.locator.run_id,
                "session_id": command.locator.session_id,
            }))
        }
        EXTERNAL_EFFECT_COMPLETION => {
            let command: ExternalEffectCompletionCommand =
                serde_json::from_str(&normalized).map_err(|_| PrebetaError::InvalidShape)?;
            Ok(serde_json::json!({
                "kind": kind,
                "status": "applied",
                "run_id": command.locator.run_id,
                "session_id": command.locator.session_id,
            }))
        }
        _ => Err(PrebetaError::UnsupportedKind),
    }
}

/// Stable pre-beta normalization failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrebetaError {
    /// Unknown kind string.
    UnsupportedKind,
    /// JSON failed Rust DTO validation.
    InvalidShape,
}

impl PrebetaError {
    /// Stable reason suitable for a TypeError-equivalent.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::UnsupportedKind => "unsupported pre-beta shape",
            Self::InvalidShape => "invalid pre-beta shape",
        }
    }
}

fn normalize_shape<T>(encoded: &str) -> Result<String, PrebetaError>
where
    T: DeserializeOwned + Serialize,
{
    let value: T = serde_json::from_str(encoded).map_err(|_| PrebetaError::InvalidShape)?;
    serde_json::to_string(&value).map_err(|_| PrebetaError::InvalidShape)
}

#[cfg(test)]
mod tests {
    use super::{
        CHILD_RUN_PREPARED, INTERACTION_RESOLUTION, apply_prebeta_command, normalize_prebeta_shape,
    };

    const CHILD: &str = r#"{
        "parent_run_id": "01234567-89ab-7cde-89ab-0123456789ab",
        "parent_effect_id": "01234567-89ab-7cde-89ab-0123456789ad",
        "child": {
            "operation": {
                "tenant_scope": "tenant-a",
                "session_id": "01234567-89ab-7cde-89ab-0123456789ae",
                "lane_id": "01234567-89ab-7cde-89ab-0123456789af",
                "run_id": "01234567-89ab-7cde-89ab-0123456789b0"
            }
        },
        "request_digest": "0000000000000000000000000000000000000000000000000000000000000000",
        "placement": "compatible_lane_in_parent_session"
    }"#;

    #[test]
    fn child_run_prepared_round_trips() {
        let normalized = normalize_prebeta_shape(CHILD_RUN_PREPARED, CHILD).expect("normalize");
        let value: serde_json::Value = serde_json::from_str(&normalized).expect("json");
        let original: serde_json::Value = serde_json::from_str(CHILD).expect("original");
        assert_eq!(value, original);
        let applied = apply_prebeta_command(CHILD_RUN_PREPARED, CHILD).expect("apply");
        assert_eq!(applied["status"], "applied");
        assert_eq!(applied["kind"], CHILD_RUN_PREPARED);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut value: serde_json::Value = serde_json::from_str(CHILD).expect("json");
        value
            .as_object_mut()
            .expect("object")
            .insert("unexpected".into(), serde_json::Value::Bool(true));
        let encoded = serde_json::to_string(&value).expect("encode");
        let error = normalize_prebeta_shape(CHILD_RUN_PREPARED, &encoded).expect_err("reject");
        assert_eq!(error.reason(), "invalid pre-beta shape");
    }

    #[test]
    fn unsupported_kind_is_stable() {
        let error = normalize_prebeta_shape("not_a_kind", "{}").expect_err("kind");
        assert_eq!(error.reason(), "unsupported pre-beta shape");
        let _ = INTERACTION_RESOLUTION;
    }
}
