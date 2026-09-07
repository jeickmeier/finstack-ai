//! Offline Ollama-loopback NDJSON server for composition tests.
//!
//! Copied from `examples/rust-minimal` (test doubles are copied per crate,
//! never shared); trimmed to the text-response subset these tests use.

use std::error::Error;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub type BoxError = Box<dyn Error + Send + Sync>;

/// One NDJSON text completion used by the offline loopback server.
#[must_use]
pub fn text_response(text: &str) -> String {
    format!(
        "{{\"message\":{{\"role\":\"assistant\",\"content\":\"{text}\"}},\"done\":false}}\n{{\"message\":{{\"role\":\"assistant\",\"content\":\"\"}},\"done\":true,\"prompt_eval_count\":1,\"eval_count\":1}}\n"
    )
}

/// Serve a finite sequence of NDJSON bodies on a loopback listener.
pub async fn serve_ndjson(
    responses: Vec<String>,
) -> Result<(String, tokio::task::JoinHandle<Result<(), String>>), BoxError> {
    let (address, task) = serve_ndjson_capture(responses).await?;
    Ok((
        address,
        tokio::spawn(async move { task.await.map_err(|error| error.to_string())?.map(|_| ()) }),
    ))
}

/// As [`serve_ndjson`], additionally returning each request body received.
pub async fn serve_ndjson_capture(
    responses: Vec<String>,
) -> Result<(String, tokio::task::JoinHandle<Result<Vec<String>, String>>), BoxError> {
    serve_capture(responses, false).await
}

/// Index the actual attachment advertised in the first captured request.
pub async fn serve_ingest_capture(
    responses: Vec<String>,
) -> Result<(String, tokio::task::JoinHandle<Result<Vec<String>, String>>), BoxError> {
    serve_capture(responses, true).await
}

async fn serve_capture(
    mut responses: Vec<String>,
    index_attachment: bool,
) -> Result<(String, tokio::task::JoinHandle<Result<Vec<String>, String>>), BoxError> {
    if index_attachment {
        responses.insert(0, String::new());
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let mut captured = Vec::new();
        for (index, body) in responses.into_iter().enumerate() {
            let (mut socket, _) = listener.accept().await.map_err(|error| error.to_string())?;
            let (_path, request_body) = read_request(&mut socket).await?;
            let body = if index_attachment && index == 0 {
                index_response(&request_body)?
            } else {
                body
            };
            captured.push(request_body);
            write_response(&mut socket, "application/x-ndjson", &body).await?;
        }
        Ok(captured)
    });
    Ok((format!("http://{address}"), task))
}

/// As [`serve_ndjson_capture`], additionally answering `POST /api/embed`
/// deterministically with [`HashEmbedder`]-computed vectors.
///
/// Chat requests consume `responses` in order and their bodies are
/// captured (embed requests are answered, not captured, and may arrive in
/// any number and order between them). The task ends — and the listener
/// closes — once the last chat response is served, so post-run best-effort
/// embed drains simply see a refused connection.
pub async fn serve_ollama_scripted(
    responses: Vec<String>,
    embed_dimensions: usize,
) -> Result<(String, tokio::task::JoinHandle<Result<Vec<String>, String>>), BoxError> {
    use finstack_ai_embeddings::embedder::HashEmbedder;

    let embedder = HashEmbedder::try_new(embed_dimensions).map_err(|error| error.to_string())?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let mut captured = Vec::new();
        let mut pending = responses.into_iter();
        let mut next_chat = pending.next();
        while let Some(chat_body) = next_chat.take() {
            let (mut socket, _) = listener.accept().await.map_err(|error| error.to_string())?;
            let (path, request_body) = read_request(&mut socket).await?;
            if path == "/api/embed" {
                let body = embed_body(&embedder, &request_body).await?;
                write_response(&mut socket, "application/json", &body).await?;
                next_chat = Some(chat_body);
            } else {
                captured.push(request_body);
                write_response(&mut socket, "application/x-ndjson", &chat_body).await?;
                next_chat = pending.next();
            }
        }
        Ok(captured)
    });
    Ok((format!("http://{address}"), task))
}

/// Compute the `/api/embed` response for `request_body` with `embedder`.
async fn embed_body(
    embedder: &finstack_ai_embeddings::embedder::HashEmbedder,
    request_body: &str,
) -> Result<String, String> {
    use finstack_ai_embeddings::embedder::TextEmbedder;

    let parsed: serde_json::Value =
        serde_json::from_str(request_body).map_err(|error| error.to_string())?;
    let inputs = parsed
        .get("input")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "embed request lacks an input array".to_owned())?;
    let texts = inputs
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(std::sync::Arc::<str>::from)
                .ok_or_else(|| "embed request input is not a string".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let vectors = embedder
        .embed(texts)
        .await
        .map_err(|error| error.to_string())?;
    let components: Vec<Vec<f32>> = vectors
        .iter()
        .map(|vector| vector.as_slice().to_vec())
        .collect();
    serde_json::to_string(&serde_json::json!({ "embeddings": components }))
        .map_err(|error| error.to_string())
}

/// Write one `200 OK` response with `content_type` and `body`, then close.
async fn write_response(
    socket: &mut tokio::net::TcpStream,
    content_type: &str,
    body: &str,
) -> Result<(), String> {
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket
        .write_all(headers.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    socket
        .write_all(body.as_bytes())
        .await
        .map_err(|error| error.to_string())
}

/// Read one HTTP request whole; returns its path and body.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<(String, String), String> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let header_end = loop {
        let count = socket
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("request closed before headers".to_owned());
        }
        request.extend_from_slice(&buffer[..count]);
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "request line lacks a path".to_owned())?
        .to_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .and_then(|value| value.parse::<usize>().ok())
        })
        .ok_or_else(|| "request omitted content-length".to_owned())?;
    while request.len() < header_end + content_length {
        let count = socket
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("request closed before body".to_owned());
        }
        request.extend_from_slice(&buffer[..count]);
    }
    let body = String::from_utf8_lossy(&request[header_end..]).into_owned();
    Ok((path, body))
}

/// Build an indexing call using only the uploaded reference in model context.
fn index_response(request: &str) -> Result<String, String> {
    let request: serde_json::Value = serde_json::from_str(request).map_err(|e| e.to_string())?;
    let messages = request["messages"].as_array().ok_or("missing messages")?;
    let reference = messages
        .iter()
        .filter_map(|message| message["content"].as_str())
        .find_map(|content| {
            content
                .split_once("Document source reference: ")
                .map(|(_, tail)| tail)
        })
        .and_then(|tail| tail.lines().next())
        .ok_or("missing uploaded source reference")?;
    let arguments: serde_json::Value =
        serde_json::from_str(reference).map_err(|e| e.to_string())?;
    let call = serde_json::json!({"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"index_document","arguments":arguments}}]},"done":false});
    let done = serde_json::json!({"message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":1,"eval_count":1});
    Ok(format!("{call}\n{done}\n"))
}
