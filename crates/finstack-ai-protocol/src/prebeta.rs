//! Shared normalization for provisional lineage and authenticated command shapes.

use finstack_ai_kernel::{
    ChildRunPrepared, ExternalEffectCompletionCommand, InteractionResolutionCommand,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Child-run preparation shape kind.
pub const CHILD_RUN_PREPARED: &str = "child_run_prepared";
/// Interaction-resolution command shape kind.
pub const INTERACTION_RESOLUTION: &str = "interaction_resolution";
/// Authenticated external-effect completion shape kind.
pub const EXTERNAL_EFFECT_COMPLETION: &str = "external_effect_completion";

/// Normalize one provisional lineage or authenticated command shape.
///
/// # Errors
///
/// Returns a stable error when `kind` is unsupported or `encoded` fails the
/// canonical Rust DTO validation for that kind.
pub fn normalize_prebeta_shape(kind: &str, encoded: &str) -> Result<String, PrebetaError> {
    match kind {
        CHILD_RUN_PREPARED => normalize_shape::<ChildRunPrepared>(encoded),
        INTERACTION_RESOLUTION => normalize_shape::<InteractionResolutionCommand>(encoded),
        EXTERNAL_EFFECT_COMPLETION => normalize_shape::<ExternalEffectCompletionCommand>(encoded),
        _ => Err(PrebetaError::UnsupportedKind),
    }
}

/// Stable provisional shape-normalization failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrebetaError {
    /// Unknown shape kind.
    UnsupportedKind,
    /// JSON failed canonical Rust DTO validation.
    InvalidShape,
}

impl PrebetaError {
    /// Stable reason suitable for a binding `TypeError`.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::UnsupportedKind => "unsupported pre-beta shape",
            Self::InvalidShape => "invalid pre-beta shape",
        }
    }
}

impl core::fmt::Display for PrebetaError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.reason())
    }
}

impl std::error::Error for PrebetaError {}

fn normalize_shape<T>(encoded: &str) -> Result<String, PrebetaError>
where
    T: DeserializeOwned + Serialize,
{
    let value: T = serde_json::from_str(encoded).map_err(|_| PrebetaError::InvalidShape)?;
    serde_json::to_string(&value).map_err(|_| PrebetaError::InvalidShape)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn unknown_fields_and_kinds_have_stable_errors() {
        let mut value: serde_json::Value = serde_json::from_str(CHILD).expect("json");
        value
            .as_object_mut()
            .expect("object")
            .insert("unexpected".into(), serde_json::Value::Bool(true));
        let encoded = serde_json::to_string(&value).expect("encode");
        assert_eq!(
            normalize_prebeta_shape(CHILD_RUN_PREPARED, &encoded)
                .expect_err("unknown field")
                .reason(),
            "invalid pre-beta shape"
        );
        assert_eq!(
            normalize_prebeta_shape("not_a_kind", "{}")
                .expect_err("kind")
                .reason(),
            "unsupported pre-beta shape"
        );
    }
}
