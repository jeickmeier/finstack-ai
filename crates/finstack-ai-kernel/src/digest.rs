//! Domain-separated SHA-256 digests and JCS known-answer helpers.

use core::fmt;
use core::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use thiserror::Error;

/// Fixed domain name for `RawJson` / `Metadata` digests (TDD §6.4).
pub const DOMAIN_RAW_JSON: &str = "raw-json";
/// Schema version embedded in the current `RawJson` digest domain.
pub const RAW_JSON_DIGEST_SCHEMA_VERSION: u32 = 1;

/// 32-byte SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest(
    #[serde(serialize_with = "serialize_hex", deserialize_with = "deserialize_hex")] [u8; 32],
);

impl Digest {
    /// Borrow the raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase 64-character hexadecimal encoding.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// Parse a lowercase or uppercase 64-character hex digest.
    ///
    /// # Errors
    ///
    /// Returns [`DigestError::InvalidHex`] for malformed input.
    pub fn from_hex(input: &str) -> Result<Self, DigestError> {
        if input.len() != 64 || !input.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(DigestError::InvalidHex {
                input: input.to_owned(),
            });
        }
        let mut bytes = [0_u8; 32];
        for (idx, chunk) in input.as_bytes().chunks_exact(2).enumerate() {
            bytes[idx] = (hex_nibble(chunk[0])? << 4) | hex_nibble(chunk[1])?;
        }
        Ok(Self(bytes))
    }

    /// Domain-separated SHA-256 over canonical bytes.
    ///
    /// Encoding:
    /// `"finstack-ai" NUL domain-name NUL schema-version-u32-be NUL canonical-bytes`
    ///
    /// Domain names must be non-empty and must not contain NUL bytes so the
    /// encoding remains unambiguous.
    ///
    /// # Errors
    ///
    /// Returns [`DigestError::InvalidDomain`] when `domain` is empty or contains NUL.
    pub fn domain_separated(
        domain: &str,
        schema_version: u32,
        canonical_bytes: &[u8],
    ) -> Result<Self, DigestError> {
        validate_domain(domain)?;
        let mut hasher = Sha256::new();
        hasher.update(b"finstack-ai");
        hasher.update([0]);
        hasher.update(domain.as_bytes());
        hasher.update([0]);
        hasher.update(schema_version.to_be_bytes());
        hasher.update([0]);
        hasher.update(canonical_bytes);
        Ok(Self(hasher.finalize().into()))
    }

    /// Digest for RFC 8785 canonical JSON under the `raw-json` domain.
    ///
    /// # Panics
    ///
    /// Panics only if the fixed [`DOMAIN_RAW_JSON`] registry entry were invalid.
    #[must_use]
    pub fn raw_json(canonical_bytes: &[u8]) -> Self {
        Self::domain_separated(
            DOMAIN_RAW_JSON,
            RAW_JSON_DIGEST_SCHEMA_VERSION,
            canonical_bytes,
        )
        .expect("fixed raw-json domain is valid")
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Digest").field(&self.to_hex()).finish()
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Digest parsing failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DigestError {
    /// Hex text was not 64 hexadecimal characters.
    #[error("invalid digest hex: {input}")]
    InvalidHex {
        /// Rejected input.
        input: String,
    },
    /// Domain name was empty or contained a NUL byte.
    #[error("invalid digest domain: {domain:?}")]
    InvalidDomain {
        /// Rejected domain text.
        domain: String,
    },
}

fn validate_domain(domain: &str) -> Result<(), DigestError> {
    if domain.is_empty() || domain.as_bytes().contains(&0) {
        return Err(DigestError::InvalidDomain {
            domain: domain.to_owned(),
        });
    }
    Ok(())
}

fn hex_nibble(byte: u8) -> Result<u8, DigestError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(DigestError::InvalidHex {
            input: String::from_utf8_lossy(&[byte]).into_owned(),
        }),
    }
}

fn serialize_hex<S>(value: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&Digest(*value).to_hex())
}

fn deserialize_hex<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    Digest::from_hex(&text)
        .map(|digest| *digest.as_bytes())
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separation_changes_digest() {
        let payload = br#"{"a":1}"#;
        let a = Digest::domain_separated("raw-json", 1, payload).expect("a");
        let b = Digest::domain_separated("record-payload", 1, payload).expect("b");
        let c = Digest::domain_separated("raw-json", 2, payload).expect("c");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, Digest::raw_json(payload));
        assert_eq!(Digest::from_hex(&a.to_hex()).expect("hex"), a);
        assert!(matches!(
            Digest::domain_separated("raw\0json", 1, payload).expect_err("nul"),
            DigestError::InvalidDomain { .. }
        ));
        assert!(matches!(
            Digest::domain_separated("", 1, payload).expect_err("empty"),
            DigestError::InvalidDomain { .. }
        ));
    }
}
