//! One-request loopback fixture for runnable lifecycle recipes.
//! Application definitions still use the released Ollama provider.

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::{BoxError, read_request, text_response};

/// A bounded fixture whose caller owns the reply and server task lifecycle.
pub struct ControlledServer {
    /// Operator-configured loopback endpoint for the native provider.
    pub endpoint: String,
    /// Resolves after a real provider request reaches the fixture.
    pub request: oneshot::Receiver<String>,
    /// Supply the final text only after checking the received request.
    pub reply: oneshot::Sender<String>,
    /// Join on completion, or abort and join after explicit run cancellation.
    pub task: tokio::task::JoinHandle<Result<(), String>>,
}

/// Start one deterministic offline model response, bounded to twenty seconds.
///
/// # Errors
/// Returns listener setup errors. Transport/timeout failures arrive on `task`.
pub async fn serve() -> Result<ControlledServer, BoxError> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let (request_tx, request) = oneshot::channel();
    let (reply, reply_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(20), async move {
            let (mut socket, _) = listener.accept().await.map_err(|error| error.to_string())?;
            let body = read_request(&mut socket).await?;
            request_tx.send(body).map_err(|_| "request consumer closed".to_owned())?;
            let answer: String = reply_rx.await.map_err(|_| "reply producer closed".to_owned())?;
            let body = text_response(&answer, "controlled");
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            socket.write_all(header.as_bytes()).await.map_err(|error| error.to_string())?;
            socket.write_all(body.as_bytes()).await.map_err(|error| error.to_string())
        }).await.map_err(|_| "controlled server deadline".to_owned())?
    });
    Ok(ControlledServer {
        endpoint,
        request,
        reply,
        task,
    })
}
