//! Lossless diagnostic JSON / JSONL projection.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ProtocolError;

/// Serialize `value` with human-readable diagnostic JSON.
///
/// Typed `CostAmount.micros` stay canonical decimal strings. This is not a
/// journal encoding.
///
/// # Errors
///
/// Returns [`ProtocolError::Codec`] when JSON serialization fails.
pub fn to_diagnostic_json<T: Serialize + ?Sized>(value: &T) -> Result<String, ProtocolError> {
    serde_json::to_string(value).map_err(|error| ProtocolError::codec(error.to_string()))
}

/// Serialize `values` as JSONL (one diagnostic JSON object per line).
///
/// # Errors
///
/// Returns [`ProtocolError::Codec`] when JSON serialization fails.
pub fn to_diagnostic_jsonl<T: Serialize>(values: &[T]) -> Result<String, ProtocolError> {
    let mut out = String::new();
    for value in values {
        out.push_str(&to_diagnostic_json(value)?);
        out.push('\n');
    }
    Ok(out)
}

/// Parse one diagnostic JSON document into a typed value.
///
/// # Errors
///
/// Returns [`ProtocolError::Codec`] when JSON decoding fails.
pub fn from_diagnostic_json<T: DeserializeOwned>(text: &str) -> Result<T, ProtocolError> {
    serde_json::from_str(text).map_err(|error| ProtocolError::codec(error.to_string()))
}
