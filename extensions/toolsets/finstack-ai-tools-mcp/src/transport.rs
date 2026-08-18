//! MCP transports: scripted (tests), stdio, and streamable HTTP.

#[cfg(test)]
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine;
use futures_util::future::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::protocol::{Meta, PROTOCOL_VERSION, jsonrpc_request};
use crate::{MCP_PROTOCOL_VIOLATION, MCP_SERVER_NOT_ALLOWLISTED, MCP_TRANSPORT_ERROR, McpError};

/// One MCP request/response round trip.
///
/// Implementations MUST merge `Meta::default()` into `params._meta` before
/// sending. `protocolVersion` and `clientCapabilities` are REQUIRED on EVERY
/// request; a server MUST reject a request missing either with -32602.
pub(crate) trait McpTransport: Send + Sync {
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>>;

    fn request_identified(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let _ = id;
        self.request(method, params)
    }
}

/// Merge required metadata into a params object.
pub(crate) fn with_meta(mut params: serde_json::Value) -> serde_json::Value {
    let meta = serde_json::to_value(Meta::default()).expect("meta serializes");
    if let Some(object) = params.as_object_mut() {
        object.insert("_meta".to_owned(), meta);
        return params;
    }
    serde_json::json!({ "_meta": meta })
}

/// In-memory transport that returns queued results and asserts `_meta`.
#[cfg(test)]
pub(crate) struct ScriptedTransport {
    responses: Mutex<VecDeque<Result<serde_json::Value, McpError>>>,
    methods: Mutex<Vec<String>>,
}

#[cfg(test)]
impl ScriptedTransport {
    pub(crate) fn new(responses: Vec<serde_json::Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(Ok).collect()),
            methods: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn with_error(code: i64, message: &str) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                format!("jsonrpc {code} {message}"),
            ))])),
            methods: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn called_methods(&self) -> Vec<String> {
        self.methods
            .lock()
            .map(|methods| methods.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl McpTransport for ScriptedTransport {
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let method = method.to_owned();
        Box::pin(async move {
            if let Ok(mut methods) = self.methods.lock() {
                methods.push(method);
            }
            let params = with_meta(params);
            let version = params
                .get("_meta")
                .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
                .and_then(serde_json::Value::as_str);
            if version != Some(PROTOCOL_VERSION) {
                return Err(McpError::stable(
                    MCP_PROTOCOL_VIOLATION,
                    "scripted transport missing required protocolVersion",
                ));
            }
            let mut queue = self.responses.lock().map_err(|_| {
                McpError::stable(MCP_TRANSPORT_ERROR, "scripted transport lock is poisoned")
            })?;
            queue.pop_front().unwrap_or_else(|| {
                Err(McpError::stable(
                    MCP_TRANSPORT_ERROR,
                    "scripted transport has no queued response",
                ))
            })
        })
    }
}

/// Allowlisted stdio MCP server command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioConfig {
    /// Exact program path or allowlisted basename.
    pub program: PathBuf,
    /// Additional argv after the program.
    pub args: Vec<String>,
}

impl StdioConfig {
    /// Build a stdio target.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, args: impl Into<Vec<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into(),
        }
    }

    pub(crate) fn identity(&self) -> String {
        let mut identity = self.program.display().to_string();
        for arg in &self.args {
            identity.push(' ');
            identity.push_str(arg);
        }
        identity
    }
}

struct StdioState {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Newline-framed stdio transport. T1; not a sandbox.
pub(crate) struct StdioTransport {
    state: tokio::sync::Mutex<StdioState>,
    next_id: AtomicU64,
}

impl StdioTransport {
    pub(crate) fn try_spawn(config: &StdioConfig) -> Result<Self, McpError> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            McpError::stable(MCP_TRANSPORT_ERROR, format!("stdio spawn failed: {error}"))
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::stable(MCP_TRANSPORT_ERROR, "stdio child stdin is unavailable")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::stable(MCP_TRANSPORT_ERROR, "stdio child stdout is unavailable")
        })?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(&mut stderr);
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    line.clear();
                }
            });
        }
        Ok(Self {
            state: tokio::sync::Mutex::new(StdioState {
                _child: child,
                stdin,
                stdout: BufReader::new(stdout),
            }),
            next_id: AtomicU64::new(1),
        })
    }

    /// Encode one message as a single newline-terminated line.
    ///
    /// A message MUST NOT contain an embedded newline; `serde_json`'s compact
    /// form escapes newlines inside strings, so any raw `\n` in the output is a
    /// framing bug and is rejected rather than sent.
    pub(crate) fn encode_line(message: &serde_json::Value) -> Result<String, McpError> {
        let encoded = serde_json::to_string(message)
            .map_err(|error| McpError::stable(MCP_PROTOCOL_VIOLATION, error.to_string()))?;
        Self::finish_line(&encoded)
    }

    pub(crate) fn finish_line(encoded: &str) -> Result<String, McpError> {
        if encoded.contains('\n') {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "stdio message contains an embedded newline",
            ));
        }
        Ok(format!("{encoded}\n"))
    }

    async fn round_trip(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let params = with_meta(params);
        let message = jsonrpc_request(&id, method, &params);
        let line = Self::encode_line(&message)?;
        let mut state = self.state.lock().await;
        state
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| {
                McpError::stable(MCP_TRANSPORT_ERROR, format!("stdio write failed: {error}"))
            })?;
        state.stdin.flush().await.map_err(|error| {
            McpError::stable(MCP_TRANSPORT_ERROR, format!("stdio flush failed: {error}"))
        })?;
        let mut response = String::new();
        let read = state
            .stdout
            .read_line(&mut response)
            .await
            .map_err(|error| {
                McpError::stable(MCP_TRANSPORT_ERROR, format!("stdio read failed: {error}"))
            })?;
        if read == 0 {
            return Err(McpError::stable(
                MCP_TRANSPORT_ERROR,
                "stdio server closed stdout",
            ));
        }
        parse_jsonrpc_response(&response)
    }
}

impl McpTransport for StdioTransport {
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let id = serde_json::json!(self.next_id.fetch_add(1, Ordering::Relaxed));
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params).await })
    }

    fn request_identified(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params).await })
    }
}

/// Allowlisted streamable-HTTP MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    url: String,
}

impl HttpConfig {
    /// Construct a streamable-HTTP target.
    ///
    /// # Errors
    ///
    /// Rejects an empty URL.
    pub fn try_new(url: impl Into<String>) -> Result<Self, McpError> {
        let url = url.into();
        if url.is_empty() {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "http url is empty",
            ));
        }
        Ok(Self { url })
    }

    /// SSE-only servers are rejected. Streamable HTTP is POST-only.
    ///
    /// # Errors
    ///
    /// Always returns [`MCP_PROTOCOL_VIOLATION`].
    pub fn sse_only(_url: impl Into<String>) -> Result<Self, McpError> {
        Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "sse-only transports are rejected",
        ))
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

/// Streamable HTTP transport. T4; not isolated.
pub(crate) struct HttpTransport {
    url: String,
    client: reqwest::Client,
    next_id: AtomicU64,
}

impl HttpTransport {
    pub(crate) fn try_new(config: &HttpConfig) -> Result<Self, McpError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| {
                McpError::stable(MCP_TRANSPORT_ERROR, format!("http client failed: {error}"))
            })?;
        Ok(Self {
            url: config.url.clone(),
            client,
            next_id: AtomicU64::new(1),
        })
    }

    /// Mandatory headers for one POST.
    ///
    /// The server validates header-vs-body agreement and MUST reject a mismatch
    /// with 400 and -32020, so these are derived from the request, never
    /// hardcoded per call site.
    pub(crate) fn headers_for(method: &str, name: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_static(PROTOCOL_VERSION),
        );
        headers.insert(
            HeaderName::from_static("mcp-method"),
            HeaderValue::from_str(method).expect("ascii method"),
        );
        if let Some(name) = name {
            headers.insert(
                HeaderName::from_static("mcp-name"),
                encode_header_value(name),
            );
        }
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        headers
    }

    async fn round_trip(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, McpError> {
        let params = with_meta(params);
        let name = params.get("name").and_then(serde_json::Value::as_str);
        let body = jsonrpc_request(&id, method, &params);
        let response = self
            .client
            .post(&self.url)
            .headers(Self::headers_for(method, name))
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                McpError::stable(MCP_TRANSPORT_ERROR, format!("http post failed: {error}"))
            })?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let text = response.text().await.map_err(|error| {
            McpError::stable(MCP_TRANSPORT_ERROR, format!("http body failed: {error}"))
        })?;
        if content_type.starts_with("text/event-stream") {
            return parse_sse_jsonrpc(&text);
        }
        parse_jsonrpc_response(&text)
    }
}

impl McpTransport for HttpTransport {
    fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let id = serde_json::json!(self.next_id.fetch_add(1, Ordering::Relaxed));
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params).await })
    }

    fn request_identified(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params).await })
    }
}

/// Non-ASCII or unsafe header values use the sentinel encoding
/// `=?base64?{Base64EncodedValue}?=` required by the spec.
fn encode_header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| {
        let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
        HeaderValue::from_str(&format!("=?base64?{encoded}?=")).expect("sentinel encoding is ascii")
    })
}

fn parse_jsonrpc_response(text: &str) -> Result<serde_json::Value, McpError> {
    let value: serde_json::Value = serde_json::from_str(text.trim()).map_err(|error| {
        McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            format!("jsonrpc response is not json: {error}"),
        )
    })?;
    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("jsonrpc error");
        return Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            format!("jsonrpc {code} {message}"),
        ));
    }
    value.get("result").cloned().ok_or_else(|| {
        McpError::stable(MCP_PROTOCOL_VIOLATION, "jsonrpc response is missing result")
    })
}

fn parse_sse_jsonrpc(text: &str) -> Result<serde_json::Value, McpError> {
    let mut data = String::new();
    for line in text.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(payload) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(payload.trim_start());
        }
    }
    if data.is_empty() {
        return Err(McpError::stable(
            MCP_TRANSPORT_ERROR,
            "sse stream contained no data event",
        ));
    }
    parse_jsonrpc_response(&data)
}

pub(crate) fn authorize_stdio(allowed: &[Arc<str>], program: &Path) -> Result<(), McpError> {
    let displayed = program.to_string_lossy();
    if allowed
        .iter()
        .any(|entry| entry.as_ref() == displayed.as_ref())
    {
        return Ok(());
    }
    if let Some(name) = program.file_name().and_then(|name| name.to_str())
        && allowed.iter().any(|entry| entry.as_ref() == name)
    {
        return Ok(());
    }
    Err(McpError::stable(
        MCP_SERVER_NOT_ALLOWLISTED,
        "stdio program is not allowlisted",
    ))
}

pub(crate) fn authorize_http(allowed: &[Arc<str>], url: &str) -> Result<(), McpError> {
    if allowed.iter().any(|entry| entry.as_ref() == url) {
        return Ok(());
    }
    Err(McpError::stable(
        MCP_SERVER_NOT_ALLOWLISTED,
        "http url is not allowlisted",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scripted_transport_returns_the_queued_response() {
        let transport = ScriptedTransport::new(vec![serde_json::json!({
            "resultType": "complete",
            "tools": []
        })]);
        let value = transport
            .request("tools/list", serde_json::json!({}))
            .await
            .expect("request succeeds");
        assert_eq!(value["resultType"], "complete");
    }

    #[tokio::test]
    async fn transport_rejects_a_jsonrpc_error_member() {
        let transport = ScriptedTransport::with_error(-32601, "Method not found");
        let error = transport
            .request("tools/list", serde_json::json!({}))
            .await
            .expect_err("must surface the protocol error");
        assert!(format!("{error}").contains("-32601"));
    }

    #[tokio::test]
    async fn stdio_framing_rejects_an_embedded_newline() {
        let escaped = StdioTransport::encode_line(&serde_json::json!({"a": "b\nc"}))
            .expect("compact json escapes embedded newlines");
        assert!(escaped.ends_with('\n'));
        assert_eq!(escaped.matches('\n').count(), 1);
        let error = StdioTransport::finish_line("{\"a\":\n1}")
            .expect_err("embedded newline must be rejected");
        assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
    }

    #[test]
    fn http_request_carries_the_mandatory_headers() {
        let headers = HttpTransport::headers_for("tools/call", Some("get_weather"));
        assert_eq!(
            headers.get("MCP-Protocol-Version").unwrap(),
            PROTOCOL_VERSION
        );
        assert_eq!(headers.get("Mcp-Method").unwrap(), "tools/call");
        assert_eq!(headers.get("Mcp-Name").unwrap(), "get_weather");
    }

    #[test]
    fn sse_only_transport_is_rejected() {
        let error = HttpConfig::sse_only("https://example.invalid/sse")
            .expect_err("sse-only must be rejected");
        assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
    }
}
