//! Explicit signing configuration for the completion ingress.

use std::fmt;
use std::sync::Arc;

use finstack_ai_runtime::SecretString;
use thiserror::Error;

/// Minimum accepted signing-key length in bytes.
pub const MIN_KEY_BYTES: usize = 32;
/// Maximum total active plus rotation verification keys.
pub const MAX_VERIFICATION_KEYS: usize = 8;

const KEY_ID_MAX_BYTES: usize = 128;

/// Explicit signing configuration. Never read from the environment.
#[derive(Clone)]
pub struct CompletionIngressConfig {
    /// Active signing key id, stamped into minted tokens.
    pub key_id: String,
    /// Active signing key (at least [`MIN_KEY_BYTES`] bytes).
    pub key: SecretString,
    /// Additional accepted verification keys for rotation.
    pub additional_verification_keys: Vec<(String, SecretString)>,
}

impl fmt::Debug for CompletionIngressConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompletionIngressConfig")
            .field("key_id", &self.key_id)
            .field("key", &"[REDACTED]")
            .field(
                "additional_verification_keys",
                &self
                    .additional_verification_keys
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Why a [`CompletionIngressConfig`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CompletionIngressConfigError {
    /// A key id was empty, oversize, or contained NUL/non-ASCII bytes.
    #[error("invalid key id")]
    InvalidKeyId,
    /// A key was shorter than [`MIN_KEY_BYTES`].
    #[error("signing key shorter than the 32-byte minimum")]
    KeyTooShort,
    /// Two keys shared one id.
    #[error("duplicate key id")]
    DuplicateKeyId,
    /// Active plus rotation keys exceeded [`MAX_VERIFICATION_KEYS`].
    #[error("too many verification keys")]
    TooManyVerificationKeys,
}

#[derive(Debug)]
pub(crate) struct ResolvedKeys {
    /// Active signing key first; all entries accepted for verification.
    /// The first entry's id is the `kid` stamped into minted tokens.
    pub(crate) keys: Vec<(Arc<str>, SecretString)>,
}

fn valid_key_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= KEY_ID_MAX_BYTES
        && id.is_ascii()
        && !id.bytes().any(|byte| byte == 0)
}

pub(crate) fn validated_keys(
    config: &CompletionIngressConfig,
) -> Result<ResolvedKeys, CompletionIngressConfigError> {
    if config.additional_verification_keys.len().saturating_add(1) > MAX_VERIFICATION_KEYS {
        return Err(CompletionIngressConfigError::TooManyVerificationKeys);
    }
    let mut keys: Vec<(Arc<str>, SecretString)> = Vec::new();
    let mut push = |id: &str, key: &SecretString| {
        if !valid_key_id(id) {
            return Err(CompletionIngressConfigError::InvalidKeyId);
        }
        if key.expose().len() < MIN_KEY_BYTES {
            return Err(CompletionIngressConfigError::KeyTooShort);
        }
        if keys.iter().any(|(existing, _)| existing.as_ref() == id) {
            return Err(CompletionIngressConfigError::DuplicateKeyId);
        }
        keys.push((Arc::from(id), key.clone()));
        Ok(())
    };
    push(&config.key_id, &config.key)?;
    for (id, key) in &config.additional_verification_keys {
        push(id, key)?;
    }
    Ok(ResolvedKeys { keys })
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_runtime::SecretString;

    fn key(byte: u8) -> SecretString {
        SecretString::try_new(String::from_utf8(vec![byte; 32]).expect("ascii")).expect("secret")
    }

    #[test]
    fn accepts_active_key_and_rotation_keys() {
        let config = CompletionIngressConfig {
            key_id: "k-active".to_owned(),
            key: key(b'a'),
            additional_verification_keys: vec![("k-old".to_owned(), key(b'b'))],
        };
        let resolved = validated_keys(&config).expect("valid");
        assert_eq!(resolved.keys.len(), 2);
        assert_eq!(resolved.keys[0].0.as_ref(), "k-active");
    }

    #[test]
    fn rejects_short_key_bad_id_and_duplicate_id() {
        let short = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: SecretString::try_new("tooshort").expect("secret"),
            additional_verification_keys: vec![],
        };
        assert_eq!(
            validated_keys(&short).expect_err("short"),
            CompletionIngressConfigError::KeyTooShort
        );
        let bad_id = CompletionIngressConfig {
            key_id: String::new(),
            key: key(b'a'),
            additional_verification_keys: vec![],
        };
        assert_eq!(
            validated_keys(&bad_id).expect_err("id"),
            CompletionIngressConfigError::InvalidKeyId
        );
        let dup = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: key(b'a'),
            additional_verification_keys: vec![("k1".to_owned(), key(b'b'))],
        };
        assert_eq!(
            validated_keys(&dup).expect_err("dup"),
            CompletionIngressConfigError::DuplicateKeyId
        );
    }

    #[test]
    fn debug_never_prints_key_material() {
        let config = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: key(b'a'),
            additional_verification_keys: vec![],
        };
        let printed = format!("{config:?}");
        assert!(!printed.contains('a'.to_string().repeat(32).as_str()));
        assert!(printed.contains("REDACTED"));
    }

    #[test]
    fn rejects_more_than_eight_total_verification_keys() {
        let config = CompletionIngressConfig {
            key_id: "active".to_owned(),
            key: key(b'a'),
            additional_verification_keys: (0..MAX_VERIFICATION_KEYS)
                .map(|index| (format!("old-{index}"), key(b'b')))
                .collect(),
        };
        assert_eq!(
            validated_keys(&config).expect_err("too many"),
            CompletionIngressConfigError::TooManyVerificationKeys
        );
    }
}
