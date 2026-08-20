//! [`S3ObjectStore`]: the `ObjectStore` implementation over `SigV4`-signed
//! HTTP calls to an S3-compatible bucket.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::Digest;
use finstack_ai_runtime::{
    Bytes, ObjectEntry, ObjectError, ObjectKey, ObjectMetadata, ObjectPage, ObjectRef, ObjectScope,
    ObjectStore, ObjectStoreLimits, PageToken, PortFuture, PresignedUrl, PutPayload,
    physical_object_key, validate_object_metadata,
};
use futures_util::StreamExt;
use reqwest::{Method, Response, StatusCode, Url};
use tokio::io::AsyncReadExt;

use crate::config::S3ObjectStoreConfig;
use crate::request::{
    BlobDigestHasher, ListTarget, RequestTarget, StreamingSha256, extract_tag_values, list_url,
    map_status_error, map_transport_error, object_url, payload_sha256_hex,
};
use crate::sigv4::{SigningParams, UtcStamp, presign_url, sign_headers};

/// 64 KiB read chunk used for both hash-pass and streaming-send file reads.
const FILE_CHUNK_BYTES: usize = 64 * 1024;

/// Header carrying the domain-separated content digest.
const HEADER_DIGEST: &str = "x-amz-meta-fsai-digest";
/// Header carrying the caller's scope digest.
const HEADER_SCOPE: &str = "x-amz-meta-fsai-scope";
/// Header carrying the optional display name, `SigV4`-URI-encoded.
const HEADER_NAME: &str = "x-amz-meta-fsai-name";

/// `ObjectStore` backend over an S3-compatible HTTP API, signed with a
/// hand-rolled `SigV4` client (`crate::sigv4`) and no AWS SDK dependency.
pub struct S3ObjectStore {
    client: reqwest::Client,
    config: S3ObjectStoreConfig,
}

impl S3ObjectStore {
    /// Build a store from a validated transport configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ObjectError::InvalidMetadata`] when the underlying HTTP
    /// client cannot be constructed (e.g. an unsupported TLS configuration).
    pub fn try_new(config: S3ObjectStoreConfig) -> Result<Self, ObjectError> {
        // A 3xx would otherwise cause reqwest to transparently re-send the
        // request (including any PUT body and the SigV4-signed headers,
        // which are only valid for the original host/path) to whatever
        // target a compromised or misconfigured endpoint names in
        // `Location`. Disabling redirects makes that a hard failure
        // (mapped to `Unavailable { message: "http_3xx" }`) instead.
        let client = reqwest::Client::builder()
            .connect_timeout(config.timeout())
            .read_timeout(config.timeout())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_error| ObjectError::InvalidMetadata {
                message: Arc::from("http_client_build_failed"),
            })?;
        Ok(Self { client, config })
    }
}

impl ObjectStore for S3ObjectStore {
    fn put(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: PutPayload,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { put_impl(&client, &config, scope, key, content, metadata).await })
    }

    fn get(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<Bytes, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { get_impl(&client, &config, scope, key).await })
    }

    fn get_to_file(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        dest: PathBuf,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { get_to_file_impl(&client, &config, scope, key, dest).await })
    }

    fn head(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { head_impl(&client, &config, scope, key).await })
    }

    fn delete(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<(), ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { delete_impl(&client, &config, scope, key).await })
    }

    fn list(
        &self,
        scope: ObjectScope,
        prefix: Option<ObjectKey>,
        page: PageToken,
    ) -> PortFuture<Result<ObjectPage, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { list_impl(&client, &config, scope, prefix, page).await })
    }

    fn presign_get(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expiry: Duration,
    ) -> PortFuture<Result<PresignedUrl, ObjectError>> {
        let config = self.config.clone();
        Box::pin(async move { presign_get_impl(&config, &scope, &key, expiry) })
    }

    fn limits(&self) -> ObjectStoreLimits {
        ObjectStoreLimits {
            max_object_bytes: self.config.max_object_bytes(),
        }
    }
}

fn missing_credentials() -> ObjectError {
    ObjectError::InvalidMetadata {
        message: Arc::from("missing_credentials"),
    }
}

fn signing_params(
    config: &S3ObjectStoreConfig,
    timestamp: UtcStamp,
) -> Result<SigningParams<'_>, ObjectError> {
    let access_key_id = config.access_key_id().ok_or_else(missing_credentials)?;
    let secret_access_key = config.secret_access_key().ok_or_else(missing_credentials)?;
    Ok(SigningParams {
        access_key_id,
        secret_key: secret_access_key.expose(),
        region: config.region(),
        service: "s3",
        timestamp,
    })
}

fn io_error(error: &std::io::Error) -> ObjectError {
    ObjectError::Io {
        message: Arc::from(error.to_string()),
    }
}

fn send_signed(
    client: &reqwest::Client,
    method: Method,
    url: Url,
    headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    let mut builder = client.request(method, url);
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
}

fn header_value(response: &Response, name: &str) -> Result<String, ObjectError> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| ObjectError::Integrity {
            message: Arc::from("missing_metadata_header"),
        })
}

fn parse_scope_digest(header: &str, expected: Digest) -> Result<Digest, ObjectError> {
    let actual = Digest::from_hex(header).map_err(|_error| ObjectError::Integrity {
        message: Arc::from("malformed_scope_header"),
    })?;
    if actual != expected {
        return Err(ObjectError::ScopeMismatch { expected, actual });
    }
    Ok(actual)
}

/// Read `content-length`, if present and well-formed, and reject early when
/// it already exceeds `max_bytes` — before any body bytes are read.
///
/// A missing or malformed header is not itself an error here: the
/// incremental byte counter on the streaming read path is the real
/// enforcement; this is a fast path that avoids opening a body stream at
/// all for a response that already announces itself as too large.
fn reject_if_content_length_exceeds(
    response: &Response,
    max_bytes: u64,
) -> Result<(), ObjectError> {
    let Some(header) = response.headers().get("content-length") else {
        return Ok(());
    };
    let Some(len) = header
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return Ok(());
    };
    if len > max_bytes {
        return Err(ObjectError::TooLarge {
            len,
            max: max_bytes,
        });
    }
    Ok(())
}

/// Reads a local file in two full passes: once to compute the real SHA-256
/// payload hash, the domain-separated content digest, and total length
/// (enforcing `max_bytes` as it goes so nothing over the ceiling is ever
/// sent on the wire), and once — separately, in `stream_file_body` — to
/// stream the body. The file is never materialized in memory.
async fn hash_file(path: &Path, max_bytes: u64) -> Result<(String, Digest, u64), ObjectError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| io_error(&error))?;
    let mut raw_hasher = StreamingSha256::new();
    let mut blob_hasher = BlobDigestHasher::new();
    let mut total: u64 = 0;
    let mut buffer = vec![0_u8; FILE_CHUNK_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| io_error(&error))?;
        if read == 0 {
            break;
        }
        let Some(chunk) = buffer.get(..read) else {
            return Err(io_error(&std::io::Error::other("short read buffer")));
        };
        raw_hasher.update(chunk);
        blob_hasher.update(chunk);
        total = total.saturating_add(read as u64);
        if total > max_bytes {
            return Err(ObjectError::TooLarge {
                len: total,
                max: max_bytes,
            });
        }
    }
    let payload_hash = raw_hasher.finish();
    let (content_digest, _len) = blob_hasher.finish()?;
    Ok((payload_hash, content_digest, total))
}

fn file_body_stream(file: tokio::fs::File) -> reqwest::Body {
    let stream = futures_util::stream::unfold(file, |mut file| async move {
        let mut buffer = vec![0_u8; FILE_CHUNK_BYTES];
        match file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok::<Bytes, std::io::Error>(Bytes::from(buffer)), file))
            }
            Err(error) => Some((Err(error), file)),
        }
    });
    reqwest::Body::wrap_stream(stream)
}

struct PutSource {
    payload_hash: String,
    content_digest: Digest,
    length: u64,
    body: reqwest::Body,
}

async fn put_source(content: PutPayload, max_bytes: u64) -> Result<PutSource, ObjectError> {
    match content {
        PutPayload::Bytes(bytes) => {
            let length = u64::try_from(bytes.len()).map_err(|_error| ObjectError::Io {
                message: Arc::from("length_overflow"),
            })?;
            if length > max_bytes {
                return Err(ObjectError::TooLarge {
                    len: length,
                    max: max_bytes,
                });
            }
            let payload_hash = payload_sha256_hex(&bytes);
            let content_digest = Digest::blob_content(&bytes);
            Ok(PutSource {
                payload_hash,
                content_digest,
                length,
                body: reqwest::Body::from(bytes),
            })
        }
        PutPayload::File(path) => {
            let (payload_hash, content_digest, length) = hash_file(&path, max_bytes).await?;
            let file = tokio::fs::File::open(&path)
                .await
                .map_err(|error| io_error(&error))?;
            Ok(PutSource {
                payload_hash,
                content_digest,
                length,
                body: file_body_stream(file),
            })
        }
    }
}

async fn put_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    key: ObjectKey,
    content: PutPayload,
    metadata: ObjectMetadata,
) -> Result<ObjectRef, ObjectError> {
    validate_object_metadata(&metadata)?;
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, &key);

    let source = put_source(content, config.max_object_bytes()).await?;

    let target = object_url(config, &physical)?;
    let params = signing_params(config, UtcStamp::now())?;

    let mut extra_headers = vec![
        ("content-type".to_owned(), metadata.media_type.to_string()),
        ("content-length".to_owned(), source.length.to_string()),
        (HEADER_DIGEST.to_owned(), source.content_digest.to_hex()),
        (HEADER_SCOPE.to_owned(), scope_digest.to_hex()),
    ];
    if let Some(name) = &metadata.name {
        extra_headers.push((HEADER_NAME.to_owned(), crate::sigv4::uri_encode(name, true)));
    }

    let signed = sign_headers(
        &params,
        "PUT",
        &target.path,
        "",
        &target.host,
        &source.payload_hash,
        &extra_headers,
    );

    let response = send_signed(client, Method::PUT, target.url, &signed)
        .body(source.body)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if !status.is_success() {
        return Err(map_status_error(status));
    }

    Ok(ObjectRef {
        key,
        scope_digest,
        content_digest: source.content_digest,
        length: source.length,
        media_type: Arc::clone(&metadata.media_type),
    })
}

async fn get_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    key: ObjectKey,
) -> Result<Bytes, ObjectError> {
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, &key);
    let target = object_url(config, &physical)?;
    let params = signing_params(config, UtcStamp::now())?;
    let empty_hash = payload_sha256_hex(b"");
    let signed = sign_headers(
        &params,
        "GET",
        &target.path,
        "",
        &target.host,
        &empty_hash,
        &[],
    );

    let response = send_signed(client, Method::GET, target.url, &signed)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if !status.is_success() {
        return Err(map_status_error(status));
    }

    let scope_header = header_value(&response, HEADER_SCOPE)?;
    let digest_header = header_value(&response, HEADER_DIGEST)?;
    parse_scope_digest(&scope_header, scope_digest)?;

    let max_bytes = config.max_object_bytes();
    reject_if_content_length_exceeds(&response, max_bytes)?;

    // Stream rather than `response.bytes()` so a hostile or misconfigured
    // endpoint that lies about (or omits) `content-length` cannot force an
    // unbounded buffer allocation before the size ceiling is checked.
    let mut buffer: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_error| map_transport_error())?;
        total = total.saturating_add(chunk.len() as u64);
        if total > max_bytes {
            return Err(ObjectError::TooLarge {
                len: total,
                max: max_bytes,
            });
        }
        buffer.extend_from_slice(&chunk);
    }

    let bytes = Bytes::from(buffer);
    let computed = Digest::blob_content(&bytes);
    if computed.to_hex() != digest_header {
        return Err(ObjectError::Integrity {
            message: Arc::from("digest_mismatch"),
        });
    }
    Ok(bytes)
}

async fn get_to_file_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    key: ObjectKey,
    dest: PathBuf,
) -> Result<ObjectRef, ObjectError> {
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, &key);
    let target = object_url(config, &physical)?;
    let params = signing_params(config, UtcStamp::now())?;
    let empty_hash = payload_sha256_hex(b"");
    let signed = sign_headers(
        &params,
        "GET",
        &target.path,
        "",
        &target.host,
        &empty_hash,
        &[],
    );

    let response = send_signed(client, Method::GET, target.url, &signed)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if !status.is_success() {
        return Err(map_status_error(status));
    }

    let scope_header = header_value(&response, HEADER_SCOPE)?;
    let digest_header = header_value(&response, HEADER_DIGEST)?;
    let media_type = header_value(&response, "content-type")
        .unwrap_or_else(|_error| "application/octet-stream".to_owned());
    parse_scope_digest(&scope_header, scope_digest)?;

    let max_bytes = config.max_object_bytes();
    reject_if_content_length_exceeds(&response, max_bytes)?;

    let parent = dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|error| io_error(&error))?;

    let mut hasher = BlobDigestHasher::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_error| map_transport_error())?;
        // Enforce the ceiling against the incremental counter, not just the
        // (possibly absent or falsified) `content-length` header, so a
        // hostile endpoint cannot exhaust disk before verification.
        let projected = hasher.bytes_written().saturating_add(chunk.len() as u64);
        if projected > max_bytes {
            return Err(ObjectError::TooLarge {
                len: projected,
                max: max_bytes,
            });
        }
        hasher.update(&chunk);
        std::io::Write::write_all(&mut tmp, &chunk).map_err(|error| io_error(&error))?;
    }

    let (content_digest, length) = hasher.finish()?;
    if content_digest.to_hex() != digest_header {
        return Err(ObjectError::Integrity {
            message: Arc::from("digest_mismatch"),
        });
    }

    tmp.persist(&dest).map_err(|error| ObjectError::Io {
        message: Arc::from(error.to_string()),
    })?;

    Ok(ObjectRef {
        key,
        scope_digest,
        content_digest,
        length,
        media_type: Arc::from(media_type),
    })
}

async fn head_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    key: ObjectKey,
) -> Result<ObjectRef, ObjectError> {
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, &key);
    let target = object_url(config, &physical)?;
    let params = signing_params(config, UtcStamp::now())?;
    let empty_hash = payload_sha256_hex(b"");
    let signed = sign_headers(
        &params,
        "HEAD",
        &target.path,
        "",
        &target.host,
        &empty_hash,
        &[],
    );

    let response = send_signed(client, Method::HEAD, target.url, &signed)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if !status.is_success() {
        return Err(map_status_error(status));
    }

    let scope_header = header_value(&response, HEADER_SCOPE)?;
    let digest_header = header_value(&response, HEADER_DIGEST)?;
    let content_length = header_value(&response, "content-length")?;
    let media_type = header_value(&response, "content-type")
        .unwrap_or_else(|_error| "application/octet-stream".to_owned());

    parse_scope_digest(&scope_header, scope_digest)?;
    let content_digest =
        Digest::from_hex(&digest_header).map_err(|_error| ObjectError::Integrity {
            message: Arc::from("malformed_digest_header"),
        })?;
    let length: u64 = content_length
        .parse()
        .map_err(|_error| ObjectError::Integrity {
            message: Arc::from("malformed_content_length"),
        })?;

    Ok(ObjectRef {
        key,
        scope_digest,
        content_digest,
        length,
        media_type: Arc::from(media_type),
    })
}

async fn delete_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    key: ObjectKey,
) -> Result<(), ObjectError> {
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, &key);
    let target = object_url(config, &physical)?;
    let params = signing_params(config, UtcStamp::now())?;
    let empty_hash = payload_sha256_hex(b"");
    let signed = sign_headers(
        &params,
        "DELETE",
        &target.path,
        "",
        &target.host,
        &empty_hash,
        &[],
    );

    let response = send_signed(client, Method::DELETE, target.url, &signed)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if status.is_success() || status == StatusCode::NOT_FOUND {
        return Ok(());
    }
    Err(map_status_error(status))
}

fn scope_prefix(config: &S3ObjectStoreConfig, scope_digest: &Digest) -> String {
    let hex = scope_digest.to_hex();
    let hex16 = hex.get(..16).unwrap_or(&hex);
    match config.key_prefix() {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{hex16}/"),
        _ => format!("{hex16}/"),
    }
}

async fn list_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    prefix: Option<ObjectKey>,
    page: PageToken,
) -> Result<ObjectPage, ObjectError> {
    let scope_digest = scope.digest()?;
    let base_prefix = scope_prefix(config, &scope_digest);
    let mut full_prefix = base_prefix.clone();
    if let Some(user_prefix) = &prefix {
        full_prefix.push_str(user_prefix.as_str());
    }

    let ListTarget {
        url,
        path,
        host,
        canonical_query,
    } = list_url(config, &full_prefix, page.value())?;
    let params = signing_params(config, UtcStamp::now())?;
    let empty_hash = payload_sha256_hex(b"");
    let signed = sign_headers(
        &params,
        "GET",
        &path,
        &canonical_query,
        &host,
        &empty_hash,
        &[],
    );

    let response = send_signed(client, Method::GET, url, &signed)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if !status.is_success() {
        return Err(map_status_error(status));
    }
    let body = response
        .text()
        .await
        .map_err(|_error| map_transport_error())?;

    let keys = extract_tag_values(&body, "Key");
    let sizes = extract_tag_values(&body, "Size");
    let truncated = extract_tag_values(&body, "IsTruncated")
        .first()
        .is_some_and(|value| value == "true");
    let next_token = extract_tag_values(&body, "NextContinuationToken")
        .into_iter()
        .next();

    let mut entries = Vec::with_capacity(keys.len());
    for (raw_key, raw_size) in keys.iter().zip(sizes.iter()) {
        // Fail closed: a key outside the caller's scope prefix means the
        // endpoint ignored `prefix=` (compromised or buggy). Surfacing it
        // anyway would leak another tenant's physical key (including their
        // scope digest) as a valid in-scope entry.
        let Some(logical) = raw_key.strip_prefix(&base_prefix) else {
            return Err(ObjectError::Integrity {
                message: Arc::from("list_key_outside_scope"),
            });
        };
        let object_key = ObjectKey::try_new(logical)?;
        let length: u64 = raw_size.parse().map_err(|_error| ObjectError::Io {
            message: Arc::from("invalid_list_size"),
        })?;
        entries.push(ObjectEntry {
            key: object_key,
            length,
        });
    }

    let next = if truncated {
        next_token.map(PageToken::opaque)
    } else {
        None
    };
    Ok(ObjectPage { entries, next })
}

fn presign_get_impl(
    config: &S3ObjectStoreConfig,
    scope: &ObjectScope,
    key: &ObjectKey,
    expiry: Duration,
) -> Result<PresignedUrl, ObjectError> {
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, key);
    let target: RequestTarget = object_url(config, &physical)?;
    let clamped = expiry.min(config.presign_expiry_max());
    let params = signing_params(config, UtcStamp::now())?;
    let url = presign_url(
        &params,
        "GET",
        &target.path,
        &target.host,
        &target.scheme,
        clamped.as_secs(),
    );
    Ok(PresignedUrl {
        url: Arc::from(url),
        expires_in_secs: clamped.as_secs(),
    })
}
