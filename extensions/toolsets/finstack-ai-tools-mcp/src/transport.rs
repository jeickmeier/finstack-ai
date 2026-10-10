//! MCP transports: scripted (tests), stdio, and streamable HTTP.

use std::collections::{BTreeSet, VecDeque};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use base64::Engine;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::protocol::{Meta, PROTOCOL_VERSION, jsonrpc_request};
use finstack_ai_net_guard::NetGuardError;
use finstack_ai_net_guard::{BodyReadInterrupt, read_body_bounded_interruptible};
use finstack_ai_runtime::ids::{Clock, SystemClock};
use finstack_ai_runtime::ports::model::RunCallContext;

use crate::{
    MCP_LIMIT_EXCEEDED, MCP_PROTOCOL_VIOLATION, MCP_SERVER_NOT_ALLOWLISTED, MCP_TIMEOUT,
    MCP_TRANSPORT_ERROR, MCP_TRANSPORT_POISONED, McpError,
};

const MAX_LINE_BYTES: u64 = 1024 * 1024;
const MAX_NOTIFICATIONS_PER_ROUND_TRIP: usize = 32;
/// Server-initiated JSON-RPC requests tolerated inside one round trip
/// before the stream is treated as a protocol violation.
const MAX_SERVER_REQUESTS_PER_ROUND_TRIP: usize = 32;
/// Queued notification methods retained across round trips until
/// [`McpTransport::take_notifications`] drains them; the oldest entry is
/// dropped once the queue is full.
const MAX_QUEUED_NOTIFICATIONS: usize = 256;
/// JSON-RPC `method not found`, returned to a server that sends the client
/// a request it does not implement.
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
/// Longest server-supplied method name echoed into a
/// `client/request_rejected:<method>` marker that reaches the host observer.
const MAX_REJECTED_METHOD_CHARS: usize = 128;
/// Prefix of the notification-queue marker recorded when a server-initiated
/// request was answered with `method not found`.
pub(crate) const REQUEST_REJECTED_PREFIX: &str = "client/request_rejected:";
/// Default HTTP response-body cap for an `HttpConfig` that does not set one
/// explicitly.
const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1_048_576;
/// Upper bound accepted by `HttpConfig::with_max_response_bytes`. A caller
/// wanting a larger cap has to opt in explicitly at construction; there is
/// no runtime override.
const MAX_RESPONSE_BYTES_CEILING: usize = 64 * 1_048_576;
const INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(30);
/// Default stdio response timeout for a `StdioConfig` that does not set one
/// explicitly. Bounds how long one round trip may wait on the server process
/// even when the caller supplied no deadline.
const DEFAULT_STDIO_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Cancellation and deadline source for one transport round trip.
///
/// A run-scoped request delegates to the runtime's
/// [`RunCallContext`] helpers (`is_interrupted`, `wait_deadline`,
/// `interrupted`) so the semantic deadline and the effect-local cancellation
/// signal are honoured exactly as every other port does. Construct-time
/// catalog enumeration has no run in scope and keeps a thin synthetic budget
/// of `INITIALIZATION_TIMEOUT` measured on the tokio clock; it cannot be
/// cancelled.
#[derive(Clone)]
pub(crate) enum RequestControl {
    Run(Box<RunCallContext>),
    Initialization { expires_at: tokio::time::Instant },
}

impl RequestControl {
    pub(crate) fn from_run(run: &RunCallContext) -> Self {
        Self::Run(Box::new(run.clone()))
    }

    fn initialization() -> Self {
        Self::Initialization {
            expires_at: tokio::time::Instant::now() + INITIALIZATION_TIMEOUT,
        }
    }

    /// Whether the request should stop now. A clock that cannot tell the
    /// time fails closed (treated as elapsed).
    fn is_interrupted(&self) -> bool {
        match self {
            Self::Run(run) => SystemClock
                .now()
                .map_or(true, |now| run.is_interrupted(now)),
            Self::Initialization { expires_at } => tokio::time::Instant::now() >= *expires_at,
        }
    }

    /// Resolve once the deadline elapses; pends forever without one.
    async fn wait_deadline(&self) {
        match self {
            Self::Run(run) => run.wait_deadline().await,
            Self::Initialization { expires_at } => tokio::time::sleep_until(*expires_at).await,
        }
    }

    /// Resolve when the request is cancelled or its deadline elapses.
    async fn interrupted(&self) {
        match self {
            Self::Run(run) => run.interrupted().await,
            Self::Initialization { expires_at } => tokio::time::sleep_until(*expires_at).await,
        }
    }

    /// Resolve on cancellation only; pends forever for the initialization
    /// budget, which has no cancellation signal.
    async fn cancelled(&self) {
        match self {
            Self::Run(run) => run.cancellation.cancelled().await,
            Self::Initialization { .. } => std::future::pending::<()>().await,
        }
    }
}

fn timeout_error() -> McpError {
    McpError::stable(
        MCP_TIMEOUT,
        "MCP request was cancelled or exceeded its deadline",
    )
}

fn poisoned_error() -> McpError {
    McpError::stable(
        MCP_TRANSPORT_POISONED,
        "stdio transport is poisoned and must be reconstructed",
    )
}

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
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        self.request_controlled(method, params, RequestControl::initialization())
    }

    fn request_controlled(
        &self,
        method: &str,
        params: serde_json::Value,
        control: RequestControl,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>>;

    fn request_identified_controlled(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
        control: RequestControl,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let _ = id;
        self.request_controlled(method, params, control)
    }

    /// Drain transport-delivered MCP notifications. Default is none.
    fn take_notifications(&self) -> Vec<String> {
        Vec::new()
    }

    /// Whether the transport has lost request/response alignment and can
    /// never answer again. A poisoned transport must be rebuilt from its
    /// configuration; see `McpToolset::reconstruct`. Default is never.
    fn is_poisoned(&self) -> bool {
        false
    }
}

/// Append one method name to a bounded notification queue, dropping the
/// oldest entry once `MAX_QUEUED_NOTIFICATIONS` is reached.
fn push_notification(queue: &Mutex<VecDeque<String>>, method: String) {
    if let Ok(mut queued) = queue.lock() {
        if queued.len() >= MAX_QUEUED_NOTIFICATIONS {
            queued.pop_front();
        }
        queued.push_back(method);
    }
}

fn drain_notifications(queue: &Mutex<VecDeque<String>>) -> Vec<String> {
    queue
        .lock()
        .map(|mut queued| queued.drain(..).collect())
        .unwrap_or_default()
}

#[cfg(test)]
fn optional_list_response(
    queue: &mut VecDeque<Result<serde_json::Value, McpError>>,
    items_key: &str,
) -> Result<serde_json::Value, McpError> {
    let queued = queue.front().and_then(|entry| entry.as_ref().ok());
    if queued.is_some_and(|value| value.get(items_key).is_some()) {
        return queue
            .pop_front()
            .unwrap_or_else(|| Ok(serde_json::json!({"resultType":"complete"})));
    }
    Ok(serde_json::json!({"resultType":"complete"}))
}

/// Merge required metadata into a params object.
pub(crate) fn with_meta(mut params: serde_json::Value) -> serde_json::Value {
    let meta = serde_json::to_value(Meta::default()).unwrap_or_else(|_| serde_json::json!({}));
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
    fn request_controlled(
        &self,
        method: &str,
        params: serde_json::Value,
        control: RequestControl,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let method = method.to_owned();
        Box::pin(async move {
            if control.is_interrupted() {
                return Err(timeout_error());
            }
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
        finstack_ai_runtime::confinement::ProcessConfinement,
        finstack_ai_runtime::confinement::ConfinementProfile,
    )>,
    read_timeout: Duration,
}

impl StdioConfig {
    /// Build a stdio target.
    ///
    /// The response timeout defaults to `DEFAULT_STDIO_READ_TIMEOUT`
    /// (30 seconds); call [`Self::with_read_timeout`] to raise or lower it.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, args: impl Into<Vec<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into(),
            confinement: None,
            read_timeout: DEFAULT_STDIO_READ_TIMEOUT,
        }
    }

    /// Override how long one round trip may wait for the server's response
    /// on stdout before the request fails with [`crate::MCP_TIMEOUT`] and the
    /// transport is poisoned. Zero is clamped to one millisecond; the
    /// transport never waits unbounded on an unresponsive server process.
    #[must_use]
    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout.max(Duration::from_millis(1));
        self
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
        confinement: finstack_ai_runtime::confinement::ProcessConfinement,
        profile: finstack_ai_runtime::confinement::ConfinementProfile,
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

enum StdioChild {
    Unconfined(Child),
    Confined(finstack_ai_runtime::confinement::ConfinedChild),
}

/// One spawned server process with its owned stdio pipes. The pipes are
/// type-erased so the framing code below is written once for both spawn
/// paths.
struct StdioState {
    child: StdioChild,
    stdin: Box<dyn AsyncWrite + Send + Unpin>,
    stdout: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
    /// Set once the child has been killed and reaped by the poison path, so
    /// `Drop` never signals a pid the platform may already have reused.
    terminated: bool,
}

/// Newline-framed stdio transport. T1; not a sandbox.
///
/// Dropping the transport synchronously terminates the server process tree
/// (the whole process group on Unix, the Job Object when confined) so a
/// server that forked helpers never outlives its client.
pub(crate) struct StdioTransport {
    state: tokio::sync::Mutex<StdioState>,
    next_id: AtomicU64,
    notifications: Mutex<VecDeque<String>>,
    poisoned: AtomicBool,
    read_timeout: Duration,
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if state.terminated {
            return;
        }
        match &mut state.child {
            StdioChild::Unconfined(child) => {
                if let Some(process_id) = child.id() {
                    let _ = finstack_ai_runtime::confinement::terminate_process_tree(process_id);
                }
                // `kill_on_drop` still covers the direct child on platforms
                // where the group signal is unavailable, and tokio reaps it.
                let _ = child.start_kill();
            }
            StdioChild::Confined(child) => {
                let _ = child.kill();
                // Non-blocking reap; a killed child that has not exited yet
                // is left for the platform. Drop never waits.
                let _ = child.try_wait();
            }
        }
    }
}

impl StdioTransport {
    pub(crate) fn try_spawn(config: &StdioConfig) -> Result<Self, McpError> {
        let state = if let Some((confinement, profile)) = &config.confinement {
            if confinement.is_unavailable() {
                return Err(McpError::stable(
                    finstack_ai_runtime::confinement::CONFINEMENT_UNAVAILABLE,
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
            if let Some(mut stderr) = child.stderr.take() {
                std::thread::spawn(move || {
                    use std::io::Read as _;
                    let mut chunk = [0_u8; 8_192];
                    while stderr.read(&mut chunk).unwrap_or(0) > 0 {}
                });
            }
            StdioState {
                terminated: false,
                child: StdioChild::Confined(child),
                stdin: Box::new(tokio::fs::File::from_std(stdin)),
                stdout: BufReader::new(Box::new(tokio::fs::File::from_std(stdout))),
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
            finstack_ai_runtime::confinement::configure_process_tree(command.as_std_mut())
                .map_err(|error| {
                    McpError::stable(error.code(), "MCP stdio process tree setup failed")
                })?;
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
                    let mut chunk = [0_u8; 8_192];
                    while stderr.read(&mut chunk).await.unwrap_or(0) > 0 {}
                });
            }
            StdioState {
                terminated: false,
                child: StdioChild::Unconfined(child),
                stdin: Box::new(stdin),
                stdout: BufReader::new(Box::new(stdout)),
            }
        };
        Ok(Self {
            state: tokio::sync::Mutex::new(state),
            next_id: AtomicU64::new(1),
            notifications: Mutex::new(VecDeque::new()),
            poisoned: AtomicBool::new(false),
            read_timeout: config.read_timeout,
        })
    }

    /// JSON-RPC error reply for a server-initiated request the client does
    /// not implement.
    fn method_not_supported_reply(id: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": JSONRPC_METHOD_NOT_FOUND,
                "message": "method not supported by client",
            },
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
        control: RequestControl,
    ) -> Result<serde_json::Value, McpError> {
        if control.is_interrupted() {
            return Err(timeout_error());
        }
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(poisoned_error());
        }
        let params = with_meta(params);
        let message = jsonrpc_request(&id, method, &params);
        let line = Self::encode_line(&message)?;
        let mut state = self.state.lock().await;
        let io = async {
            state
                .stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|error| stdio_io(&error))?;
            state
                .stdin
                .flush()
                .await
                .map_err(|error| stdio_io(&error))?;
            let mut dispatch = FrameDispatch::new(&id, &self.notifications);
            loop {
                let frame = read_limited_line(&mut state.stdout).await?;
                let value = decode_frame(&frame)?;
                match dispatch.push(&value)? {
                    Dispatched::Result(result) => return Ok(result),
                    Dispatched::Skip => {}
                    Dispatched::ServerRequest {
                        id: server_id,
                        method: server_method,
                    } => {
                        // A well-formed server-initiated request is answered
                        // inline with `method not found` while the state
                        // lock is still held, so the reply cannot interleave
                        // with another client request, and the read loop
                        // keeps waiting for the pending response.
                        let reply =
                            Self::encode_line(&Self::method_not_supported_reply(&server_id))?;
                        state
                            .stdin
                            .write_all(reply.as_bytes())
                            .await
                            .map_err(|error| stdio_io(&error))?;
                        state
                            .stdin
                            .flush()
                            .await
                            .map_err(|error| stdio_io(&error))?;
                        let bounded = server_method
                            .chars()
                            .take(MAX_REJECTED_METHOD_CHARS)
                            .collect::<String>();
                        push_notification(
                            &self.notifications,
                            format!("{REQUEST_REJECTED_PREFIX}{bounded}"),
                        );
                    }
                }
            }
        };
        // `biased` keeps runtime cancellation and the caller deadline ahead of
        // the transport's own read timeout, so cancelling a run still wins.
        let result = tokio::select! {
            biased;
            () = control.interrupted() => Err(timeout_error()),
            timed = tokio::time::timeout(self.read_timeout, io) => {
                timed.unwrap_or_else(|_elapsed| {
                    Err(McpError::stable(
                        MCP_TIMEOUT,
                        "stdio response exceeded the configured read timeout",
                    ))
                })
            }
        };
        // A JSON-RPC error *response* (tagged with a code by
        // `interpret_result`) arrived as a complete, correctly framed line,
        // so the request/response cadence is still aligned and the transport
        // stays usable — construct-time catalog enumeration relies on this to
        // tolerate -32601 from servers without prompts/resources. Every other
        // failure (framing, i/o, timeout) leaves the stream position unknown
        // and MUST poison.
        if let Err(error) = &result
            && error.jsonrpc_code().is_none()
        {
            self.poisoned.store(true, Ordering::SeqCst);
            terminate_stdio_state(&mut state).await;
        }
        result
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

async fn terminate_stdio_state(state: &mut StdioState) {
    if state.terminated {
        return;
    }
    state.terminated = true;
    let _ = state.stdin.shutdown().await;
    match &mut state.child {
        StdioChild::Unconfined(child) => {
            if let Some(process_id) = child.id() {
                let _ = finstack_ai_runtime::confinement::terminate_process_tree(process_id);
            }
            let _ = child.wait().await;
        }
        StdioChild::Confined(child) => {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl McpTransport for StdioTransport {
    fn request_controlled(
        &self,
        method: &str,
        params: serde_json::Value,
        control: RequestControl,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let id = serde_json::json!(self.next_id.fetch_add(1, Ordering::Relaxed));
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params, control).await })
    }

    fn request_identified_controlled(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
        control: RequestControl,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params, control).await })
    }

    fn take_notifications(&self) -> Vec<String> {
        drain_notifications(&self.notifications)
    }

    fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }
}

/// Allowlisted streamable-HTTP MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    url: String,
    max_response_bytes: usize,
}

impl HttpConfig {
    /// Construct a streamable-HTTP target.
    ///
    /// The response-body cap defaults to `DEFAULT_MAX_RESPONSE_BYTES`
    /// (8 MiB); call [`Self::with_max_response_bytes`] to raise or lower it.
    ///
    /// # Errors
    ///
    /// Rejects an empty URL, credentials, query, fragment, a non-HTTP
    /// scheme, or plaintext HTTP that is not a literal loopback IP.
    pub fn try_new(url: impl Into<String>) -> Result<Self, McpError> {
        let url = url.into();
        validate_http_url(&url)?;
        Ok(Self {
            url,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        })
    }

    /// Override the HTTP response-body cap.
    ///
    /// # Errors
    ///
    /// Rejects zero (fail closed, never an unbounded read) and anything
    /// above `MAX_RESPONSE_BYTES_CEILING` (64 MiB).
    pub fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Result<Self, McpError> {
        if max_response_bytes == 0 {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "http max_response_bytes must not be zero",
            ));
        }
        if max_response_bytes > MAX_RESPONSE_BYTES_CEILING {
            return Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "http max_response_bytes exceeds the 64 MiB ceiling",
            ));
        }
        self.max_response_bytes = max_response_bytes;
        Ok(self)
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

/// Streamable HTTP transport. T4; not isolated.
pub(crate) struct HttpTransport {
    url: String,
    client: reqwest::Client,
    max_response_bytes: usize,
    next_id: AtomicU64,
    notifications: Mutex<VecDeque<String>>,
}

impl HttpTransport {
    pub(crate) fn try_new(config: &HttpConfig) -> Result<Self, McpError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            // MCP servers are exact-URL allowlisted (see `authorize_http`):
            // silently following a redirect would let an allowlisted URL
            // hand the request to a host/scheme the allowlist never vetted
            // (the same credential-drift concern that motivated the
            // no-redirect policy on the S3 object-store client). Unlike
            // `finstack-ai-net-guard`'s pinned client this transport does
            // NOT deny private/loopback destinations and does NOT disable
            // env/system proxies: MCP servers are host-allowlisted by
            // construction (not caller-supplied like a media `audio_url`),
            // and legitimate deployments run MCP servers on localhost or
            // a private network, or reach them only through a configured
            // egress proxy. Adding a private-IP deny or `.no_proxy()` here
            // would break those deployments for no SSRF benefit, since the
            // destination was already vetted at allowlist time, not at
            // request time.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                McpError::stable(MCP_TRANSPORT_ERROR, format!("http client failed: {error}"))
            })?;
        Ok(Self {
            url: config.url.clone(),
            client,
            max_response_bytes: config.max_response_bytes,
            next_id: AtomicU64::new(1),
            notifications: Mutex::new(VecDeque::new()),
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
            HeaderValue::from_str(method).unwrap_or_else(|_| HeaderValue::from_static("unknown")),
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
        control: RequestControl,
    ) -> Result<serde_json::Value, McpError> {
        if control.is_interrupted() {
            return Err(timeout_error());
        }
        let params = with_meta(params);
        let name = params.get("name").and_then(serde_json::Value::as_str);
        let body = jsonrpc_request(&id, method, &params);
        let send = self
            .client
            .post(&self.url)
            .headers(Self::headers_for(method, name))
            .json(&body)
            .send();
        let response = tokio::select! {
            biased;
            () = control.interrupted() => return Err(timeout_error()),
            result = send => result.map_err(|error| {
                McpError::stable(MCP_TRANSPORT_ERROR, format!("http post failed: {error}"))
            })?,
        };
        let is_event_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if is_event_stream {
            return read_sse_until_response(
                response,
                self.max_response_bytes,
                &control,
                &id,
                &self.notifications,
            )
            .await;
        }
        let text = read_bounded_body(response, self.max_response_bytes, &control).await?;
        let value = decode_frame(&text)?;
        match FrameDispatch::new(&id, &self.notifications).push(&value)? {
            Dispatched::Result(result) => Ok(result),
            Dispatched::Skip | Dispatched::ServerRequest { .. } => Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "http response did not contain the pending result",
            )),
        }
    }
}

impl McpTransport for HttpTransport {
    fn request_controlled(
        &self,
        method: &str,
        params: serde_json::Value,
        control: RequestControl,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let id = serde_json::json!(self.next_id.fetch_add(1, Ordering::Relaxed));
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params, control).await })
    }

    fn request_identified_controlled(
        &self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
        control: RequestControl,
    ) -> BoxFuture<'_, Result<serde_json::Value, McpError>> {
        let method = method.to_owned();
        Box::pin(async move { self.round_trip(id, &method, params, control).await })
    }

    fn take_notifications(&self) -> Vec<String> {
        drain_notifications(&self.notifications)
    }
}

/// Non-ASCII or unsafe header values use the sentinel encoding
/// `=?base64?{Base64EncodedValue}?=` required by the spec.
fn encode_header_value(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| {
        let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
        HeaderValue::from_str(&format!("=?base64?{encoded}?="))
            .unwrap_or_else(|_| HeaderValue::from_static("=?base64?e30=?="))
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

/// Outcome of classifying one inbound JSON-RPC frame against the pending
/// request id.
#[derive(Debug)]
enum Dispatched {
    /// The response to the pending request (`result` member).
    Result(serde_json::Value),
    /// A well-formed server-initiated request (`id` and `method`, foreign
    /// id). The transport decides how to answer it; the frame never poisons.
    ServerRequest {
        id: serde_json::Value,
        method: String,
    },
    /// A notification that was queued; keep reading.
    Skip,
}

struct FrameDispatch<'a> {
    expected_id: &'a serde_json::Value,
    notifications: &'a Mutex<VecDeque<String>>,
    notification_count: usize,
    server_request_count: usize,
}

impl<'a> FrameDispatch<'a> {
    fn new(expected_id: &'a serde_json::Value, notifications: &'a Mutex<VecDeque<String>>) -> Self {
        Self {
            expected_id,
            notifications,
            notification_count: 0,
            server_request_count: 0,
        }
    }

    /// Classify one frame. A frame carrying the pending id is always the
    /// response; a frame with a foreign id is a server request when it names
    /// a method and a protocol violation otherwise. Both per-round-trip caps
    /// (notifications, server requests) are protocol violations when crossed.
    fn push(&mut self, value: &serde_json::Value) -> Result<Dispatched, McpError> {
        let method = value.get("method").and_then(serde_json::Value::as_str);
        match (jsonrpc_id(value), method) {
            (Some(id), _) if id == self.expected_id => {
                interpret_result(value).map(Dispatched::Result)
            }
            (None, Some(method)) => {
                self.notification_count = self.notification_count.saturating_add(1);
                if self.notification_count > MAX_NOTIFICATIONS_PER_ROUND_TRIP {
                    return Err(McpError::stable(
                        MCP_PROTOCOL_VIOLATION,
                        "notification count exceeds the round-trip cap",
                    ));
                }
                push_notification(self.notifications, method.to_owned());
                Ok(Dispatched::Skip)
            }
            (Some(id), Some(method)) => {
                self.server_request_count = self.server_request_count.saturating_add(1);
                if self.server_request_count > MAX_SERVER_REQUESTS_PER_ROUND_TRIP {
                    return Err(McpError::stable(
                        MCP_PROTOCOL_VIOLATION,
                        "server request count exceeds the round-trip cap",
                    ));
                }
                Ok(Dispatched::ServerRequest {
                    id: id.clone(),
                    method: method.to_owned(),
                })
            }
            _ => Err(McpError::stable(
                MCP_PROTOCOL_VIOLATION,
                "jsonrpc frame id does not match the pending request",
            )),
        }
    }
}

/// Incremental SSE decoder that dispatches each complete event's `data`
/// payload as one JSON-RPC frame and stops at the pending response.
///
/// Events end at a blank line (`\n\n` or `\r\n\r\n`); a trailing partial
/// event is flushed by [`Self::finish`] at end of stream. Server-initiated
/// requests are skipped (HTTP cannot answer them inline) and notifications
/// are queued.
struct SseFrameReader<'a> {
    buffer: Vec<u8>,
    /// Bytes of `buffer` already known to hold no event boundary, so each
    /// chunk is scanned once rather than rescanning the whole buffer.
    scanned: usize,
    dispatch: FrameDispatch<'a>,
    saw_data: bool,
}

impl<'a> SseFrameReader<'a> {
    fn new(expected_id: &'a serde_json::Value, notifications: &'a Mutex<VecDeque<String>>) -> Self {
        Self {
            buffer: Vec::new(),
            scanned: 0,
            dispatch: FrameDispatch::new(expected_id, notifications),
            saw_data: false,
        }
    }

    /// Append one chunk and dispatch every event it completes.
    ///
    /// Returns the pending response as soon as its frame is parsed; the
    /// remainder of the stream is then irrelevant and the caller may drop it.
    fn feed(&mut self, chunk: &[u8]) -> Result<Option<serde_json::Value>, McpError> {
        self.buffer.extend_from_slice(chunk);
        // A separator may straddle the previous chunk end; back up by one
        // less than the longest separator so it is still found.
        let mut from = self.scanned.saturating_sub(3);
        while let Some((end, separator_len)) = sse_event_boundary(&self.buffer, from) {
            let event = self
                .buffer
                .drain(..end.saturating_add(separator_len))
                .collect::<Vec<u8>>();
            from = 0;
            if let Some(result) = self.dispatch_event(event.get(..end).unwrap_or(&[]))? {
                return Ok(Some(result));
            }
        }
        self.scanned = self.buffer.len();
        Ok(None)
    }

    /// Flush the trailing partial event at end of stream and report why no
    /// response was found.
    fn finish(mut self) -> Result<serde_json::Value, McpError> {
        let trailing = std::mem::take(&mut self.buffer);
        if let Some(result) = self.dispatch_event(&trailing)? {
            return Ok(result);
        }
        if !self.saw_data {
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

    fn dispatch_event(&mut self, event: &[u8]) -> Result<Option<serde_json::Value>, McpError> {
        let text = std::str::from_utf8(event).map_err(|_| {
            McpError::stable(MCP_PROTOCOL_VIOLATION, "http body is not valid utf-8")
        })?;
        let Some(data) = sse_event_data(text) else {
            return Ok(None);
        };
        self.saw_data = true;
        let value = decode_frame(&data)?;
        match self.dispatch.push(&value)? {
            Dispatched::Result(result) => Ok(Some(result)),
            Dispatched::Skip | Dispatched::ServerRequest { .. } => Ok(None),
        }
    }
}

/// Byte offset and separator length of the first complete SSE event in
/// `buffer`, searching from `from` onwards.
fn sse_event_boundary(buffer: &[u8], from: usize) -> Option<(usize, usize)> {
    let tail = buffer.get(from..).unwrap_or(&[]);
    let lf = find_subslice(tail, b"\n\n").map(|at| (from.saturating_add(at), 2));
    let crlf = find_subslice(tail, b"\r\n\r\n").map(|at| (from.saturating_add(at), 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parse a fully buffered SSE body. Test helper over [`SseFrameReader`].
#[cfg(test)]
fn parse_sse_jsonrpc(
    text: &str,
    expected_id: &serde_json::Value,
    notifications: &Mutex<VecDeque<String>>,
) -> Result<serde_json::Value, McpError> {
    let mut reader = SseFrameReader::new(expected_id, notifications);
    if let Some(result) = reader.feed(text.as_bytes())? {
        return Ok(result);
    }
    reader.finish()
}

/// Stream an SSE response under `cap` bytes and the request's cancellation
/// and deadline, returning as soon as the pending response is parsed.
///
/// A server that keeps the stream open after answering (the usual
/// streamable-HTTP behaviour) therefore never stalls the round trip; the
/// stream is dropped and the connection closed on return. Exceeding the cap
/// is [`MCP_LIMIT_EXCEEDED`], never a truncation. Interrupt semantics match
/// `finstack_ai_net_guard::read_body_bounded_interruptible`, which this path
/// cannot reuse because it only yields the body at end of stream.
async fn read_sse_until_response(
    response: reqwest::Response,
    cap: usize,
    control: &RequestControl,
    expected_id: &serde_json::Value,
    notifications: &Mutex<VecDeque<String>>,
) -> Result<serde_json::Value, McpError> {
    let mut reader = SseFrameReader::new(expected_id, notifications);
    let mut stream = response.bytes_stream();
    let mut received = 0_usize;
    loop {
        let next = tokio::select! {
            biased;
            () = control.cancelled() => return Err(timeout_error()),
            () = control.wait_deadline() => return Err(timeout_error()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = next else {
            return reader.finish();
        };
        let chunk = chunk.map_err(|_| map_net_guard_error(NetGuardError::TransportFailed))?;
        received = received.saturating_add(chunk.len());
        if received > cap {
            return Err(map_net_guard_error(NetGuardError::LimitExceeded));
        }
        if let Some(result) = reader.feed(&chunk)? {
            return Ok(result);
        }
    }
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

async fn read_bounded_body(
    response: reqwest::Response,
    cap: usize,
    control: &RequestControl,
) -> Result<String, McpError> {
    let body = read_body_bounded_interruptible(
        response,
        cap,
        BodyReadInterrupt::new(control.cancelled(), control.wait_deadline()),
    )
    .await
    .map_err(map_net_guard_error)?;
    String::from_utf8(body)
        .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "http body is not valid utf-8"))
}

fn map_net_guard_error(error: NetGuardError) -> McpError {
    match error {
        NetGuardError::LimitExceeded => McpError::stable(
            MCP_LIMIT_EXCEEDED,
            "http body exceeds the configured byte limit",
        ),
        NetGuardError::Cancelled | NetGuardError::DeadlineExceeded => timeout_error(),
        other => McpError::stable(MCP_TRANSPORT_ERROR, format!("http body failed: {other}")),
    }
}

pub(crate) fn authorize_stdio(
    allowed: &BTreeSet<Arc<str>>,
    program: &Path,
) -> Result<(), McpError> {
    if allowed.contains(program.to_string_lossy().as_ref()) {
        return Ok(());
    }
    if let Some(name) = program.file_name().and_then(|name| name.to_str())
        && allowed.contains(name)
    {
        return Ok(());
    }
    Err(McpError::stable(
        MCP_SERVER_NOT_ALLOWLISTED,
        "stdio program is not allowlisted",
    ))
}

pub(crate) fn validate_http_url(value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "http url is empty",
        ));
    }
    let url = Url::parse(value)
        .map_err(|_| McpError::stable(MCP_PROTOCOL_VIOLATION, "http url is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "http url contains forbidden components",
        ));
    }
    if url.scheme() == "http"
        && !url
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|address| address.is_loopback())
    {
        return Err(McpError::stable(
            MCP_PROTOCOL_VIOLATION,
            "plaintext http url must use a loopback IP",
        ));
    }
    Ok(())
}

pub(crate) fn authorize_http(allowed: &BTreeSet<Arc<str>>, url: &str) -> Result<(), McpError> {
    if allowed.contains(url) {
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

    fn expect_result(dispatched: Dispatched) -> serde_json::Value {
        match dispatched {
            Dispatched::Result(value) => value,
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[test]
    fn notification_before_response_pairs_and_leaves_the_next_request_aligned() {
        let notifications = Mutex::new(VecDeque::new());
        let expected = serde_json::json!(1);
        let mut dispatch = FrameDispatch::new(&expected, &notifications);
        assert!(matches!(
            dispatch
                .push(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/tools/list_changed"
                }))
                .expect("notification"),
            Dispatched::Skip
        ));
        let result = expect_result(
            dispatch
                .push(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {"ok": true}
                }))
                .expect("result"),
        );
        assert_eq!(result["ok"], serde_json::json!(true));
        assert_eq!(
            drain_notifications(&notifications),
            ["notifications/tools/list_changed"]
        );
        let next = serde_json::json!(2);
        let mut next_dispatch = FrameDispatch::new(&next, &notifications);
        let next_result = expect_result(
            next_dispatch
                .push(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {"ok": false}
                }))
                .expect("next"),
        );
        assert_eq!(next_result["ok"], serde_json::json!(false));
    }

    #[test]
    fn foreign_id_request_is_dispatched_as_a_server_request() {
        let notifications = Mutex::new(VecDeque::new());
        let expected = serde_json::json!(1);
        let mut dispatch = FrameDispatch::new(&expected, &notifications);
        let dispatched = dispatch
            .push(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": "srv-1",
                "method": "sampling/createMessage",
                "params": {}
            }))
            .expect("a well-formed server request never fails dispatch");
        match dispatched {
            Dispatched::ServerRequest { id, method } => {
                assert_eq!(id, serde_json::json!("srv-1"));
                assert_eq!(method, "sampling/createMessage");
            }
            other => panic!("expected a server request, got {other:?}"),
        }
    }

    #[test]
    fn server_requests_are_capped_per_round_trip() {
        let notifications = Mutex::new(VecDeque::new());
        let expected = serde_json::json!(1);
        let mut dispatch = FrameDispatch::new(&expected, &notifications);
        let request = serde_json::json!({"jsonrpc": "2.0", "id": "srv", "method": "ping"});
        for _ in 0..MAX_SERVER_REQUESTS_PER_ROUND_TRIP {
            dispatch.push(&request).expect("under the cap");
        }
        let error = dispatch.push(&request).expect_err("over the cap");
        assert_eq!(error.code(), MCP_PROTOCOL_VIOLATION);
    }

    #[test]
    fn notification_queue_is_capped() {
        let notifications = Mutex::new(VecDeque::new());
        for index in 0..(MAX_QUEUED_NOTIFICATIONS + 8) {
            push_notification(&notifications, format!("notifications/n{index}"));
        }
        let drained = drain_notifications(&notifications);
        assert_eq!(drained.len(), MAX_QUEUED_NOTIFICATIONS);
        assert_eq!(drained[0], "notifications/n8", "oldest entries are dropped");
        assert_eq!(
            drained[MAX_QUEUED_NOTIFICATIONS - 1],
            format!("notifications/n{}", MAX_QUEUED_NOTIFICATIONS + 7)
        );
    }

    #[test]
    fn foreign_id_is_a_protocol_violation() {
        let notifications = Mutex::new(VecDeque::new());
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

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_read_timeout_fails_the_request_and_poisons_the_transport() {
        let config = StdioConfig::new("/bin/sleep", vec!["30".to_owned()])
            .with_read_timeout(Duration::from_millis(50));
        let transport = StdioTransport::try_spawn(&config).expect("spawns");
        let error = transport
            .request("tools/list", serde_json::json!({}))
            .await
            .expect_err("a server that never responds must time out");
        assert_eq!(error.code(), MCP_TIMEOUT);
        assert!(format!("{error}").contains("read timeout"));
        let poisoned = transport
            .request("tools/list", serde_json::json!({}))
            .await
            .expect_err("a timed-out transport must stay poisoned");
        assert!(format!("{poisoned}").contains("poisoned"));
    }

    /// Regression: a stdio server without `prompts/list` (a clean -32601
    /// error response) must not poison the transport — construction
    /// tolerates the error via `optional_catalog_missing`, and later
    /// `tools/call` round trips must still work.
    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_jsonrpc_error_response_does_not_poison_the_transport() {
        let script = concat!(
            "read -r _request\n",
            "printf '{\"jsonrpc\":\"2.0\",\"id\":1,",
            "\"error\":{\"code\":-32601,\"message\":\"Method not found\"}}\\n'\n",
            "read -r _request\n",
            "printf '{\"jsonrpc\":\"2.0\",\"id\":2,",
            "\"result\":{\"resultType\":\"complete\",\"tools\":[]}}\\n'\n",
        );
        let config = StdioConfig::new("/bin/sh", vec!["-c".to_owned(), script.to_owned()]);
        let transport = StdioTransport::try_spawn(&config).expect("spawns");
        let error = transport
            .request("prompts/list", serde_json::json!({}))
            .await
            .expect_err("the server rejects prompts/list");
        assert_eq!(error.jsonrpc_code(), Some(-32601));
        let value = transport
            .request("tools/list", serde_json::json!({}))
            .await
            .expect("a clean jsonrpc error response must not poison the transport");
        assert_eq!(value["resultType"], "complete");
    }

    #[test]
    fn stdio_read_timeout_clamps_zero() {
        let config =
            StdioConfig::new("/bin/echo", Vec::<String>::new()).with_read_timeout(Duration::ZERO);
        assert_eq!(config.read_timeout, Duration::from_millis(1));
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
        let notifications = Mutex::new(VecDeque::new());
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
            drain_notifications(&notifications),
            ["notifications/tools/list_changed"]
        );
    }

    #[test]
    fn sse_crlf_events_are_dispatched_separately() {
        let notifications = Mutex::new(VecDeque::new());
        let expected = serde_json::json!(7);
        let text = concat!(
            "event: message\r\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\r\n",
            "\r\n",
            "event: message\r\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"pong\":true}}\r\n",
            "\r\n"
        );
        let result = parse_sse_jsonrpc(text, &expected, &notifications).expect("sse");
        assert_eq!(result["pong"], serde_json::json!(true));
        assert_eq!(
            drain_notifications(&notifications),
            ["notifications/tools/list_changed"]
        );
    }

    /// Feeding the stream one byte at a time splits every separator across
    /// chunks; the reader must still find each event exactly once.
    #[test]
    fn sse_reader_finds_boundaries_split_across_chunks() {
        let notifications = Mutex::new(VecDeque::new());
        let expected = serde_json::json!(3);
        let text = concat!(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/prompts/list_changed\"}\r\n",
            "\r\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n",
            "\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"done\":true}}\n",
            "\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/never_reached\"}\n\n"
        );
        let mut reader = SseFrameReader::new(&expected, &notifications);
        let mut result = None;
        for byte in text.as_bytes() {
            if let Some(value) = reader.feed(std::slice::from_ref(byte)).expect("feed") {
                result = Some(value);
                break;
            }
        }
        assert_eq!(result.expect("matched")["done"], serde_json::json!(true));
        assert_eq!(
            drain_notifications(&notifications),
            [
                "notifications/prompts/list_changed",
                "notifications/tools/list_changed"
            ]
        );
    }

    /// Write one raw HTTP/1.1 response to the first connection accepted on
    /// `listener`, then close it.
    async fn serve_once(
        listener: tokio::net::TcpListener,
        status: u16,
        headers: String,
        body: Vec<u8>,
    ) {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0_u8; 8_192];
        let _ = stream.read(&mut buf).await.expect("read request");
        let mut response = format!(
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        stream.write_all(&response).await.expect("write response");
        stream.shutdown().await.expect("shutdown");
    }

    #[tokio::test]
    async fn over_cap_response_body_is_rejected() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let body = vec![b'a'; 64];
        let server = tokio::spawn(serve_once(
            listener,
            200,
            "Content-Type: application/json\r\n".to_owned(),
            body,
        ));

        let config = HttpConfig::try_new(format!("http://{addr}"))
            .expect("config")
            .with_max_response_bytes(16)
            .expect("cap is within range");
        let transport = HttpTransport::try_new(&config).expect("transport");
        let error = transport
            .request("tools/list", serde_json::json!({}))
            .await
            .expect_err("body over the configured cap must be rejected, not truncated");
        assert_eq!(error.code(), MCP_LIMIT_EXCEEDED);
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn redirect_response_is_not_followed() {
        // A second listener plays the redirect target. If the transport
        // transparently followed the 302, it would connect here.
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind target");
        let target_addr = target_listener.local_addr().expect("addr");
        let (target_tx, mut target_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let target_server = tokio::spawn(async move {
            if target_listener.accept().await.is_ok() {
                let _ = target_tx.send(());
            }
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let location = format!("http://{target_addr}/moved");
        let server = tokio::spawn(serve_once(
            listener,
            302,
            format!("Location: {location}\r\n"),
            Vec::new(),
        ));

        let config = HttpConfig::try_new(format!("http://{addr}")).expect("config");
        let transport = HttpTransport::try_new(&config).expect("transport");
        transport
            .request("tools/list", serde_json::json!({}))
            .await
            .expect_err("a 302 with an empty body is not a valid jsonrpc frame");
        server.await.expect("server task");

        let followed =
            tokio::time::timeout(std::time::Duration::from_millis(200), target_rx.recv())
                .await
                .is_ok();
        assert!(!followed, "the 302 redirect must not be followed");
        target_server.abort();
    }

    /// The server answers, then keeps the SSE stream open. The round trip
    /// must return as soon as the matching response frame is parsed instead
    /// of waiting for end of stream.
    #[tokio::test]
    async fn sse_read_stops_at_matching_response() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0_u8; 8_192];
            let _ = stream.read(&mut buf).await.expect("read request");
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            stream.write_all(head.as_bytes()).await.expect("head");
            let events = concat!(
                "event: message\n",
                "data: {\"jsonrpc\":\"2.0\",\"id\":\"srv\",\"method\":\"roots/list\"}\n",
                "\n",
                "event: message\n",
                "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n",
                "\n",
                "event: message\n",
                "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"resultType\":\"complete\",\"tools\":[]}}\n",
                "\n",
            );
            stream.write_all(events.as_bytes()).await.expect("events");
            stream.flush().await.expect("flush");
            // Hold the stream open well past the assertion window.
            tokio::time::sleep(Duration::from_secs(10)).await;
            let _ = stream.shutdown().await;
        });

        let config = HttpConfig::try_new(format!("http://{addr}")).expect("config");
        let transport = HttpTransport::try_new(&config).expect("transport");
        let value = tokio::time::timeout(
            Duration::from_secs(2),
            transport.request("tools/list", serde_json::json!({})),
        )
        .await
        .expect("returns before the server closes the stream")
        .expect("matching response");
        assert_eq!(value["resultType"], "complete");
        assert_eq!(
            transport.take_notifications(),
            ["notifications/tools/list_changed"],
            "server requests are skipped on http; notifications are queued"
        );
        server.abort();
    }

    /// Process state for `pid` as reported by `ps`, or `None` once the
    /// process is gone or has been reaped to a zombie.
    #[cfg(unix)]
    fn live_process_state(pid: &str) -> Option<String> {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .ok()?;
        let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if state.is_empty() || state.starts_with('Z') {
            None
        } else {
            Some(state)
        }
    }

    /// Dropping the transport must take the whole process group with it:
    /// the server forks a `sleep` grandchild whose pid it reports on stdout.
    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_drop_terminates_process_group() {
        let config = StdioConfig::new(
            "/bin/sh",
            vec!["-c".to_owned(), "sleep 60 & echo $!; wait".to_owned()],
        );
        let transport = StdioTransport::try_spawn(&config).expect("spawns");
        let grandchild = {
            let mut state = transport.state.lock().await;
            read_limited_line(&mut state.stdout)
                .await
                .expect("the server reports the grandchild pid")
                .trim()
                .to_owned()
        };
        assert!(
            live_process_state(&grandchild).is_some(),
            "the grandchild is alive before drop"
        );
        drop(transport);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while live_process_state(&grandchild).is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "grandchild {grandchild} survived the transport drop"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn plaintext_http_requires_a_literal_loopback_ip() {
        for url in [
            "http://example.test",
            "http://192.0.2.1",
            "http://localhost",
            "https://user:pass@example.test",
            "https://example.test?token=nope",
            "https://example.test/#fragment",
            "",
        ] {
            assert_eq!(
                HttpConfig::try_new(url).expect_err("rejected").code(),
                MCP_PROTOCOL_VIOLATION
            );
        }
        for url in [
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
            "https://example.test",
        ] {
            assert!(HttpConfig::try_new(url).is_ok());
        }
    }
}
