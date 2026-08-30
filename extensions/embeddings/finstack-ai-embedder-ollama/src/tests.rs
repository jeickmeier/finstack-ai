use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use finstack_ai_embeddings::embedder::{EmbedError, TextEmbedder};
use finstack_ai_embeddings::vector::EMBEDDING_MAX_DIMENSIONS;

use crate::config::OllamaEmbedderConfig;
use crate::embedder::OllamaEmbedder;

/// One scripted HTTP response served by the loopback test server.
struct ScriptedResponse {
    status: u16,
    body: String,
}

fn ok_body(body: &str) -> ScriptedResponse {
    ScriptedResponse {
        status: 200,
        body: body.to_owned(),
    }
}

/// Serve one plain-JSON HTTP response per connection on a loopback
/// listener; joining the handle yields the raw requests that were read.
fn serve_json(responses: Vec<ScriptedResponse>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let (mut socket, _) = listener.accept().expect("accept connection");
            requests.push(read_http_request(&mut socket));
            let reason = if response.status == 200 {
                "OK"
            } else {
                "Error"
            };
            let payload = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status,
                reason,
                response.body.len(),
                response.body
            );
            // A client that aborts mid-body (e.g. on its size limit) closes
            // the socket early; the write failure is irrelevant to the test.
            let _ = socket.write_all(payload.as_bytes());
        }
        requests
    });
    (format!("http://{address}"), handle)
}

/// Read one HTTP request (headers plus content-length body) whole.
fn read_http_request(socket: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let header_end = loop {
        let count = socket.read(&mut buffer).expect("read request headers");
        assert!(count > 0, "request closed before headers");
        request.extend_from_slice(&buffer[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .expect("request content-length");
    while request.len() < header_end + content_length {
        let count = socket.read(&mut buffer).expect("read request body");
        assert!(count > 0, "request closed before body");
        request.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(request).expect("request is UTF-8")
}

fn embedder_for(base_url: &str, dimensions: usize) -> OllamaEmbedder {
    let config = OllamaEmbedderConfig::try_new(base_url, "nomic-embed-text", dimensions)
        .expect("embedder config");
    OllamaEmbedder::try_new(config).expect("embedder")
}

fn unavailable(message: &str) -> EmbedError {
    EmbedError::Unavailable {
        message: Arc::from(message),
    }
}

#[tokio::test]
async fn embed_happy_path_preserves_batch_order_and_request_shape() {
    let (base_url, server) = serve_json(vec![ok_body(
        r#"{"model":"nomic-embed-text","embeddings":[[1.0,0.0,0.0],[0.0,1.0,0.0]],"total_duration":7}"#,
    )]);
    let embedder = embedder_for(&base_url, 3);

    let vectors = embedder
        .embed(vec![Arc::from("first text"), Arc::from("second text")])
        .await
        .expect("embed batch");
    assert_eq!(vectors.len(), 2);
    assert_eq!(vectors[0].as_slice(), &[1.0, 0.0, 0.0]);
    assert_eq!(vectors[1].as_slice(), &[0.0, 1.0, 0.0]);

    let requests = server.join().expect("server thread");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("POST /api/embed HTTP/1.1"));
    let body_start = requests[0].find("\r\n\r\n").expect("body separator") + 4;
    let body: serde_json::Value =
        serde_json::from_str(&requests[0][body_start..]).expect("request body JSON");
    assert_eq!(body["model"], "nomic-embed-text");
    assert_eq!(
        body["input"],
        serde_json::json!(["first text", "second text"])
    );
}

#[tokio::test]
async fn embed_maps_wrong_dimensions_to_unavailable() {
    let (base_url, server) = serve_json(vec![ok_body(r#"{"embeddings":[[1.0,0.0]]}"#)]);
    let embedder = embedder_for(&base_url, 3);

    let error = embedder
        .embed(vec![Arc::from("some text")])
        .await
        .expect_err("dimension mismatch");
    assert_eq!(
        error,
        unavailable("ollama embed response vector has the wrong dimensions")
    );
    server.join().expect("server thread");
}

#[tokio::test]
async fn embed_maps_vector_count_mismatch_to_unavailable() {
    let (base_url, server) = serve_json(vec![ok_body(r#"{"embeddings":[[1.0,0.0,0.0]]}"#)]);
    let embedder = embedder_for(&base_url, 3);

    let error = embedder
        .embed(vec![Arc::from("first text"), Arc::from("second text")])
        .await
        .expect_err("count mismatch");
    assert_eq!(
        error,
        unavailable("ollama embed response vector count does not match the input count")
    );
    server.join().expect("server thread");
}

#[tokio::test]
async fn embed_maps_unsuccessful_status_to_unavailable() {
    let (base_url, server) = serve_json(vec![ScriptedResponse {
        status: 500,
        body: r#"{"error":"model runner exploded"}"#.to_owned(),
    }]);
    let embedder = embedder_for(&base_url, 3);

    let error = embedder
        .embed(vec![Arc::from("some text")])
        .await
        .expect_err("http 500");
    assert_eq!(
        error,
        unavailable("ollama embed endpoint returned an unsuccessful status")
    );
    server.join().expect("server thread");
}

#[tokio::test]
async fn embed_maps_non_finite_component_to_unavailable() {
    // 1e39 exceeds the f32 range; serde_json refuses the out-of-range
    // component at the parse boundary, so no non-finite value can ever
    // reach an `EmbeddingVector`.
    let (base_url, server) = serve_json(vec![ok_body(r#"{"embeddings":[[1e39,1.0,1.0]]}"#)]);
    let embedder = embedder_for(&base_url, 3);

    let error = embedder
        .embed(vec![Arc::from("some text")])
        .await
        .expect_err("non-finite component");
    assert_eq!(
        error,
        unavailable("ollama embed response body is not valid JSON")
    );
    server.join().expect("server thread");
}

#[tokio::test]
async fn embed_maps_a_zero_vector_to_unavailable() {
    // An all-zero embedding parses fine but fails vector validation
    // (`EmbeddingVector::try_new`): its norm is undefined.
    let (base_url, server) = serve_json(vec![ok_body(r#"{"embeddings":[[0.0,0.0,0.0]]}"#)]);
    let embedder = embedder_for(&base_url, 3);

    let error = embedder
        .embed(vec![Arc::from("some text")])
        .await
        .expect_err("zero vector");
    assert_eq!(
        error,
        unavailable("ollama embed response vector is invalid")
    );
    server.join().expect("server thread");
}

#[tokio::test]
async fn embed_maps_literal_nan_body_to_unavailable() {
    // A bare NaN token is not valid JSON, so the body fails to parse.
    let (base_url, server) = serve_json(vec![ok_body(r#"{"embeddings":[[NaN,1.0,1.0]]}"#)]);
    let embedder = embedder_for(&base_url, 3);

    let error = embedder
        .embed(vec![Arc::from("some text")])
        .await
        .expect_err("NaN body");
    assert_eq!(
        error,
        unavailable("ollama embed response body is not valid JSON")
    );
    server.join().expect("server thread");
}

#[tokio::test]
async fn embed_rejects_a_response_body_over_the_size_limit() {
    let oversized = "0".repeat(17 * 1_048_576);
    let (base_url, server) = serve_json(vec![ok_body(&oversized)]);
    let embedder = embedder_for(&base_url, 3);

    let error = embedder
        .embed(vec![Arc::from("some text")])
        .await
        .expect_err("oversized body");
    assert_eq!(
        error,
        unavailable("ollama embed response exceeded the size limit")
    );
    server.join().expect("server thread");
}

#[tokio::test]
async fn embed_maps_transport_failure_to_unavailable() {
    // Bind then drop a loopback listener so the port is closed: the
    // connection is refused without any live server.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    drop(listener);
    let embedder = embedder_for(&format!("http://{address}"), 3);

    let error = embedder
        .embed(vec![Arc::from("some text")])
        .await
        .expect_err("connection refused");
    assert_eq!(error, unavailable("ollama embed request could not be sent"));
}

#[tokio::test]
async fn embed_rejects_invalid_inputs_without_contacting_the_endpoint() {
    // Port 9 is never bound in these tests: input validation must fail
    // before any connection is attempted.
    let embedder = embedder_for("http://127.0.0.1:9", 3);

    for empty in ["", "   ", "\t\n"] {
        assert_eq!(
            embedder.embed(vec![Arc::from(empty)]).await,
            Err(EmbedError::InvalidInput {
                reason: "embed_input_empty",
            })
        );
    }

    let too_long = "x".repeat(8_193);
    assert_eq!(
        embedder.embed(vec![Arc::from(too_long.as_str())]).await,
        Err(EmbedError::InvalidInput {
            reason: "embed_input_too_long",
        })
    );

    // One invalid text fails the whole batch.
    assert_eq!(
        embedder.embed(vec![Arc::from("fine"), Arc::from("")]).await,
        Err(EmbedError::InvalidInput {
            reason: "embed_input_empty",
        })
    );
}

#[tokio::test]
async fn embed_rejects_an_oversized_batch_before_sending() {
    let embedder = embedder_for("http://127.0.0.1:9", 3);
    let text: Arc<str> = Arc::from("x".repeat(8_192));
    let batch = vec![text; 2_100];

    assert_eq!(
        embedder.embed(batch).await,
        Err(EmbedError::InvalidInput {
            reason: "embed_batch_too_large",
        })
    );
}

#[tokio::test]
async fn embed_of_an_empty_batch_is_ok_without_network() {
    let embedder = embedder_for("http://127.0.0.1:9", 3);
    assert_eq!(embedder.embed(Vec::new()).await, Ok(Vec::new()));
}

#[test]
fn descriptor_id_is_stable_and_encodes_model_and_dimensions() {
    let embedder = embedder_for("http://127.0.0.1:11434", 768);
    let descriptor = embedder.descriptor();
    assert_eq!(
        descriptor.embedder_id.as_ref(),
        "embed.ollama.nomic-embed-text.768"
    );
    assert_eq!(descriptor.dimensions, 768);
    assert_eq!(descriptor.max_input_bytes, 8_192);

    // Dimensionality is part of the identity.
    assert_eq!(
        embedder_for("http://127.0.0.1:11434", 64)
            .descriptor()
            .embedder_id
            .as_ref(),
        "embed.ollama.nomic-embed-text.64"
    );

    // So is the model name.
    let config = OllamaEmbedderConfig::try_new("http://127.0.0.1:11434", "mxbai-embed-large", 512)
        .expect("embedder config");
    let embedder = OllamaEmbedder::try_new(config).expect("embedder");
    assert_eq!(
        embedder.descriptor().embedder_id.as_ref(),
        "embed.ollama.mxbai-embed-large.512"
    );
}

#[test]
fn config_rejects_invalid_base_urls() {
    for url in [
        "not a url",
        "ftp://127.0.0.1:11434",
        "http://user:pass@127.0.0.1:11434",
        "http://127.0.0.1:11434?token=nope",
        "http://127.0.0.1:11434/#fragment",
        "http://example.test:11434",
        "http://localhost:11434",
    ] {
        assert_eq!(
            OllamaEmbedderConfig::try_new(url, "nomic-embed-text", 8).err(),
            Some(EmbedError::InvalidInput {
                reason: "embedder_base_url_invalid",
            }),
            "url: {url}"
        );
    }
    assert!(OllamaEmbedderConfig::try_new("http://[::1]:11434", "nomic-embed-text", 8).is_ok());
    assert!(
        OllamaEmbedderConfig::try_new("https://ollama.example.test", "nomic-embed-text", 8).is_ok()
    );
}

#[test]
fn config_rejects_invalid_models() {
    let oversized = "m".repeat(257);
    for model in ["", " ", "two words", "line\nbreak", oversized.as_str()] {
        assert_eq!(
            OllamaEmbedderConfig::try_new("http://127.0.0.1:11434", model, 8).err(),
            Some(EmbedError::InvalidInput {
                reason: "embedder_model_invalid",
            }),
            "model: {model:?}"
        );
    }
    assert!(
        OllamaEmbedderConfig::try_new("http://127.0.0.1:11434", "nomic-embed-text:v1.5", 8).is_ok()
    );
}

#[test]
fn config_rejects_invalid_dimensions() {
    for dimensions in [0, EMBEDDING_MAX_DIMENSIONS + 1] {
        assert_eq!(
            OllamaEmbedderConfig::try_new("http://127.0.0.1:11434", "nomic-embed-text", dimensions)
                .err(),
            Some(EmbedError::InvalidInput {
                reason: "embedder_dimensions_invalid",
            })
        );
    }
    assert!(
        OllamaEmbedderConfig::try_new(
            "http://127.0.0.1:11434",
            "nomic-embed-text",
            EMBEDDING_MAX_DIMENSIONS,
        )
        .is_ok()
    );
}
