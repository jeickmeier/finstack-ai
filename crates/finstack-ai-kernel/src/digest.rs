//! Domain-separated SHA-256 digests and JCS known-answer helpers.

use core::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use thiserror::Error;

/// Fixed domain name for `RawJson` / `Metadata` digests (TDD §6.4).
pub const DOMAIN_RAW_JSON: &str = "raw-json";
/// Schema version embedded in the current `RawJson` digest domain.
pub const RAW_JSON_DIGEST_SCHEMA_VERSION: u32 = 1;
/// Fixed domain name for blob-content digests (TDD §6.4 / §7.2).
pub const DOMAIN_BLOB_CONTENT: &str = "blob-content";
/// Schema version embedded in the current blob-content digest domain.
pub const BLOB_CONTENT_DIGEST_SCHEMA_VERSION: u32 = 1;
/// Fixed domain name for record-payload digests (TDD §6.4; bytes owned by PR-039).
pub const DOMAIN_RECORD_PAYLOAD: &str = "record-payload";
/// Schema version for the `record-payload` digest domain.
pub const RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION: u32 = 1;
/// Fixed domain name for effect-input digests (TDD §6.4 / §12.3).
pub const DOMAIN_EFFECT_INPUT: &str = "effect-input";
/// Schema version for the `effect-input` digest domain.
pub const EFFECT_INPUT_DIGEST_SCHEMA_VERSION: u32 = 1;
/// Fixed domain name for effect-output digests (TDD §6.4 / §12.3).
pub const DOMAIN_EFFECT_OUTPUT: &str = "effect-output";
/// Schema version for the `effect-output` digest domain.
pub const EFFECT_OUTPUT_DIGEST_SCHEMA_VERSION: u32 = 1;
/// Fixed domain name for snapshot-state digests (TDD §6.4).
pub const DOMAIN_SNAPSHOT_STATE: &str = "snapshot-state";
/// Schema version for the `snapshot-state` digest domain.
pub const SNAPSHOT_STATE_DIGEST_SCHEMA_VERSION: u32 = 1;
/// Fixed domain name for middleware-chain digests (TDD §6.4).
pub const DOMAIN_MIDDLEWARE_CHAIN: &str = "middleware-chain";
/// Schema version for the `middleware-chain` digest domain.
pub const MIDDLEWARE_CHAIN_DIGEST_SCHEMA_VERSION: u32 = 1;
/// Fixed domain name for agent-spec digests (TDD §6.4).
pub const DOMAIN_AGENT_SPEC: &str = "agent-spec";
/// Schema version for the `agent-spec` digest domain.
pub const AGENT_SPEC_DIGEST_SCHEMA_VERSION: u32 = 1;

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
        let mut buffer = HexBuffer::new();
        buffer.encode(&self.0).to_owned()
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

    /// Digest for exact raw blob bytes under the `blob-content` domain.
    ///
    /// # Panics
    ///
    /// Panics only if the fixed [`DOMAIN_BLOB_CONTENT`] registry entry were invalid.
    #[must_use]
    pub fn blob_content(raw_bytes: &[u8]) -> Self {
        Self::domain_separated(
            DOMAIN_BLOB_CONTENT,
            BLOB_CONTENT_DIGEST_SCHEMA_VERSION,
            raw_bytes,
        )
        .expect("fixed blob-content domain is valid")
    }

    /// Digest under the `effect-input` domain.
    ///
    /// # Panics
    ///
    /// Panics only if the fixed [`DOMAIN_EFFECT_INPUT`] registry entry were invalid.
    #[must_use]
    pub fn effect_input(canonical_bytes: &[u8]) -> Self {
        Self::domain_separated(
            DOMAIN_EFFECT_INPUT,
            EFFECT_INPUT_DIGEST_SCHEMA_VERSION,
            canonical_bytes,
        )
        .expect("fixed effect-input domain is valid")
    }

    /// Digest under the `effect-output` domain.
    ///
    /// # Panics
    ///
    /// Panics only if the fixed [`DOMAIN_EFFECT_OUTPUT`] registry entry were invalid.
    #[must_use]
    pub fn effect_output(canonical_bytes: &[u8]) -> Self {
        Self::domain_separated(
            DOMAIN_EFFECT_OUTPUT,
            EFFECT_OUTPUT_DIGEST_SCHEMA_VERSION,
            canonical_bytes,
        )
        .expect("fixed effect-output domain is valid")
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buffer = HexBuffer::new();
        f.debug_tuple("Digest")
            .field(&buffer.encode(&self.0))
            .finish()
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buffer = HexBuffer::new();
        f.write_str(buffer.encode(&self.0))
    }
}

/// Chunk size batching canonicalizer writes into the hasher.
///
/// The JCS formatter emits a great many very small writes (one per token,
/// separator, and escaped fragment). Feeding those to SHA-256 individually
/// measured ~20% slower than hashing one contiguous buffer, because the
/// per-update overhead dominates. Batching recovers bulk-hash throughput
/// without ever allocating a full-size output buffer.
const DIGEST_CHUNK_BYTES: usize = 8192;

/// Incremental domain-separated hasher that also counts the bytes it consumes.
///
/// Canonicalizing into a `Vec` and hashing it afterwards grew a fresh
/// full-size buffer per digest, and reallocation was among the hottest paths
/// in the reducer. This hashes through a fixed-size chunk instead; the byte
/// count comes along for free, which is what limit accounting needs.
pub(crate) struct DigestWriter {
    hasher: Sha256,
    chunk: Vec<u8>,
    written: usize,
}

impl DigestWriter {
    /// Start a digest over `domain` and `schema_version`.
    ///
    /// # Errors
    ///
    /// Returns [`DigestError::InvalidDomain`] when `domain` is empty or NUL-bearing.
    pub(crate) fn new(domain: &str, schema_version: u32) -> Result<Self, DigestError> {
        validate_domain(domain)?;
        let mut hasher = Sha256::new();
        hasher.update(b"finstack-ai");
        hasher.update([0]);
        hasher.update(domain.as_bytes());
        hasher.update([0]);
        hasher.update(schema_version.to_be_bytes());
        hasher.update([0]);
        Ok(Self {
            hasher,
            chunk: Vec::with_capacity(DIGEST_CHUNK_BYTES),
            written: 0,
        })
    }

    /// Finish the digest and report how many canonical bytes were hashed.
    pub(crate) fn finish(mut self) -> (Digest, usize) {
        self.hasher.update(&self.chunk);
        (Digest(self.hasher.finalize().into()), self.written)
    }
}

impl std::io::Write for DigestWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.chunk.len().saturating_add(buf.len()) > DIGEST_CHUNK_BYTES {
            self.hasher.update(&self.chunk);
            self.chunk.clear();
        }
        if buf.len() >= DIGEST_CHUNK_BYTES {
            // Oversized writes bypass the chunk rather than growing it.
            self.hasher.update(buf);
        } else {
            self.chunk.extend_from_slice(buf);
        }
        self.written = self.written.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Lowercase hex nibble table shared by digest and UUID encoding.
pub(crate) const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Stack buffer for 64-character digest hex text.
///
/// Encoding writes through [`HEX_DIGITS`] rather than 32 `write!` calls, which
/// avoids a heap allocation and 32 `core::fmt` dispatches per serialized
/// digest. Digests appear in nearly every canonical projection field.
pub(crate) struct HexBuffer([u8; 64]);

impl HexBuffer {
    pub(crate) const fn new() -> Self {
        Self([0; 64])
    }

    pub(crate) fn encode(&mut self, bytes: &[u8; 32]) -> &str {
        for (index, byte) in bytes.iter().enumerate() {
            self.0[index * 2] = HEX_DIGITS[usize::from(byte >> 4)];
            self.0[index * 2 + 1] = HEX_DIGITS[usize::from(byte & 0x0f)];
        }
        // Only ASCII hex digits are ever written.
        core::str::from_utf8(&self.0).expect("hex text is ASCII")
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
    let mut buffer = HexBuffer::new();
    serializer.serialize_str(buffer.encode(value))
}

fn deserialize_hex<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct HexVisitor;

    impl serde::de::Visitor<'_> for HexVisitor {
        type Value = [u8; 32];

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a 64-character hexadecimal digest string")
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
            Digest::from_hex(value)
                .map(|digest| *digest.as_bytes())
                .map_err(E::custom)
        }
    }

    deserializer.deserialize_str(HexVisitor)
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
