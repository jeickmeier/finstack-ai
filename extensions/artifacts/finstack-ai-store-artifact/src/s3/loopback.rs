//! Loopback HTTP fixture tests for [`super::S3ObjectStore`].
//!
//! Every test talks to a `TcpListener` bound on `127.0.0.1:0`. No live
//! credentials or external network.

use std::sync::Arc;

use finstack_ai_kernel::{Digest, Metadata, Sensitivity};
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::ports::model::SecretString;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::driver::{
    OBJECT_INTEGRITY_FAILURE, OBJECT_NOT_FOUND, OBJECT_SCOPE_MISMATCH, ObjectDriver, ObjectKey,
    ObjectMetadata, ObjectScope,
};

use super::request::payload_sha256_hex;
use super::{Addressing, S3ObjectStore, S3ObjectStoreConfig};

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

fn scope_hex() -> String {
    scope().digest().expect("scope digest").to_hex()
}

fn store(base_url: &str) -> S3ObjectStore {
    let config = S3ObjectStoreConfig::try_new(base_url, "bucket", "us-east-1")
        .expect("config")
        .with_addressing(Addressing::Path)
        .try_with_key_prefix("finstack")
        .expect("prefix")
        .with_credentials(
            "AKIAEXAMPLE",
            SecretString::try_new("supersecretvalue").expect("secret"),
        )
        .expect("credentials");
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
        Self {
            status_line: "200 OK",
            headers,
            body,
        }
    }

    fn status(status_line: &'static str) -> Self {
        Self {
            status_line,
            headers: Vec::new(),
            body: Vec::new(),
        }
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
    let _ = write!(
        response,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        canned.body.len()
    );
    socket
        .write_all(response.as_bytes())
        .await
        .expect("write headers");
    socket.write_all(&canned.body).await.expect("write body");
}

#[tokio::test(flavor = "multi_thread")]
async fn put_signs_path_style_and_sends_metadata_headers() {
    let (base_url, server) = serve(vec![CannedResponse::ok(Vec::new(), Vec::new())]).await;
    let store = store(&base_url);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");
    let content = Bytes::from_static(b"hello world");

    let object_ref = store
        .put(scope(), key, content.clone(), metadata("application/pdf"))
        .await
        .expect("put must succeed");
    assert_eq!(object_ref.length, content.len() as u64);

    let captured = server.await.expect("server task");
    let request = &captured[0];
    assert!(
        request.starts_with(&format!(
            "PUT /bucket/finstack/{}/docs/a.pdf HTTP/1.1",
            scope_hex()
        )),
        "captured request: {request}"
    );
    assert!(request.contains("authorization: AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/"));
    assert!(request.contains(&format!(
        "x-amz-content-sha256: {}",
        payload_sha256_hex(b"hello world")
    )));
    assert!(request.contains("x-amz-meta-fsai-digest:"));
    assert!(request.contains("x-amz-meta-fsai-scope:"));
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
    let store = store(&base_url);
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
    let store = store(&base_url);
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
    let store = store(&base_url);
    let key = ObjectKey::try_new("docs/a.pdf").expect("key");

    let error = store
        .get(scope(), key)
        .await
        .expect_err("must detect tampering");
    assert_eq!(error.code(), OBJECT_INTEGRITY_FAILURE);
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_object_maps_404_to_not_found() {
    let (base_url, server) = serve(vec![CannedResponse::status("404 Not Found")]).await;
    let store = store(&base_url);
    let key = ObjectKey::try_new("docs/missing.pdf").expect("key");

    let error = store
        .get(scope(), key)
        .await
        .expect_err("missing object must fail");
    assert_eq!(error.code(), OBJECT_NOT_FOUND);
    server.await.expect("server task");
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_treats_404_as_success() {
    let (base_url, server) = serve(vec![CannedResponse::status("404 Not Found")]).await;
    let store = store(&base_url);
    let key = ObjectKey::try_new("docs/missing.pdf").expect("key");

    store
        .delete(scope(), key)
        .await
        .expect("delete of a missing object must be Ok");
    server.await.expect("server task");
}
