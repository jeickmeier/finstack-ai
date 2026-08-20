//! Loopback-only fixture tests for [`S3ObjectStore`].
//!
//! No live network calls: every test either talks to a `TcpListener` bound
//! on `127.0.0.1:0`, or exercises pure computation (`presign_get`) against
//! no server at all.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{Digest, Metadata, Sensitivity};
use finstack_ai_runtime::{
    Bytes, OBJECT_NOT_FOUND, OBJECT_SCOPE_MISMATCH, OBJECT_TOO_LARGE, ObjectKey, ObjectMetadata,
    ObjectScope, ObjectStore, PageToken, PutPayload, SecretString,
};
use finstack_ai_store_object_s3::request::payload_sha256_hex;
use finstack_ai_store_object_s3::{Addressing, S3ObjectStore, S3ObjectStoreConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ---------------------------------------------------------------------------
// Shared fixture plumbing
// ---------------------------------------------------------------------------

fn scope() -> ObjectScope {
    ObjectScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: None,
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

fn metadata(media_type: &str) -> ObjectMetadata {
    ObjectMetadata {
        media_type: Arc::from(media_type),
        name: None,
        attributes: Metadata::empty(),
    }
}

fn hex16() -> String {
    scope().digest().expect("scope digest").to_hex().chars().take(16).collect()
}

fn store(base_url: &str, addressing: Addressing, max_object_bytes: Option<u64>) -> S3ObjectStore {
    let mut config = S3ObjectStoreConfig::try_new(base_url, "bucket", "us-east-1")
        .expect("config")
        .with_addressing(addressing)
        .with_key_prefix("finstack")
        .with_credentials("AKIAEXAMPLE", SecretString::try_new("supersecretvalue").expect("secret"))
        .expect("credentials");
    if let Some(max) = max_object_bytes {
        config = config.with_max_object_bytes(max);
    }
    S3ObjectStore::try_new(config).expect("store")
}

/// One canned HTTP/1.1 response the fixture server writes back for one
/// accepted connection.
struct CannedResponse {
    status_line: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl CannedResponse {
    fn ok(headers: Vec<(&'static str, String)>, body: Vec<u8>) -> Self {
        Self { status_line: "200 OK", headers, body }
    }

    fn status(status_line: &'static str) -> Self {
        Self { status_line, headers: Vec::new(), body: Vec::new() }
    }
}

/// Serve `responses` one per accepted connection (in order), each closed
/// after the response is written so the client must reconnect for the next
/// one. Returns the base URL and a handle yielding every raw captured
/// request (request line + headers + body, as UTF-8).
async fn serve(responses: Vec<CannedResponse>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let task = tokio::spawn(async move {
        let mut captured = Vec::new();
        for canned in responses {
            let (mut socket, _) = listener.accept().await.expect("accept");
            captured.push(read_request(&mut socket).await);
            write_response(&mut socket, &canned).await;
        }
        captured
    });
    (format!("http://{address}"), task)
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 8_192];
    let header_end = loop {
        let count = socket.read(&mut buffer).await.expect("read request");
        assert!(count > 0, "request closed before headers");
        request.extend_from_slice(&buffer[..count]);
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let count = socket.read(&mut buffer).await.expect("read body");
        assert!(count > 0, "request closed before body");
        request.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8_lossy(&request).into_owned()
}

async fn write_response(socket: &mut TcpStream, canned: &CannedResponse) {
    use std::fmt::Write as _;

    let mut response = format!("HTTP/1.1 {}\r\n", canned.status_line);
    for (name, value) in &canned.headers {
        let _ = write!(response, "{name}: {value}\r\n");
    }
    let _ = write!(response, "Content-Length: {}\r\nConnection: close\r\n\r\n", canned.body.len());
    socket.write_all(response.as_bytes()).await.expect("write headers");
    socket.write_all(&canned.body).await.expect("write body");
}

// ---------------------------------------------------------------------------
// Step 1: loopback fixture tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn put_signs_path_style_and_sends_metadata_headers() {
    let (base_url, server) = serve(vec![CannedResponse::ok(Vec::new(), Vec::new())]).await;
    let store = store(&base_url, Addressing::Path, None);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");
    let content = Bytes::from_static(b"hello world");

    let object_ref = store
        .put(scope(), key, PutPayload::Bytes(content.clone()), metadata("application/pdf"))
        .await
        .expect("put must succeed");
    assert_eq!(object_ref.length, content.len() as u64);

    let captured = server.await.expect("server task");
    let request = &captured[0];
    assert!(
        request.starts_with(&format!("PUT /bucket/finstack/{}/docs/a.pdf HTTP/1.1", hex16())),
        "captured request: {request}"
    );
    assert!(request.contains("authorization: AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/"));
    assert!(request.contains(&format!("x-amz-content-sha256: {}", payload_sha256_hex(b"hello world"))));
    assert!(request.contains("x-amz-meta-fsai-digest:"));
    assert!(request.contains("x-amz-meta-fsai-scope:"));
}

#[tokio::test(flavor = "multi_thread")]
async fn put_virtual_host_addressing_targets_bucket_host() {
    let (base_url, server) = serve(vec![CannedResponse::ok(Vec::new(), Vec::new())]).await;
    // `bucket.127.0.0.1` triggers the WHATWG URL parser's "ends in a
    // number" IPv4 heuristic (the last label, `1`, looks numeric, so it
    // tries and fails to parse the whole host as an IPv4 address).
    // `localhost` sidesteps that without any live DNS lookup.
    let base_url = base_url.replacen("127.0.0.1", "localhost", 1);
    let store = store(&base_url, Addressing::VirtualHost, None);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");
    let content = Bytes::from_static(b"hello world");

    store
        .put(scope(), key, PutPayload::Bytes(content), metadata("application/pdf"))
        .await
        .expect("put must succeed");

    let captured = server.await.expect("server task");
    let request = captured[0].to_ascii_lowercase();
    assert!(request.contains("host: bucket.localhost:"), "captured request: {request}");
    assert!(
        request.starts_with(&format!("put /finstack/{}/docs/a.pdf http/1.1", hex16())),
        "captured request: {request}"
    );
    assert!(!request.contains("/bucket/"), "captured request must not carry a /bucket path segment");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_verifies_scope_and_digest() {
    let content = b"hello object".to_vec();
    let content_digest = Digest::blob_content(&content).to_hex();
    let scope_digest = scope().digest().expect("scope digest").to_hex();
    let (base_url, server) = serve(vec![CannedResponse::ok(
        vec![
            ("content-type", "application/octet-stream".to_owned()),
            ("x-amz-meta-fsai-digest", content_digest),
            ("x-amz-meta-fsai-scope", scope_digest),
        ],
        content.clone(),
    )])
    .await;
    let store = store(&base_url, Addressing::Path, None);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");

    let fetched = store.get(scope(), key).await.expect("get must succeed");
    assert_eq!(fetched.as_ref(), content.as_slice());
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_with_wrong_scope_header_fails_closed() {
    let content = b"hello object".to_vec();
    let content_digest = Digest::blob_content(&content).to_hex();
    let wrong_scope_digest = Digest::blob_content(b"someone-else").to_hex();
    let (base_url, server) = serve(vec![CannedResponse::ok(
        vec![
            ("content-type", "application/octet-stream".to_owned()),
            ("x-amz-meta-fsai-digest", content_digest),
            ("x-amz-meta-fsai-scope", wrong_scope_digest),
        ],
        content,
    )])
    .await;
    let store = store(&base_url, Addressing::Path, None);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");

    let error = store.get(scope(), key).await.expect_err("must fail closed");
    assert_eq!(error.code(), OBJECT_SCOPE_MISMATCH);
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_with_tampered_body_fails_integrity() {
    let original = b"hello object".to_vec();
    let content_digest = Digest::blob_content(&original).to_hex();
    let scope_digest = scope().digest().expect("scope digest").to_hex();
    let tampered = b"HELLO OBJECT".to_vec();
    let (base_url, server) = serve(vec![CannedResponse::ok(
        vec![
            ("content-type", "application/octet-stream".to_owned()),
            ("x-amz-meta-fsai-digest", content_digest),
            ("x-amz-meta-fsai-scope", scope_digest),
        ],
        tampered,
    )])
    .await;
    let store = store(&base_url, Addressing::Path, None);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");

    let error = store.get(scope(), key).await.expect_err("must detect tampering");
    assert_eq!(error.code(), finstack_ai_runtime::OBJECT_INTEGRITY_FAILURE);
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_object_maps_404_to_not_found() {
    let (base_url, server) = serve(vec![CannedResponse::status("404 Not Found")]).await;
    let store = store(&base_url, Addressing::Path, None);
    let key = ObjectKey::try_new("docs/missing.pdf").expect("key");

    let error = store.get(scope(), key).await.expect_err("missing object must fail");
    assert_eq!(error.code(), OBJECT_NOT_FOUND);
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_treats_404_as_success() {
    let (base_url, server) = serve(vec![CannedResponse::status("404 Not Found")]).await;
    let store = store(&base_url, Addressing::Path, None);
    let key = ObjectKey::try_new("docs/missing.pdf").expect("key");

    store.delete(scope(), key).await.expect("delete of a missing object must be Ok");
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_walks_continuation_tokens() {
    let hex16 = hex16();
    let first_page = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>\
         <IsTruncated>true</IsTruncated>\
         <Contents><Key>finstack/{hex16}/docs/a.pdf</Key><Size>5</Size></Contents>\
         <NextContinuationToken>TOKEN1</NextContinuationToken>\
         </ListBucketResult>"
    );
    let second_page = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>\
         <IsTruncated>false</IsTruncated>\
         <Contents><Key>finstack/{hex16}/docs/b.pdf</Key><Size>6</Size></Contents>\
         </ListBucketResult>"
    );
    let (base_url, server) = serve(vec![
        CannedResponse::ok(Vec::new(), first_page.into_bytes()),
        CannedResponse::ok(Vec::new(), second_page.into_bytes()),
    ])
    .await;
    let store = store(&base_url, Addressing::Path, None);

    let page1 = store.list(scope(), None, PageToken::first()).await.expect("first page");
    assert_eq!(page1.entries.len(), 1);
    assert_eq!(page1.entries[0].key.as_str(), "docs/a.pdf");
    assert_eq!(page1.entries[0].length, 5);
    let next = page1.next.expect("must continue");

    let page2 = store.list(scope(), None, next).await.expect("second page");
    assert_eq!(page2.entries.len(), 1);
    assert_eq!(page2.entries[0].key.as_str(), "docs/b.pdf");
    assert!(page2.next.is_none());

    let captured = server.await.expect("server task");
    assert!(
        captured[1].contains("continuation-token=TOKEN1"),
        "second request must carry the continuation token: {}",
        captured[1]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn presign_get_produces_a_signed_query_url() {
    let store = store("http://127.0.0.1:9", Addressing::Path, None);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");

    let presigned = store
        .presign_get(scope(), key, Duration::from_mins(15))
        .await
        .expect("presign must succeed with no server involved");

    assert!(presigned.url.contains("X-Amz-Signature="), "url: {}", presigned.url);
    assert!(presigned.url.contains("X-Amz-Expires=900"), "url: {}", presigned.url);
    assert!(
        presigned.url.contains(&format!("/bucket/finstack/{}/docs/a.pdf", hex16())),
        "url: {}",
        presigned.url
    );
    assert_eq!(presigned.expires_in_secs, 900);
}

#[tokio::test(flavor = "multi_thread")]
async fn oversize_file_put_is_rejected_before_any_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let accept_task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(300), listener.accept()).await
    });

    let store = store(&format!("http://{address}"), Addressing::Path, Some(1024));
    let mut file = tempfile::NamedTempFile::new().expect("tempfile");
    std::io::Write::write_all(&mut file, &vec![7_u8; 2 * 1024]).expect("write tempfile");
    let key = ObjectKey::try_new("docs/oversize.bin").expect("key");

    let error = store
        .put(scope(), key, PutPayload::File(file.path().to_path_buf()), metadata("application/octet-stream"))
        .await
        .expect_err("oversize file put must be rejected");
    assert_eq!(error.code(), OBJECT_TOO_LARGE);

    let accepted = accept_task.await.expect("accept task");
    assert!(accepted.is_err(), "the listener must never have seen a connection");
}

// ---------------------------------------------------------------------------
// Step 3: minimal in-process S3 stub satisfying the shared contract suite
// ---------------------------------------------------------------------------

mod s3_stub {
    //! Just enough HTTP/1.1 + S3 `ListObjectsV2` XML to satisfy
    //! `run_object_store_contract_suite`. Test-only: unlike the crate under
    //! test, this may use `expect` freely.

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const PAGE_SIZE: usize = 2;

    struct StoredObject {
        body: Vec<u8>,
        headers: Vec<(String, String)>,
    }

    type State = Arc<Mutex<BTreeMap<String, StoredObject>>>;

    /// Start the stub; returns the base URL and a handle that, once
    /// aborted, tears the server down.
    pub async fn start() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let state: State = Arc::new(Mutex::new(BTreeMap::new()));
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { break };
                let state = Arc::clone(&state);
                tokio::spawn(handle(socket, state));
            }
        });
        (format!("http://{address}"), task)
    }

    async fn handle(mut socket: TcpStream, state: State) {
        let Some((method, path, headers, body)) = read_request(&mut socket).await else { return };
        let (raw_path, query) = path.split_once('?').unwrap_or((path.as_str(), ""));
        let key = raw_path.strip_prefix("/bucket/").map(str::to_owned);

        match method.as_str() {
            "PUT" => {
                let Some(key) = key else { return respond(&mut socket, 400, "", Vec::new()).await };
                let meta_headers: Vec<(String, String)> = headers
                    .iter()
                    .filter(|(name, _)| name.starts_with("x-amz-meta-") || name == "content-type")
                    .cloned()
                    .collect();
                state
                    .lock()
                    .expect("lock")
                    .insert(key, StoredObject { body, headers: meta_headers });
                respond(&mut socket, 200, "OK", Vec::new()).await;
            }
            "GET" if raw_path == "/bucket" => handle_list(&mut socket, &state, query).await,
            "GET" => {
                let Some(key) = key else { return respond(&mut socket, 400, "", Vec::new()).await };
                let found = state.lock().expect("lock").get(&key).map(|object| {
                    (object.body.clone(), object.headers.clone())
                });
                match found {
                    Some((body, meta)) => respond_with_headers(&mut socket, 200, "OK", meta, body).await,
                    None => respond(&mut socket, 404, "Not Found", Vec::new()).await,
                }
            }
            "HEAD" => {
                let Some(key) = key else { return respond(&mut socket, 400, "", Vec::new()).await };
                let found = state.lock().expect("lock").get(&key).map(|object| {
                    let mut meta = object.headers.clone();
                    meta.push(("content-length".to_owned(), object.body.len().to_string()));
                    meta
                });
                match found {
                    Some(meta) => respond_with_headers(&mut socket, 200, "OK", meta, Vec::new()).await,
                    None => respond(&mut socket, 404, "Not Found", Vec::new()).await,
                }
            }
            "DELETE" => {
                if let Some(key) = key {
                    state.lock().expect("lock").remove(&key);
                }
                respond(&mut socket, 204, "No Content", Vec::new()).await;
            }
            _ => respond(&mut socket, 400, "Bad Request", Vec::new()).await,
        }
    }

    async fn handle_list(socket: &mut TcpStream, state: &State, query: &str) {
        use std::fmt::Write as _;

        let params = parse_query(query);
        let prefix = params.get("prefix").cloned().unwrap_or_default();
        let continuation = params.get("continuation-token").cloned();

        let matching: Vec<(String, usize)> = {
            let objects = state.lock().expect("lock");
            objects
                .iter()
                .filter(|(key, _)| key.starts_with(&prefix))
                .map(|(key, object)| (key.clone(), object.body.len()))
                .collect()
        };

        let start = match continuation {
            Some(cursor) => matching.iter().position(|(key, _)| *key == cursor).map_or(0, |index| index + 1),
            None => 0,
        };
        let remaining = matching.get(start..).unwrap_or_default();
        let page: Vec<&(String, usize)> = remaining.iter().take(PAGE_SIZE).collect();
        let truncated = remaining.len() > page.len();

        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>");
        xml.push_str(if truncated { "<IsTruncated>true</IsTruncated>" } else { "<IsTruncated>false</IsTruncated>" });
        for (key, size) in &page {
            let _ = write!(xml, "<Contents><Key>{key}</Key><Size>{size}</Size></Contents>");
        }
        if truncated
            && let Some((last_key, _)) = page.last()
        {
            let _ = write!(xml, "<NextContinuationToken>{last_key}</NextContinuationToken>");
        }
        xml.push_str("</ListBucketResult>");
        respond(socket, 200, "OK", xml.into_bytes()).await;
    }

    fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
        query
            .split('&')
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.split_once('='))
            .map(|(key, value)| (key.to_owned(), percent_decode(value)))
            .collect()
    }

    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index += 3;
                    continue;
                }
            }
            out.push(bytes[index]);
            index += 1;
        }
        String::from_utf8(out).unwrap_or_default()
    }

    async fn read_request(socket: &mut TcpStream) -> Option<(String, String, Vec<(String, String)>, Vec<u8>)> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 8_192];
        let header_end = loop {
            let count = socket.read(&mut buffer).await.ok()?;
            if count == 0 {
                return None;
            }
            request.extend_from_slice(&buffer[..count]);
            if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break end + 4;
            }
        };
        let header_text = String::from_utf8_lossy(&request[..header_end]).into_owned();
        let mut lines = header_text.lines();
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_owned();
        let path = parts.next().unwrap_or_default().to_owned();

        let mut headers = Vec::new();
        let mut content_length = 0_usize;
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim().to_ascii_lowercase();
                let value = value.trim().to_owned();
                if name == "content-length" {
                    content_length = value.parse().unwrap_or(0);
                }
                headers.push((name, value));
            }
        }
        while request.len() < header_end + content_length {
            let count = socket.read(&mut buffer).await.ok()?;
            if count == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..count]);
        }
        let body = request.get(header_end..header_end + content_length).unwrap_or_default().to_vec();
        Some((method, path, headers, body))
    }

    async fn respond(socket: &mut TcpStream, status: u16, reason: &str, body: Vec<u8>) {
        respond_with_headers(socket, status, reason, Vec::new(), body).await;
    }

    async fn respond_with_headers(
        socket: &mut TcpStream,
        status: u16,
        reason: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) {
        use std::fmt::Write as _;

        let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
        for (name, value) in &headers {
            let _ = write!(response, "{name}: {value}\r\n");
        }
        let _ = write!(response, "Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.write_all(&body).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn s3_store_satisfies_the_contract() {
    let (base_url, server) = s3_stub::start().await;
    let store = store(&base_url, Addressing::Path, None);
    finstack_ai_test::object_store::run_object_store_contract_suite(Arc::new(store), true).await;
    server.abort();
}

// ---------------------------------------------------------------------------
// Step 4: secret canary
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn transport_errors_never_leak_signing_material() {
    // Bind, then close, a listener: the port is valid but nothing is
    // listening, so the connection attempt fails fast.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    drop(listener);

    let secret = "supersecretvalue";
    let config = S3ObjectStoreConfig::try_new(format!("http://{address}"), "bucket", "us-east-1")
        .expect("config")
        .with_credentials("AKIAEXAMPLE", SecretString::try_new(secret).expect("secret"))
        .expect("credentials");
    let store = S3ObjectStore::try_new(config).expect("store");
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");

    let error = store
        .put(scope(), key, PutPayload::Bytes(Bytes::from_static(b"hello")), metadata("application/pdf"))
        .await
        .expect_err("closed port must fail");

    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains(secret), "rendered error: {rendered}");
    assert!(!rendered.contains("X-Amz-Signature"), "rendered error: {rendered}");
    assert_eq!(error.code(), finstack_ai_runtime::OBJECT_UNAVAILABLE);
}
