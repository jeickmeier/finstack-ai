//! URL construction, payload hashing, and response mapping for
//! [`crate::store::S3ObjectStore`].
//!
//! Everything here is pure or local I/O only; the actual HTTP calls live in
//! `store.rs`. Keeping URL building and XML parsing free of `reqwest`
//! request/response types keeps them trivially unit-testable.

use std::sync::Arc;

use finstack_ai_kernel::Digest;
use finstack_ai_runtime::ObjectError;
use reqwest::{StatusCode, Url};
use sha2::{Digest as Sha2Digest, Sha256};

use crate::config::{Addressing, S3ObjectStoreConfig};

/// Fixed domain name backing [`finstack_ai_kernel::Digest::blob_content`].
///
/// Replicated here (rather than imported) because the kernel's incremental
/// digest writer is crate-private; streaming a large file through
/// [`BlobDigestHasher`] without ever materializing it in memory requires
/// reproducing the exact same domain-separation prefix independently. The
/// two must always agree, which is asserted by
/// [`blob_digest_hasher_matches_digest_blob_content`] below.
const BLOB_CONTENT_DOMAIN: &str = "blob-content";
/// Schema version backing [`finstack_ai_kernel::Digest::blob_content`].
const BLOB_CONTENT_SCHEMA_VERSION: u32 = 1;

/// A fully resolved request target: URL, canonical path, `Host` header
/// value, and scheme, derived from the configured [`Addressing`] style.
pub struct RequestTarget {
    /// Complete request URL.
    pub url: Url,
    /// Absolute path component used both on the wire and in the `SigV4`
    /// canonical request.
    pub path: String,
    /// `Host` header value (and `SigV4` signed host).
    pub host: String,
    /// URL scheme (`http` or `https`).
    pub scheme: String,
}

fn invalid_endpoint() -> ObjectError {
    ObjectError::InvalidMetadata {
        message: Arc::from("invalid_endpoint"),
    }
}

fn split_endpoint(config: &S3ObjectStoreConfig) -> Result<(String, String, String), ObjectError> {
    let endpoint = Url::parse(config.endpoint()).map_err(|_error| invalid_endpoint())?;
    let scheme = endpoint.scheme().to_owned();
    let base_host = endpoint.host_str().ok_or_else(invalid_endpoint)?.to_owned();
    let port_suffix = endpoint.port().map_or_else(String::new, |port| format!(":{port}"));
    Ok((scheme, base_host, port_suffix))
}

/// Build the URL, canonical path, and `Host` header for one object key.
///
/// # Errors
///
/// Returns [`ObjectError::InvalidMetadata`] when the configured endpoint
/// cannot be parsed.
pub fn object_url(
    config: &S3ObjectStoreConfig,
    physical_key: &str,
) -> Result<RequestTarget, ObjectError> {
    let (scheme, base_host, port_suffix) = split_endpoint(config)?;
    let (host, path) = match config.addressing() {
        Addressing::Path => (
            format!("{base_host}{port_suffix}"),
            format!("/{}/{physical_key}", config.bucket()),
        ),
        Addressing::VirtualHost => (
            format!("{}.{base_host}{port_suffix}", config.bucket()),
            format!("/{physical_key}"),
        ),
    };
    let url = Url::parse(&format!("{scheme}://{host}{path}")).map_err(|_error| invalid_endpoint())?;
    Ok(RequestTarget { url, path, host, scheme })
}

/// A resolved `ListObjectsV2` request target, including the canonical query
/// string used both on the wire and for `SigV4` signing.
pub struct ListTarget {
    /// Complete request URL.
    pub url: Url,
    /// Absolute path component (bucket root).
    pub path: String,
    /// `Host` header value.
    pub host: String,
    /// Canonical, sorted, `SigV4`-encoded query string (no leading `?`).
    pub canonical_query: String,
}

/// Build the `ListObjectsV2` URL and canonical query for one page.
///
/// # Errors
///
/// Returns [`ObjectError::InvalidMetadata`] when the configured endpoint
/// cannot be parsed.
pub fn list_url(
    config: &S3ObjectStoreConfig,
    prefix: &str,
    continuation_token: Option<&str>,
) -> Result<ListTarget, ObjectError> {
    let (scheme, base_host, port_suffix) = split_endpoint(config)?;
    let (host, path) = match config.addressing() {
        Addressing::Path => (
            format!("{base_host}{port_suffix}"),
            format!("/{}", config.bucket()),
        ),
        Addressing::VirtualHost => (format!("{}.{base_host}{port_suffix}", config.bucket()), "/".to_owned()),
    };

    let mut query_params: Vec<(&str, String)> =
        vec![("list-type", "2".to_owned()), ("prefix", prefix.to_owned())];
    if let Some(token) = continuation_token {
        query_params.push(("continuation-token", token.to_owned()));
    }
    query_params.sort_by(|left, right| left.0.cmp(right.0));
    let canonical_query: String = query_params
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                crate::sigv4::uri_encode(name, true),
                crate::sigv4::uri_encode(value, true)
            )
        })
        .collect::<Vec<_>>()
        .join("&");

    let url = Url::parse(&format!("{scheme}://{host}{path}?{canonical_query}"))
        .map_err(|_error| invalid_endpoint())?;
    Ok(ListTarget { url, path, host, canonical_query })
}

/// SHA-256 hex of `data`, used as the `x-amz-content-sha256` payload hash.
///
/// This is the plain (non-domain-separated) SHA-256 `SigV4` requires; it is
/// intentionally distinct from [`finstack_ai_kernel::Digest::blob_content`].
#[must_use]
pub fn payload_sha256_hex(data: &[u8]) -> String {
    to_hex(&Sha256::digest(data))
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Incremental SHA-256 hex hasher for the plain `x-amz-content-sha256`
/// payload hash, distinct from [`BlobDigestHasher`] which computes the
/// domain-separated content digest.
pub struct StreamingSha256(Sha256);

impl StreamingSha256 {
    /// Start a new streaming payload hash.
    #[must_use]
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    /// Fold in the next chunk, in order.
    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk);
    }

    /// Finish, producing the lowercase hex digest.
    #[must_use]
    pub fn finish(self) -> String {
        to_hex(&self.0.finalize())
    }
}

impl Default for StreamingSha256 {
    fn default() -> Self {
        Self::new()
    }
}

/// Incremental hasher producing exactly the digest
/// [`finstack_ai_kernel::Digest::blob_content`] would over the same bytes,
/// without ever holding those bytes in memory at once.
pub struct BlobDigestHasher {
    hasher: Sha256,
    len: u64,
}

impl BlobDigestHasher {
    /// Start a new streaming blob-content digest.
    #[must_use]
    pub fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"finstack-ai");
        hasher.update([0_u8]);
        hasher.update(BLOB_CONTENT_DOMAIN.as_bytes());
        hasher.update([0_u8]);
        hasher.update(BLOB_CONTENT_SCHEMA_VERSION.to_be_bytes());
        hasher.update([0_u8]);
        Self { hasher, len: 0 }
    }

    /// Fold in the next chunk of raw object bytes, in order.
    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
        self.len = self.len.saturating_add(chunk.len() as u64);
    }

    /// Bytes folded in so far.
    #[must_use]
    pub const fn bytes_written(&self) -> u64 {
        self.len
    }

    /// Finish, producing the digest and total byte count.
    ///
    /// # Errors
    ///
    /// Returns [`ObjectError::Io`] only if the computed hash is somehow not
    /// valid hex, which cannot happen for a 32-byte SHA-256 output; kept as
    /// a `Result` so callers never need `unwrap`.
    pub fn finish(self) -> Result<(Digest, u64), ObjectError> {
        let hex = to_hex(&self.hasher.finalize());
        let digest = Digest::from_hex(&hex).map_err(|_error| ObjectError::Io {
            message: Arc::from("digest_hex_encode_failed"),
        })?;
        Ok((digest, self.len))
    }
}

impl Default for BlobDigestHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a transport-level failure (connect refused, timeout, TLS, DNS, ...).
///
/// Never interpolates the underlying `reqwest` error text: it can carry the
/// full request URL, including a presigned query string's signature.
#[must_use]
pub fn map_transport_error() -> ObjectError {
    ObjectError::Unavailable { message: Arc::from("transport_failure") }
}

/// Map a non-2xx HTTP status to the exact [`ObjectError`] the contract requires.
#[must_use]
pub fn map_status_error(status: StatusCode) -> ObjectError {
    match status {
        StatusCode::NOT_FOUND => ObjectError::NotFound,
        StatusCode::FORBIDDEN => ObjectError::Unavailable { message: Arc::from("access_denied") },
        other => ObjectError::Unavailable { message: Arc::from(format!("http_{}", other.as_u16())) },
    }
}

/// Extract the text content of every `<tag>...</tag>` occurrence in `body`,
/// decoding the five predefined XML entities (`&amp; &lt; &gt; &quot; &#39;`).
///
/// A hand-written scanner rather than a real XML parser: `ListObjectsV2`
/// responses are a small, fixed, well-known shape, and this avoids taking on
/// an XML dependency for five tag names.
#[must_use]
pub fn extract_tag_values(body: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        let after_open = rest.get(start + open.len()..).unwrap_or_default();
        let Some(end) = after_open.find(&close) else {
            break;
        };
        let raw = after_open.get(..end).unwrap_or_default();
        out.push(decode_entities(raw));
        rest = after_open.get(end + close.len()..).unwrap_or_default();
    }
    out
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut remaining = input;
    loop {
        if let Some(next) = remaining.strip_prefix("&amp;") {
            out.push('&');
            remaining = next;
        } else if let Some(next) = remaining.strip_prefix("&lt;") {
            out.push('<');
            remaining = next;
        } else if let Some(next) = remaining.strip_prefix("&gt;") {
            out.push('>');
            remaining = next;
        } else if let Some(next) = remaining.strip_prefix("&quot;") {
            out.push('"');
            remaining = next;
        } else if let Some(next) = remaining.strip_prefix("&#39;") {
            out.push('\'');
            remaining = next;
        } else {
            let mut chars = remaining.chars();
            match chars.next() {
                Some(ch) => {
                    out.push(ch);
                    remaining = chars.as_str();
                }
                None => break,
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::Digest;
    use sha2::{Digest as Sha2Digest, Sha256};

    use super::{BlobDigestHasher, decode_entities, extract_tag_values, payload_sha256_hex, to_hex};

    #[test]
    fn extract_tag_values_reads_repeated_tags_and_decodes_entities() {
        let body = "<a><Key>docs/&amp;a.pdf</Key><Size>10</Size></a>\
                     <a><Key>docs/b.pdf</Key><Size>20</Size></a>";
        assert_eq!(extract_tag_values(body, "Key"), vec!["docs/&a.pdf", "docs/b.pdf"]);
        assert_eq!(extract_tag_values(body, "Size"), vec!["10", "20"]);
        assert!(extract_tag_values(body, "Missing").is_empty());
    }

    #[test]
    fn decode_entities_handles_all_five_predefined_entities() {
        assert_eq!(decode_entities("&amp;&lt;&gt;&quot;&#39;"), "&<>\"'");
        assert_eq!(decode_entities("plain"), "plain");
    }

    #[test]
    fn payload_sha256_hex_matches_the_known_empty_string_hash() {
        assert_eq!(
            payload_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn blob_digest_hasher_matches_digest_blob_content() {
        let content = b"hello world, streamed in pieces";
        let mut hasher = BlobDigestHasher::new();
        hasher.update(&content[..10]);
        hasher.update(&content[10..]);
        let (streamed, len) = hasher.finish().expect("finish");
        assert_eq!(len, content.len() as u64);
        assert_eq!(streamed, Digest::blob_content(content));
    }

    #[test]
    fn to_hex_matches_sha256_known_answer() {
        assert_eq!(
            to_hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
