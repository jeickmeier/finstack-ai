//! Secret-safe S3-compatible object store configuration.

use core::fmt;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_runtime::artifact::ArtifactError;

use crate::artifact::map_object_error;
use crate::driver::ObjectError;
use finstack_ai_runtime::ports::model::SecretString;
use reqwest::Url;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_OBJECT_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const DEFAULT_PRESIGN_EXPIRY_MAX: Duration = Duration::from_hours(168);

/// Bucket path style used to build object URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Addressing {
    /// `https://endpoint/bucket/key` (default; works with most
    /// S3-compatible servers, including MinIO/Garage without extra DNS
    /// configuration).
    #[default]
    Path,
    /// `https://bucket.endpoint/key` (AWS S3 convention).
    VirtualHost,
}

/// Strict S3-compatible object store transport configuration.
#[derive(Clone)]
pub struct S3ObjectStoreConfig {
    endpoint: Arc<str>,
    bucket: Arc<str>,
    region: Arc<str>,
    key_prefix: Option<Arc<str>>,
    addressing: Addressing,
    access_key_id: Option<Arc<str>>,
    secret_access_key: Option<SecretString>,
    timeout: Duration,
    max_object_bytes: u64,
    presign_expiry_max: Duration,
}

impl fmt::Debug for S3ObjectStoreConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3ObjectStoreConfig")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("key_prefix", &self.key_prefix)
            .field("addressing", &self.addressing)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &self.secret_access_key)
            .field("timeout", &self.timeout)
            .field("max_object_bytes", &self.max_object_bytes)
            .field("presign_expiry_max", &self.presign_expiry_max)
            .finish()
    }
}

impl S3ObjectStoreConfig {
    /// Construct keyless configuration for one bucket.
    ///
    /// # Errors
    ///
    /// Rejects a non-http(s) endpoint, an endpoint carrying embedded
    /// credentials, a query, or a fragment; rejects an empty/NUL-bearing
    /// region; rejects a bucket that is empty, NUL-bearing, outside 3-63
    /// bytes, or outside the `[a-z0-9.-]` charset.
    pub fn try_new(
        endpoint: impl AsRef<str>,
        bucket: impl AsRef<str>,
        region: impl AsRef<str>,
    ) -> Result<Self, ArtifactError> {
        let endpoint = endpoint.as_ref();
        let bucket = bucket.as_ref();
        let region = region.as_ref();
        validate_endpoint(endpoint).map_err(map_object_error)?;
        validate_bucket(bucket).map_err(map_object_error)?;
        validate_region(region).map_err(map_object_error)?;
        Ok(Self {
            endpoint: Arc::from(endpoint),
            bucket: Arc::from(bucket),
            region: Arc::from(region),
            key_prefix: None,
            addressing: Addressing::default(),
            access_key_id: None,
            secret_access_key: None,
            timeout: DEFAULT_TIMEOUT,
            max_object_bytes: DEFAULT_MAX_OBJECT_BYTES,
            presign_expiry_max: DEFAULT_PRESIGN_EXPIRY_MAX,
        })
    }

    /// Set a validated physical key prefix ahead of the scope digest.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, relative, or otherwise unsafe segments.
    pub fn try_with_key_prefix(
        mut self,
        key_prefix: impl AsRef<str>,
    ) -> Result<Self, ArtifactError> {
        let key_prefix = key_prefix.as_ref();
        if key_prefix.is_empty()
            || key_prefix.len() > 447
            || key_prefix.split('/').any(|segment| {
                segment.is_empty()
                    || matches!(segment, "." | "..")
                    || !segment.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    })
            })
        {
            return Err(map_object_error(invalid("invalid_key_prefix")));
        }
        self.key_prefix = Some(Arc::from(key_prefix));
        Ok(self)
    }

    /// Select path-style or virtual-host-style addressing.
    #[must_use]
    pub const fn with_addressing(mut self, addressing: Addressing) -> Self {
        self.addressing = addressing;
        self
    }

    /// Set static credentials used for `SigV4` signing.
    ///
    /// # Errors
    ///
    /// Rejects an empty or NUL-bearing access key id.
    pub fn with_credentials(
        mut self,
        access_key_id: impl AsRef<str>,
        secret_access_key: SecretString,
    ) -> Result<Self, ArtifactError> {
        let access_key_id = access_key_id.as_ref();
        if access_key_id.is_empty() || access_key_id.as_bytes().contains(&0) {
            return Err(map_object_error(invalid("invalid_access_key_id")));
        }
        self.access_key_id = Some(Arc::from(access_key_id));
        self.secret_access_key = Some(secret_access_key);
        Ok(self)
    }

    /// Set the connection-establishment and inter-chunk idle-read timeout.
    ///
    /// This bounds two things: how long the client waits to establish the
    /// TCP/TLS connection, and how long it waits between successive reads
    /// while streaming a response body (the timer resets on every read).
    /// It does **not** bound the total duration of a request — a large
    /// `get_to_file`/put against a store with a large `max_object_bytes`
    /// ceiling can legitimately run far longer than this value as long as
    /// bytes keep arriving. Hosts that need a hard total deadline should
    /// wrap calls into this store in their own timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the maximum object size this store will accept on put.
    #[must_use]
    pub const fn with_max_object_bytes(mut self, max_object_bytes: u64) -> Self {
        self.max_object_bytes = max_object_bytes;
        self
    }

    /// Set the maximum expiry a caller may request for a presigned URL.
    #[must_use]
    pub const fn with_presign_expiry_max(mut self, presign_expiry_max: Duration) -> Self {
        self.presign_expiry_max = presign_expiry_max;
        self
    }

    /// Configured endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Configured bucket.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Configured region.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Configured physical key prefix, if any.
    #[must_use]
    pub fn key_prefix(&self) -> Option<&str> {
        self.key_prefix.as_deref()
    }

    /// Configured addressing style.
    #[must_use]
    pub const fn addressing(&self) -> Addressing {
        self.addressing
    }

    /// Configured access key id, if credentials were set.
    #[must_use]
    pub fn access_key_id(&self) -> Option<&str> {
        self.access_key_id.as_deref()
    }

    /// Configured secret access key, if credentials were set.
    #[must_use]
    pub fn secret_access_key(&self) -> Option<&SecretString> {
        self.secret_access_key.as_ref()
    }

    /// Configured connection-establishment and inter-chunk idle-read
    /// timeout (not a total-request deadline; see [`Self::with_timeout`]).
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Configured maximum object size.
    #[must_use]
    pub const fn max_object_bytes(&self) -> u64 {
        self.max_object_bytes
    }

    /// Configured maximum presigned URL expiry.
    #[must_use]
    pub const fn presign_expiry_max(&self) -> Duration {
        self.presign_expiry_max
    }
}

fn invalid(message: &'static str) -> ObjectError {
    ObjectError::InvalidMetadata {
        message: Arc::from(message),
    }
}

fn validate_endpoint(value: &str) -> Result<(), ObjectError> {
    let url = Url::parse(value).map_err(|_| invalid("invalid_endpoint"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("invalid_endpoint"));
    }
    Ok(())
}

fn validate_bucket(value: &str) -> Result<(), ObjectError> {
    if value.is_empty()
        || value.as_bytes().contains(&0)
        || value.len() < 3
        || value.len() > 63
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
    {
        return Err(invalid("invalid_bucket"));
    }
    Ok(())
}

fn validate_region(value: &str) -> Result<(), ObjectError> {
    if value.is_empty() || value.as_bytes().contains(&0) {
        return Err(invalid("invalid_region"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // `artifact_invalid_metadata` has no reachable constant: it is one of the
    // runtime codes declared inside a private module. Use the literal until the
    // codes are surfaced as values.
    const ARTIFACT_INVALID_METADATA: &str = "artifact_invalid_metadata";

    use super::{S3ObjectStoreConfig, SecretString};

    #[test]
    fn config_rejects_query_fragment_and_credentials_in_endpoint() {
        for endpoint in [
            "http://127.0.0.1:9000/?x=1",
            "http://127.0.0.1:9000/#frag",
            "http://user:pass@127.0.0.1:9000",
        ] {
            let error = S3ObjectStoreConfig::try_new(endpoint, "bucket", "garage")
                .expect_err("must reject");
            assert_eq!(error.code(), ARTIFACT_INVALID_METADATA);
        }
    }

    #[test]
    fn config_rejects_invalid_bucket_and_region() {
        for bucket in ["", "ab", &"x".repeat(64), "Bad_Bucket", "bad\0bucket"] {
            let error = S3ObjectStoreConfig::try_new("http://127.0.0.1:9000", bucket, "garage")
                .expect_err("must reject");
            assert_eq!(error.code(), ARTIFACT_INVALID_METADATA);
        }
        for region in ["", "bad\0region"] {
            let error = S3ObjectStoreConfig::try_new("http://127.0.0.1:9000", "bucket", region)
                .expect_err("must reject");
            assert_eq!(error.code(), ARTIFACT_INVALID_METADATA);
        }
    }

    #[test]
    fn debug_and_errors_never_leak_the_secret_key() {
        let config = S3ObjectStoreConfig::try_new("http://127.0.0.1:9000", "bucket", "garage")
            .expect("config")
            .with_credentials(
                "AKIAEXAMPLE",
                SecretString::try_new("SUPERSECRET").expect("secret"),
            )
            .expect("credentials");
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("SUPERSECRET"));
        assert!(rendered.contains("[REDACTED]"));
    }
}
