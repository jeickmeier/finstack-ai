//! MCP transports: scripted (tests), stdio, and streamable HTTP.

#[cfg(test)]
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::protocol::{Meta, PROTOCOL_VERSION, jsonrpc_request};
use crate::{
    MCP_LIMIT_EXCEEDED, MCP_PROTOCOL_VIOLATION, MCP_SERVER_NOT_ALLOWLISTED, MCP_TRANSPORT_ERROR,
    McpError,
};

const MAX_LINE_BYTES: u64 = 1024 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_NOTIFICATIONS_PER_ROUND_TRIP: usize = 32;

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

    /// Drain transport-delivered MCP notifications. Default is none.
    fn take_notifications(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
fn optional_list_response(
    queue: &mut VecDeque<Result<serde_json::Value, McpError>>,
    items_key: &str,
) -> Result<serde_json::Value, McpError> {
    let queued = queue.front().and_then(|entry| entry.as_ref().ok());
    if queued.is_some_and(|value| value.get(items_key).is_some()) {
        return queue.pop_front().expect("front exists");
    }
    Ok(serde_json::json!({"resultType":"complete"}))
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
    notifications: Mutex<Vec<String>>,
}

#[cfg(test)]
impl ScriptedTransport {
    pub(crate) fn new(responses: Vec<serde_json::Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(Ok).collect()),
            methods: Mutex::new(Vec::new()),
            notifications: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn with_error(code: i64, message: &str) -> Self {
        Self {
            responses: Mutex::new(VecDeque::from([Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                format!("jsonrpc {code} {message}"),
            )
            .with_jsonrpc(code))])),
            methods: Mutex::new(Vec::new()),
            notifications: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn with_pending_notifications(self, notifications: Vec<String>) -> Self {
        if let Ok(mut queued) = self.notifications.lock() {
            *queued = notifications;
        }
        self
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
                methods.push(method.clone());
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
            if method == "prompts/list" {
                return optional_list_response(&mut queue, "prompts");
            }
            if method == "resources/templates" {
                return optional_list_response(&mut queue, "resourceTemplates");
            }
            queue.pop_front().unwrap_or_else(|| {
                Err(McpError::stable(
                    MCP_TRANSPORT_ERROR,
                    "scripted transport has no queued response",
                ))
            })
        })
    }

    fn take_notifications(&self) -> Vec<String> {
        self.notifications
            .lock()
            .map(|mut queued| std::mem::take(&mut *queued))
            .unwrap_or_default()
    }
}

/// Allowlisted stdio MCP server command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioConfig {
    /// Exact program path or allowlisted basename.
    pub program: PathBuf,
    /// Additional argv after the program.
    pub args: Vec<String>,
    confinement: Option<(
        finstack_ai_runtime::ProcessConfinement,
        finstack_ai_runtime::ConfinementProfile,
    )>,
}

impl StdioConfig {
    /// Build a stdio target.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, args: impl Into<Vec<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into(),
            confinement: None,
        }
    }

    /// Request the same runtime confinement service used by the shell crate.
    ///
    /// Unconfined stdio stays the default T1 path. Requested-and-unavailable
    /// fails closed at spawn. Windows uses `ProcessConfinement::spawn`
    /// (`CreateProcessAsUser` plus Job Object); it does not fall back to
    /// an unconfined tokio spawn.
    #[must_use]
    pub fn with_confinement(
        mut self,
        confinement: finstack_ai_runtime::ProcessConfinement,
        profile: finstack_ai_runtime::ConfinementProfile,
    ) -> Self {
        self.confinement = Some((confinement, profile));
        self
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

enum StdioState {
    Unconfined {
        _child: Child,
        stdin: ChildStdin,
        stdout: BufReader<ChildStdout>,
    },
    Confined {
        _child: finstack_ai_runtime::ConfinedChild,
        stdin: tokio::fs::File,
        stdout: BufReader<tokio::fs::File>,
    },
}

/// Newline-framed stdio transport. T1; not a sandbox.
pub(crate) struct StdioTransport {
    state: tokio::sync::Mutex<StdioState>,
    next_id: AtomicU64,
    notifications: Mutex<Vec<String>>,
    poisoned: AtomicBool,
}

impl StdioTransport {
    pub(crate) fn try_spawn(config: &StdioConfig) -> Result<Self, McpError> {
        let state = if let Some((confinement, profile)) = &config.confinement {
            if confinement.is_unavailable() {
                return Err(McpError::stable(
                    finstack_ai_runtime::CONFINEMENT_UNAVAILABLE,
                    "MCP stdio confinement was requested and is unavailable",
                ));
            }
            let mut command = std::process::Command::new(&config.program);
            command
                .args(&config.args)
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = confinement
                .spawn(command, profile)
                .map_err(|error| McpError::stable(error.code(), error.message().to_string()))?;
            let stdin = child.stdin.take().ok_or_else(|| {
                McpError::stable(MCP_TRANSPORT_ERROR, "stdio child stdin is unavailable")
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                McpError::stable(MCP_TRANSPORT_ERROR, "stdio child stdout is unavailable")
            })?;
            if let Some(stderr) = child.stderr.take() {
                drain_std_stderr(stderr);
            }
            StdioState::Confined {
                _child: child,
                stdin: tokio_file_from_stdin(stdin),
                stdout: BufReader::new(tokio_file_from_stdout(stdout)),
            }
        } else {
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
            StdioState::Unconfined {
                _child: child,
                stdin,
                stdout: BufReader::new(stdout),
            }
        };
        Ok(Self {
            state: tokio::sync::Mutex::new(state),
            next_id: AtomicU64::new(1),
            notifications: Mutex::new(Vec::new()),
            poisoned: AtomicBool::new(false),
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
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "stdio transport is poisoned",
            ));
        }
        let params = with_meta(params);
        let message = jsonrpc_request(&id, method, &params);
        let line = Self::encode_line(&message)?;
        let mut state = self.state.lock().await;
        write_stdio_line(&mut state, line.as_bytes()).await?;
        let mut dispatch = FrameDispatch::new(&id, &self.notifications);
        loop {
            let frame = match read_stdio_frame(&mut state).await {
                Ok(frame) => frame,
                Err(error) => {
                    self.poisoned.store(true, Ordering::SeqCst);
                    return Err(error);
                }
            };
            let value = match decode_frame(&frame) {
                Ok(value) => value,
                Err(error) => {
                    self.poisoned.store(true, Ordering::SeqCst);
                    return Err(error);
                }
            };
            match dispatch.push(&value) {
                Ok(Some(result)) => return Ok(result),
                Ok(None) => {}
                Err(error) => {
                    self.poisoned.store(true, Ordering::SeqCst);
                    return Err(error);
                }
            }
        }
    }
}

async fn write_stdio_line(state: &mut StdioState, line: &[u8]) -> Result<(), McpError> {
    match state {
        StdioState::Unconfined { stdin, .. } => {
            stdin
                .write_all(line)
                .await
                .map_err(|error| stdio_io(&error))?;
            stdin.flush().await.map_err(|error| stdio_io(&error))
        }
        StdioState::Confined { stdin, .. } => {
            stdin
                .write_all(line)
                .await
                .map_err(|error| stdio_io(&error))?;
            stdin.flush().await.map_err(|error| stdio_io(&error))
        }
    }
}

async fn read_stdio_frame(state: &mut StdioState) -> Result<String, McpError> {
    match state {
        StdioState::Unconfined { stdout, .. } => read_limited_line(stdout).await,
        StdioState::Confined { stdout, .. } => read_limited_line(stdout).await,
    }
}

async fn read_limited_line<R: AsyncBufReadExt + Unpin>(stdout: &mut R) -> Result<String, McpError> {
    let mut buf = Vec::new();
    let read = stdout
        .take(MAX_LINE_BYTES)
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|error| stdio_io(&error))?;
    if read == 0 {
        return Err(McpError::stable(
            MCP_TRANSPORT_ERROR,
            "stdio server closed stdout",
        ));
    }
    if !buf.ends_with(b"\n") {
        return Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "stdio line exceeds the configured byte limit",
        ));
    }
    String::from_utf8(buf)
        .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "stdio line is not valid utf-8"))
}

fn stdio_io(error: &std::io::Error) -> McpError {
    McpError::stable(MCP_TRANSPORT_ERROR, format!("stdio i/o failed: {error}"))
}

fn drain_std_stderr(stderr: std::process::ChildStderr) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(stderr);
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap_or(0) > 0 {
            line.clear();
        }
    });
}

fn tokio_file_from_stdin(stdin: std::process::ChildStdin) -> tokio::fs::File {
    tokio::fs::File::from_std(std_file_from_stdin(stdin))
}

fn tokio_file_from_stdout(stdout: std::process::ChildStdout) -> tokio::fs::File {
    tokio::fs::File::from_std(std_file_from_stdout(stdout))
}

#[allow(
    unsafe_code,
    reason = "stdio confinement wraps owned child pipes as tokio files"
)]
fn std_file_from_stdin(stdin: std::process::ChildStdin) -> std::fs::File {
    #[cfg(unix)]
    {
        use std::os::fd::{FromRawFd, IntoRawFd};
        // SAFETY: `ChildStdin` owns the fd; `into_raw_fd` transfers it.
        unsafe { std::fs::File::from_raw_fd(stdin.into_raw_fd()) }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        // SAFETY: `ChildStdin` owns the handle; `into_raw_handle` transfers it.
        unsafe { std::fs::File::from_raw_handle(stdin.into_raw_handle()) }
    }
}

#[allow(
    unsafe_code,
    reason = "stdio confinement wraps owned child pipes as tokio files"
)]
fn std_file_from_stdout(stdout: std::process::ChildStdout) -> std::fs::File {
    #[cfg(unix)]
    {
        use std::os::fd::{FromRawFd, IntoRawFd};
        // SAFETY: `ChildStdout` owns the fd; `into_raw_fd` transfers it.
        unsafe { std::fs::File::from_raw_fd(stdout.into_raw_fd()) }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{FromRawHandle, IntoRawHandle};
        // SAFETY: `ChildStdout` owns the handle; `into_raw_handle` transfers it.
        unsafe { std::fs::File::from_raw_handle(stdout.into_raw_handle()) }
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

    fn take_notifications(&self) -> Vec<String> {
        self.notifications
            .lock()
            .map(|mut queued| std::mem::take(&mut *queued))
            .unwrap_or_default()
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

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

/// Streamable HTTP transport. T4; not isolated.
pub(crate) struct HttpTransport {
    url: String,
    client: reqwest::Client,
    next_id: AtomicU64,
    notifications: Mutex<Vec<String>>,
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
            notifications: Mutex::new(Vec::new()),
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
        let text = read_bounded_body(response).await?;
        if content_type.starts_with("text/event-stream") {
            return parse_sse_jsonrpc(&text, &id, &self.notifications);
        }
        let value = decode_frame(&text)?;
        match FrameDispatch::new(&id, &self.notifications).push(&value)? {
            Some(result) => Ok(result),
            None => Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "http response contained only notifications",
            )),
        }
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

    fn take_notifications(&self) -> Vec<String> {
        self.notifications
            .lock()
            .map(|mut queued| std::mem::take(&mut *queued))
            .unwrap_or_default()
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

fn decode_frame(text: &str) -> Result<serde_json::Value, McpError> {
    serde_json::from_str(text.trim()).map_err(|error| {
        McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            format!("jsonrpc response is not json: {error}"),
        )
    })
}

fn interpret_result(value: &serde_json::Value) -> Result<serde_json::Value, McpError> {
    if let Some(error) = value.get("error") {
        let code = error
            .get("code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("jsonrpc error");
        return Err(
            McpError::stable(MCP_PROTOCOL_VIOLATION, format!("jsonrpc {code} {message}"))
                .with_jsonrpc(code),
        );
    }
    value.get("result").cloned().ok_or_else(|| {
        McpError::stable(MCP_PROTOCOL_VIOLATION, "jsonrpc response is missing result")
    })
}

fn jsonrpc_id(value: &serde_json::Value) -> Option<&serde_json::Value> {
    match value.get("id") {
        None | Some(serde_json::Value::Null) => None,
        Some(id) => Some(id),
    }
}

struct FrameDispatch<'a> {
    expected_id: &'a serde_json::Value,
    notifications: &'a Mutex<Vec<String>>,
    notification_count: usize,
}

impl<'a> FrameDispatch<'a> {
    fn new(expected_id: &'a serde_json::Value, notifications: &'a Mutex<Vec<String>>) -> Self {
        Self {
            expected_id,
            notifications,
            notification_count: 0,
        }
    }

    fn push(&mut self, value: &serde_json::Value) -> Result<Option<serde_json::Value>, McpError> {
        let method = value.get("method").and_then(serde_json::Value::as_str);
        match (jsonrpc_id(value), method) {
            (None, Some(method)) => {
                self.notification_count = self.notification_count.saturating_add(1);
                if self.notification_count > MAX_NOTIFICATIONS_PER_ROUND_TRIP {
                    return Err(McpError::stable(
                        MCP_PROTOCOL_VIOLATION,
                        "notification count exceeds the round-trip cap",
                    ));
                }
                if let Ok(mut queued) = self.notifications.lock() {
                    queued.push(method.to_owned());
                }
                Ok(None)
            }
            (Some(id), _) if id == self.expected_id => interpret_result(value).map(Some),
            _ => Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "jsonrpc frame id does not match the pending request",
            )),
        }
    }
}

fn parse_sse_jsonrpc(
    text: &str,
    expected_id: &serde_json::Value,
    notifications: &Mutex<Vec<String>>,
) -> Result<serde_json::Value, McpError> {
    let mut dispatch = FrameDispatch::new(expected_id, notifications);
    let mut saw_data = false;
    for event in text.split("\n\n") {
        let Some(data) = sse_event_data(event) else {
            continue;
        };
        saw_data = true;
        let value = decode_frame(&data)?;
        if let Some(result) = dispatch.push(&value)? {
            return Ok(result);
        }
    }
    if !saw_data {
        return Err(McpError::stable(
            MCP_TRANSPORT_ERROR,
            "sse stream contained no data event",
        ));
    }
    Err(McpError::stable(
        MCP_PROTOCOL_VIOLATION,
        "sse stream contained no matching response",
    ))
}

fn sse_event_data(event: &str) -> Option<String> {
    let mut data = String::new();
    for line in event.lines() {
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
    if data.is_empty() { None } else { Some(data) }
}

async fn read_bounded_body(response: reqwest::Response) -> Result<String, McpError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            McpError::stable(MCP_TRANSPORT_ERROR, format!("http body failed: {error}"))
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_HTTP_BODY_BYTES {
            return Err(McpError::stable(
                MCP_LIMIT_EXCEEDED,
                "http body exceeds the configured byte limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body)
        .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "http body is not valid utf-8"))
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
    fn notification_before_response_pairs_and_leaves_the_next_request_aligned() {
        let notifications = Mutex::new(Vec::new());
        let expected = serde_json::json!(1);
        let mut dispatch = FrameDispatch::new(&expected, &notifications);
        assert!(
            dispatch
                .push(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/tools/list_changed"
                }))
                .expect("notification")
                .is_none()
        );
        let result = dispatch
            .push(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"ok": true}
            }))
            .expect("result")
            .expect("paired");
        assert_eq!(result["ok"], serde_json::json!(true));
        assert_eq!(
            notifications.lock().expect("lock").as_slice(),
            ["notifications/tools/list_changed"]
        );
        let next = serde_json::json!(2);
        let mut next_dispatch = FrameDispatch::new(&next, &notifications);
        let next_result = next_dispatch
            .push(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {"ok": false}
            }))
            .expect("next")
            .expect("paired");
        assert_eq!(next_result["ok"], serde_json::json!(false));
    }

    #[test]
    fn foreign_id_is_a_protocol_violation() {
        let notifications = Mutex::new(Vec::new());
        let expected = serde_json::json!(1);
        let mut dispatch = FrameDispatch::new(&expected, &notifications);
        let error = dispatch
            .push(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 99,
                "result": {}
            }))
            .expect_err("foreign id");
        assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
    }

    #[tokio::test]
    async fn oversized_line_poisons_without_resync() {
        let oversized = usize::try_from(MAX_LINE_BYTES).expect("line cap") + 8;
        let mut reader = BufReader::new(std::io::Cursor::new(vec![b'x'; oversized]));
        let error = read_limited_line(&mut reader).await.expect_err("oversize");
        assert!(format!("{error}").contains(MCP_PROTOCOL_VIOLATION));
    }

    #[test]
    fn sse_events_are_dispatched_separately() {
        let notifications = Mutex::new(Vec::new());
        let expected = serde_json::json!(7);
        let text = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n",
            "\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"pong\":true}}\n",
            "\n"
        );
        let result = parse_sse_jsonrpc(text, &expected, &notifications).expect("sse");
        assert_eq!(result["pong"], serde_json::json!(true));
        assert_eq!(
            notifications.lock().expect("lock").as_slice(),
            ["notifications/tools/list_changed"]
        );
    }
}
