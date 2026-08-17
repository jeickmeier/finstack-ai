//! Shared reference validation errors and label/micros helpers.

use std::sync::Arc;

use serde::Deserialize;
use serde::de;
use thiserror::Error;

use crate::content::TEXT_MAX_BYTES;

/// Shared reference validation errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RefsError {
    /// Label failed non-empty / size / NUL checks.
    #[error("invalid {field} label")]
    InvalidLabel {
        /// Field name.
        field: &'static str,
    },
    /// Serialization failed while building canonical bytes.
    #[error("serialize failed: {detail}")]
    Serialize {
        /// Detail.
        detail: String,
    },
    /// Semantic map exceeded its v1 entry ceiling.
    #[error("{field} has {len} entries; max {max}")]
    TooManyEntries {
        /// Field name.
        field: &'static str,
        /// Observed entry count.
        len: usize,
        /// Maximum entry count.
        max: usize,
    },
    /// Semantic array exceeded its v1 item ceiling.
    #[error("{field} has {len} items; max {max}")]
    TooManyItems {
        /// Field name.
        field: &'static str,
        /// Observed item count.
        len: usize,
        /// Maximum item count.
        max: usize,
    },
}

impl RefsError {
    /// Stable error code for fixtures.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLabel { .. } => "invalid_label",
            Self::Serialize { .. } => "serialize_failed",
            Self::TooManyEntries { .. } => "too_many_entries",
            Self::TooManyItems { .. } => "too_many_items",
        }
    }
}

pub(crate) fn validated_label(value: &str, field: &'static str) -> Result<Arc<str>, RefsError> {
    if !crate::label_is_valid(value) {
        return Err(RefsError::InvalidLabel { field });
    }
    Ok(Arc::<str>::from(value))
}

pub(crate) fn validated_text(value: &str, field: &'static str) -> Result<Arc<str>, RefsError> {
    if value.is_empty() || value.len() > TEXT_MAX_BYTES || value.as_bytes().contains(&0) {
        return Err(RefsError::InvalidLabel { field });
    }
    Ok(Arc::<str>::from(value))
}

pub(super) fn validate_label_ref<E>(value: &str, field: &'static str) -> Result<(), E>
where
    E: serde::ser::Error,
{
    if !crate::label_is_valid(value) {
        return Err(E::custom(RefsError::InvalidLabel { field }));
    }
    Ok(())
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(super) fn serialize_micros<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

pub(super) fn deserialize_micros<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(de::Error::custom(
            "micros must be a canonical decimal string",
        ));
    }
    text.parse::<u64>()
        .map_err(|_| de::Error::custom("micros overflow or invalid"))
}
