//! URL construction, payload hashing, and response mapping for
//! [`crate::s3::store::S3ObjectStore`].
//!
//! Everything here is pure; the actual HTTP calls live in `store.rs`. Keeping
//! URL building free of `reqwest` request/response types keeps it trivially
//! unit-testable.

use std::sync::Arc;

use crate::driver::ObjectError;
use reqwest::{StatusCode, Url};
use sha2::{Digest as Sha2Digest, Sha256};

use crate::s3::config::{Addressing, S3ObjectStoreConfig};

/// A fully resolved request target: URL, canonical path, `Host` header
/// value, derived from the configured [`Addressing`] style.
pub struct RequestTarget {
    /// Complete request URL.
    pub url: Url,
    /// Absolute path component used both on the wire and in the `SigV4`
    /// canonical request.
    pub path: String,
    /// `Host` header value (and `SigV4` signed host).
    pub host: String,
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
    let port_suffix = endpoint
        .port()
        .map_or_else(String::new, |port| format!(":{port}"));
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
    let url =
        Url::parse(&format!("{scheme}://{host}{path}")).map_err(|_error| invalid_endpoint())?;
    Ok(RequestTarget { url, path, host })
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
        Addressing::VirtualHost => (
            format!("{}.{base_host}{port_suffix}", config.bucket()),
            "/".to_owned(),
        ),
    };

    let mut query_params: Vec<(&str, String)> = vec![
        ("list-type", "2".to_owned()),
        ("max-keys", "1000".to_owned()),
        ("prefix", prefix.to_owned()),
    ];
    if let Some(token) = continuation_token {
        query_params.push(("continuation-token", token.to_owned()));
    }
    query_params.sort_by(|left, right| left.0.cmp(right.0));
    let canonical_query: String = query_params
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                crate::s3::sigv4::uri_encode(name, true),
                crate::s3::sigv4::uri_encode(value, true)
            )
        })
        .collect::<Vec<_>>()
        .join("&");

    let url = Url::parse(&format!("{scheme}://{host}{path}?{canonical_query}"))
        .map_err(|_error| invalid_endpoint())?;
    Ok(ListTarget {
        url,
        path,
        host,
        canonical_query,
    })
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

/// Map a transport-level failure (connect refused, timeout, TLS, DNS, ...).
///
/// Never interpolates the underlying `reqwest` error text: it can carry the
/// full request URL.
#[must_use]
pub fn map_transport_error() -> ObjectError {
    ObjectError::Unavailable {
        message: Arc::from("transport_failure"),
    }
}

/// Map a non-2xx HTTP status to the exact [`ObjectError`] the contract requires.
#[must_use]
pub fn map_status_error(status: StatusCode) -> ObjectError {
    match status {
        StatusCode::NOT_FOUND => ObjectError::NotFound,
        StatusCode::FORBIDDEN => ObjectError::Unavailable {
            message: Arc::from("access_denied"),
        },
        other => ObjectError::Unavailable {
            message: Arc::from(format!("http_{}", other.as_u16())),
        },
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest as Sha2Digest, Sha256};

    use super::{payload_sha256_hex, to_hex};

    #[test]
    fn payload_sha256_hex_matches_the_known_empty_string_hash() {
        assert_eq!(
            payload_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn to_hex_matches_sha256_known_answer() {
        assert_eq!(
            to_hex(&Sha256::digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
