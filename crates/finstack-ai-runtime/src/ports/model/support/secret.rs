//! Shared model-adapter secret type. Leaf crates map [`SecretRejected`] to their
//! own stable error codes; this module does not own those codes.

use core::fmt;
use std::sync::Arc;

/// Maximum accepted provider secret length.
pub const SECRET_MAX_BYTES: usize = 16 * 1_024;

/// Why [`SecretString::try_new`] rejected a value.
///
/// Leaf crates map each variant to their own stable adapter code. This type
/// must not grow a `code` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretRejected {
    /// Empty secret.
    Empty,
    /// Longer than the 16 KiB secret ceiling.
    TooLong,
    /// Contains a NUL or a non-ASCII byte.
    NonAscii,
}

/// Opaque configured secret whose formatting is always redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(Arc<str>);

impl SecretString {
    /// Construct a non-empty bounded ASCII secret.
    ///
    /// # Errors
    ///
    /// Returns [`SecretRejected`] without a stable adapter code. Crate-root
    /// re-exports wait for the dedicated-provider credential wave.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, SecretRejected> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(SecretRejected::Empty);
        }
        if value.len() > SECRET_MAX_BYTES {
            return Err(SecretRejected::TooLong);
        }
        if !value.is_ascii() || value.as_bytes().contains(&0) {
            return Err(SecretRejected::NonAscii);
        }
        Ok(Self(Arc::from(value)))
    }

    /// Borrow the secret material. Callers must not log or persist it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

/// Whether a configured secret is non-empty, bounded, and NUL-free.
///
/// This helper keeps the historical NUL check used by existing adapters
/// (ADR-048). [`SecretString`] is stricter: it also rejects non-ASCII bytes.
///
/// ```
/// use finstack_ai_runtime::ports::model::secret_is_valid;
///
/// assert!(secret_is_valid("sk-test"));
/// assert!(!secret_is_valid(""));
/// ```
#[must_use]
pub fn secret_is_valid(value: &str) -> bool {
    !value.is_empty() && value.len() <= SECRET_MAX_BYTES && !value.as_bytes().contains(&0)
}

#[cfg(test)]
mod tests {
    use super::{SECRET_MAX_BYTES, SecretRejected, SecretString, secret_is_valid};

    #[test]
    fn secret_string_accepts_ascii_and_redacts_debug() {
        let secret = SecretString::try_new("sk-test").expect("secret");
        assert_eq!(secret.expose(), "sk-test");
        assert_eq!(format!("{secret:?}"), "SecretString([REDACTED])");
    }

    #[test]
    fn secret_string_rejects_empty_oversized_and_non_ascii() {
        assert_eq!(SecretString::try_new(""), Err(SecretRejected::Empty));
        assert_eq!(
            SecretString::try_new("x".repeat(SECRET_MAX_BYTES + 1)),
            Err(SecretRejected::TooLong)
        );
        assert_eq!(
            SecretString::try_new("sk\0key"),
            Err(SecretRejected::NonAscii)
        );
        assert_eq!(SecretString::try_new("clé"), Err(SecretRejected::NonAscii));
    }

    #[test]
    fn secret_is_valid_keeps_historical_nul_predicate() {
        assert!(secret_is_valid("sk-test"));
        assert!(secret_is_valid("clé"));
        assert!(!secret_is_valid(""));
        assert!(!secret_is_valid("sk\0key"));
        assert!(!secret_is_valid(&"x".repeat(SECRET_MAX_BYTES + 1)));
    }
}
