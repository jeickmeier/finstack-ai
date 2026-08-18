//! Shared label and hex-nibble helpers.

use std::sync::Arc;

use crate::content::LABEL_MAX_BYTES;

/// Whether a semantic label is non-empty, bounded, and NUL-free.
///
/// # Arguments
///
/// * `value` - Candidate label. Must be 1..=[`LABEL_MAX_BYTES`] UTF-8 bytes and
///   must not contain a NUL byte.
///
/// # Examples
///
/// ```
/// assert!(finstack_ai_kernel::label_is_valid("gpt-4"));
/// assert!(!finstack_ai_kernel::label_is_valid(""));
/// ```
#[must_use]
pub fn label_is_valid(value: &str) -> bool {
    !value.is_empty() && value.len() <= LABEL_MAX_BYTES && !value.as_bytes().contains(&0)
}

/// Decode one ASCII hex nibble.
///
/// # Arguments
///
/// * `byte` - ASCII `0-9`, `a-f`, or `A-F`. Any other byte returns `None`.
///
/// # Examples
///
/// ```
/// assert_eq!(finstack_ai_kernel::hex_nibble(b'a'), Some(10));
/// assert_eq!(finstack_ai_kernel::hex_nibble(b'x'), None);
/// ```
#[must_use]
pub const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn validated_label<E>(
    value: &str,
    field: &'static str,
    invalid: impl FnOnce(&'static str) -> E,
) -> Result<Arc<str>, E> {
    if !label_is_valid(value) {
        return Err(invalid(field));
    }
    Ok(Arc::from(value))
}
