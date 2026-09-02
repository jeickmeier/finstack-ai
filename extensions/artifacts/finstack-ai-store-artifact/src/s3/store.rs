//! [`S3ObjectStore`]: the `ObjectDriver` implementation over `SigV4`-signed
//! HTTP calls to an S3-compatible bucket.

use std::sync::Arc;

use crate::driver::{
    ObjectDriver, ObjectEntry, ObjectError, ObjectKey, ObjectMetadata, ObjectPage, ObjectRef,
    ObjectScope, ObjectStoreLimits, PageToken, physical_object_key, validate_object_metadata,
};
use finstack_ai_kernel::Digest;
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::ports::PortFuture;
use futures_util::StreamExt;
use reqwest::{Method, Response, StatusCode, Url};
use serde::Deserialize;

use crate::s3::config::S3ObjectStoreConfig;
use crate::s3::request::{
    ListTarget, list_url, map_status_error, map_transport_error, object_url, payload_sha256_hex,
};
use crate::s3::sigv4::{SigningParams, UtcStamp, sign_headers};

const LIST_RESPONSE_MAX_BYTES: u64 = 8 * 1024 * 1024;
const LIST_MAX_ENTRIES: usize = 1000;

/// Header carrying the domain-separated content digest.
const HEADER_DIGEST: &str = "x-amz-meta-fsai-digest";
/// Header carrying the caller's scope digest.
const HEADER_SCOPE: &str = "x-amz-meta-fsai-scope";
/// Header carrying the optional display name, `SigV4`-URI-encoded.
const HEADER_NAME: &str = "x-amz-meta-fsai-name";

/// `ObjectDriver` backend over an S3-compatible HTTP API, signed with a
/// hand-rolled `SigV4` client (`crate::s3::sigv4`) and no AWS SDK dependency.
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

impl ObjectDriver for S3ObjectStore {
    fn put(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: Bytes,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(
            async move { put_impl(&client, &config, scope, key, content, metadata, None).await },
        )
    }

    fn get(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<Bytes, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { get_impl(&client, &config, scope, key).await })
    }

    fn delete(&self, scope: ObjectScope, key: ObjectKey) -> PortFuture<Result<(), ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { delete_impl(&client, &config, scope, key, None).await })
    }

    fn put_if_absent(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        content: Bytes,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move {
            put_impl(
                &client,
                &config,
                scope,
                key,
                content,
                metadata,
                Some(("if-none-match", "*")),
            )
            .await
        })
    }

    fn replace_if_digest(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expected: Digest,
        content: Bytes,
        metadata: ObjectMetadata,
    ) -> PortFuture<Result<ObjectRef, ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move {
            let (current, etag) = current_digest_and_etag(&client, &config, &scope, &key).await?;
            if current != expected {
                return Err(ObjectError::Conflict);
            }
            put_impl(
                &client,
                &config,
                scope,
                key,
                content,
                metadata,
                Some(("if-match", etag.as_str())),
            )
            .await
        })
    }

    fn delete_if_digest(
        &self,
        scope: ObjectScope,
        key: ObjectKey,
        expected: Digest,
    ) -> PortFuture<Result<(), ObjectError>> {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move {
            let (current, etag) = current_digest_and_etag(&client, &config, &scope, &key).await?;
            if current != expected {
                return Err(ObjectError::Conflict);
            }
            delete_impl(&client, &config, scope, key, Some(etag.as_str())).await
        })
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

/// `PUT` one object, optionally under an `if-match` / `if-none-match`
/// precondition header (a `412`/`409` answer is [`ObjectError::Conflict`]).
#[allow(
    clippy::too_many_arguments,
    reason = "one argument per request part; the driver methods are thin wrappers"
)]
async fn put_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    key: ObjectKey,
    content: Bytes,
    metadata: ObjectMetadata,
    condition: Option<(&str, &str)>,
) -> Result<ObjectRef, ObjectError> {
    validate_object_metadata(&metadata)?;
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, &key);

    let max_bytes = config.max_object_bytes();
    let length = u64::try_from(content.len()).map_err(|_error| ObjectError::Io {
        message: Arc::from("length_overflow"),
    })?;
    if length > max_bytes {
        return Err(ObjectError::TooLarge {
            len: length,
            max: max_bytes,
        });
    }
    let payload_hash = payload_sha256_hex(&content);
    let content_digest = Digest::blob_content(&content);

    let target = object_url(config, &physical)?;
    let params = signing_params(config, UtcStamp::now())?;

    let mut extra_headers = vec![
        ("content-type".to_owned(), metadata.media_type.to_string()),
        ("content-length".to_owned(), length.to_string()),
        (HEADER_DIGEST.to_owned(), content_digest.to_hex()),
        (HEADER_SCOPE.to_owned(), scope_digest.to_hex()),
    ];
    if let Some(name) = &metadata.name {
        extra_headers.push((
            HEADER_NAME.to_owned(),
            crate::s3::sigv4::uri_encode(name, true),
        ));
    }
    if let Some((name, value)) = condition {
        extra_headers.push((name.to_owned(), value.to_owned()));
    }

    let signed = sign_headers(
        &params,
        "PUT",
        &target.path,
        "",
        &target.host,
        &payload_hash,
        &extra_headers,
    );

    let response = send_signed(client, Method::PUT, target.url, &signed)
        .body(content)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if status == StatusCode::PRECONDITION_FAILED || status == StatusCode::CONFLICT {
        return Err(ObjectError::Conflict);
    }
    if !status.is_success() {
        return Err(map_status_error(status));
    }

    Ok(ObjectRef {
        key,
        scope_digest,
        content_digest,
        length,
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

async fn current_digest_and_etag(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: &ObjectScope,
    key: &ObjectKey,
) -> Result<(Digest, String), ObjectError> {
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, key);
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
    if !response.status().is_success() {
        return Err(map_status_error(response.status()));
    }
    parse_scope_digest(&header_value(&response, HEADER_SCOPE)?, scope_digest)?;
    let digest = Digest::from_hex(&header_value(&response, HEADER_DIGEST)?).map_err(|_error| {
        ObjectError::Integrity {
            message: Arc::from("malformed_digest_header"),
        }
    })?;
    let etag = header_value(&response, "etag")?;
    Ok((digest, etag))
}

/// `DELETE` one object, optionally only when its `etag` still matches. An
/// unconditional delete of a missing object is not an error.
async fn delete_impl(
    client: &reqwest::Client,
    config: &S3ObjectStoreConfig,
    scope: ObjectScope,
    key: ObjectKey,
    etag: Option<&str>,
) -> Result<(), ObjectError> {
    let scope_digest = scope.digest()?;
    let physical = physical_object_key(config.key_prefix(), &scope_digest, &key);
    let target = object_url(config, &physical)?;
    let params = signing_params(config, UtcStamp::now())?;
    let empty_hash = payload_sha256_hex(b"");
    let extra_headers = etag
        .map(|value| vec![("if-match".to_owned(), value.to_owned())])
        .unwrap_or_default();
    let signed = sign_headers(
        &params,
        "DELETE",
        &target.path,
        "",
        &target.host,
        &empty_hash,
        &extra_headers,
    );

    let response = send_signed(client, Method::DELETE, target.url, &signed)
        .send()
        .await
        .map_err(|_error| map_transport_error())?;
    let status = response.status();
    if status == StatusCode::PRECONDITION_FAILED || status == StatusCode::CONFLICT {
        return Err(ObjectError::Conflict);
    }
    if status.is_success() || (status == StatusCode::NOT_FOUND && etag.is_none()) {
        return Ok(());
    }
    Err(map_status_error(status))
}

fn scope_prefix(config: &S3ObjectStoreConfig, scope_digest: &Digest) -> String {
    let hex = scope_digest.to_hex();
    match config.key_prefix() {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}/{hex}/"),
        _ => format!("{hex}/"),
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
    reject_if_content_length_exceeds(&response, LIST_RESPONSE_MAX_BYTES)?;
    let mut body = Vec::new();
    let mut total = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_error| map_transport_error())?;
        total = total.saturating_add(chunk.len() as u64);
        if total > LIST_RESPONSE_MAX_BYTES {
            return Err(ObjectError::TooLarge {
                len: total,
                max: LIST_RESPONSE_MAX_BYTES,
            });
        }
        body.extend_from_slice(&chunk);
    }
    let parsed: ListBucketResult =
        quick_xml::de::from_reader(body.as_slice()).map_err(|_error| ObjectError::Integrity {
            message: Arc::from("malformed_list_xml"),
        })?;
    if parsed.contents.len() > LIST_MAX_ENTRIES
        || (parsed.is_truncated
            && parsed
                .next_continuation_token
                .as_deref()
                .is_none_or(str::is_empty))
        || (!parsed.is_truncated && parsed.next_continuation_token.is_some())
    {
        return Err(ObjectError::Integrity {
            message: Arc::from("invalid_list_pagination"),
        });
    }

    let mut entries = Vec::with_capacity(parsed.contents.len());
    for content in parsed.contents {
        // Fail closed: a key outside the caller's scope prefix means the
        // endpoint ignored `prefix=` (compromised or buggy). Surfacing it
        // anyway would leak another tenant's physical key (including their
        // scope digest) as a valid in-scope entry.
        let Some(logical) = content.key.strip_prefix(&base_prefix) else {
            return Err(ObjectError::Integrity {
                message: Arc::from("list_key_outside_scope"),
            });
        };
        let object_key = ObjectKey::try_new(logical)?;
        entries.push(ObjectEntry {
            key: object_key,
            length: content.size,
        });
    }

    let next = if parsed.is_truncated {
        parsed.next_continuation_token.map(PageToken::opaque)
    } else {
        None
    };
    Ok(ObjectPage { entries, next })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListBucketResult {
    #[serde(rename = "Contents", default)]
    contents: Vec<ListContent>,
    #[serde(default)]
    is_truncated: bool,
    #[serde(default)]
    next_continuation_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListContent {
    key: String,
    size: u64,
}
