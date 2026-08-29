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
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let mut captured = Vec::new();
        for body in responses {
            let (mut socket, _) = listener.accept().await.map_err(|error| error.to_string())?;
            captured.push(read_request(&mut socket).await?);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket
                .write_all(headers.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
            socket
                .write_all(body.as_bytes())
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(captured)
    });
    Ok((format!("http://{address}"), task))
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Result<String, String> {
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
    Ok(String::from_utf8_lossy(&request[header_end..]).into_owned())
}
