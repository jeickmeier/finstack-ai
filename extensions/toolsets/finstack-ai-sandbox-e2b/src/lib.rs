//! T4 E2B remote-sandbox Toolset.
//!
//! Construction requires an explicit API key and never reads environment
//! variables. Non-loopback endpoints must be HTTPS. This leaf is not Landlock
//! and is not isolated. Shell stays T1.

#![warn(missing_docs)]

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ErrorCategory, Metadata, PortFuture, RawJson,
    RetrySafety, SideEffectClass, Timestamp, ToolCallContext, ToolDeferralSupport, ToolError,
    ToolEventStream, ToolExecutionMode, ToolId, ToolResult, ToolSpec, ToolStreamItem, Toolset,
    ToolsetDescriptor, ValidatedToolCall, verify_authority,
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use thiserror::Error;

const TOOL_ID: &str = "finstack.tools.e2b_run";
const TOOL_NAME: &str = "e2b_run";
const DEFAULT_ENDPOINT: &str = "https://api.e2b.dev";
const DEFAULT_TEMPLATE: &str = "base";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESULT_BYTES: usize = 64 * 1_024;

/// Stable missing-credential code.
pub const E2B_CREDENTIAL_REQUIRED: &str = "e2b_credential_required";
/// Stable endpoint-configuration code.
pub const E2B_ENDPOINT_INVALID: &str = "e2b_endpoint_invalid";
/// Stable argument-validation code.
pub const E2B_INVALID_ARGUMENTS: &str = "e2b_invalid_arguments";
/// Stable remote-transport code.
pub const E2B_TRANSPORT_FAILED: &str = "e2b_transport_failed";
/// Stable output-limit code.
pub const E2B_LIMIT_EXCEEDED: &str = "e2b_limit_exceeded";
/// Stable cancellation/deadline code.
pub const E2B_TIMEOUT: &str = "e2b_timeout";

/// Explicit E2B route. Never populated from the environment.
#[derive(Clone)]
pub struct E2bSandboxConfig {
    /// Explicit API key. Empty values fail closed.
    pub api_key: String,
    /// HTTPS product endpoint, or loopback HTTP for scripted fixtures.
    pub endpoint: String,
    /// Optional sandbox template. Defaults to `base`.
    pub template: Option<String>,
}

impl std::fmt::Debug for E2bSandboxConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2bSandboxConfig")
            .field("api_key", &"[redacted]")
            .field("endpoint", &self.endpoint)
            .field("template", &self.template)
            .finish()
    }
}

/// Construction or route failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum E2bSandboxError {
    /// API key was omitted.
    #[error("{E2B_CREDENTIAL_REQUIRED}: e2b sandbox construction requires an explicit API key")]
    CredentialRequired,
    /// Endpoint scheme, host, or components are invalid.
    #[error("{E2B_ENDPOINT_INVALID}: {reason}")]
    EndpointInvalid {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// T4 remote E2B Toolset.
pub struct E2bSandboxToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    tool_id: ToolId,
    api_key: String,
    endpoint: String,
    template: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for E2bSandboxToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2bSandboxToolset")
            .field("endpoint", &self.endpoint)
            .field("template", &self.template)
            .finish_non_exhaustive()
    }
}

impl E2bSandboxToolset {
    /// Construct the Toolset after validating the explicit route.
    ///
    /// # Errors
    ///
    /// Returns [`E2bSandboxError::CredentialRequired`] when `api_key` is empty.
    /// Returns [`E2bSandboxError::EndpointInvalid`] for a non-HTTP URL, userinfo,
    /// query, fragment, or plaintext HTTP off loopback.
    pub fn try_new(config: E2bSandboxConfig) -> Result<Self, E2bSandboxError> {
        if config.api_key.is_empty() {
            return Err(E2bSandboxError::CredentialRequired);
        }
        let endpoint = if config.endpoint.is_empty() {
            DEFAULT_ENDPOINT.to_owned()
        } else {
            config.endpoint
        };
        validate_endpoint(&endpoint)?;
        let template = config
            .template
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_TEMPLATE.to_owned());
        let tool_id = ToolId::parse(TOOL_ID).map_err(|_| E2bSandboxError::EndpointInvalid {
            reason: "invalid_tool_id",
        })?;
        let input_schema = RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"command":{"minLength":1,"type":"string"}},"required":["command"],"type":"object"}"#,
        )
        .map_err(|_| E2bSandboxError::EndpointInvalid {
            reason: "invalid_input_schema",
        })?;
        let output_schema = RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"exit_code":{"type":"integer"},"sandbox_id":{"type":"string"},"stdout":{"type":"string"}},"required":["exit_code","sandbox_id","stdout"],"type":"object"}"#,
        )
        .map_err(|_| E2bSandboxError::EndpointInvalid {
            reason: "invalid_output_schema",
        })?;
        let spec = ToolSpec {
            id: tool_id.clone(),
            model_name: Arc::from(TOOL_NAME),
            title: Arc::from("E2B run"),
            description: Arc::from("Start one E2B sandbox and run a single command."),
            input_schema,
            output_schema: Some(output_schema),
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::NonIdempotentWrite,
            retry_safety: RetrySafety::AtMostOnce,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::Policy,
                reason: Some(Arc::from("T4 remote E2B sandbox")),
                attributes: Metadata::empty(),
            },
            max_result_bytes: u64::try_from(MAX_RESULT_BYTES).unwrap_or(u64::MAX),
            metadata: Metadata::empty(),
            deferral: ToolDeferralSupport::Never,
        };
        spec.validate()
            .map_err(|_| E2bSandboxError::EndpointInvalid {
                reason: "invalid_tool_spec",
            })?;
        let client = reqwest::Client::builder()
            .http1_only()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| E2bSandboxError::EndpointInvalid {
                reason: "http_client",
            })?;
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-e2b"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([spec]),
            tool_id,
            api_key: config.api_key,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            template,
            client,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct E2bRunArguments {
    command: String,
}

#[derive(Deserialize)]
struct CreateSandboxResponse {
    sandbox_id: String,
}

#[derive(Deserialize)]
struct RunCommandResponse {
    stdout: String,
    exit_code: i32,
}

impl Toolset for E2bSandboxToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let expected_id = self.tool_id.clone();
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let endpoint = self.endpoint.clone();
        let template = self.template.clone();
        Box::pin(async move {
            verify_authority(&ctx)?;
            if call.tool_id != expected_id || call.call.tool_name() != TOOL_NAME {
                return Err(tool_error(
                    E2B_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "e2b call identity is invalid",
                ));
            }
            let arguments: E2bRunArguments =
                serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
                    tool_error(
                        E2B_INVALID_ARGUMENTS,
                        ErrorCategory::Validation,
                        "e2b arguments are invalid",
                    )
                })?;
            if arguments.command.is_empty() {
                return Err(tool_error(
                    E2B_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "e2b command is empty",
                ));
            }
            let created = post_json::<CreateSandboxResponse>(
                &client,
                &api_key,
                &format!("{endpoint}/sandboxes"),
                &serde_json::json!({ "template": template }),
                &ctx,
            )
            .await?;
            if created.sandbox_id.is_empty() {
                return Err(tool_error(
                    E2B_TRANSPORT_FAILED,
                    ErrorCategory::Tool,
                    "e2b sandbox create omitted sandbox_id",
                ));
            }
            let ran = post_json::<RunCommandResponse>(
                &client,
                &api_key,
                &format!("{endpoint}/sandboxes/{}/run", created.sandbox_id),
                &serde_json::json!({ "command": arguments.command }),
                &ctx,
            )
            .await?;
            let output = serde_json::to_vec(&serde_json::json!({
                "sandbox_id": created.sandbox_id,
                "stdout": ran.stdout,
                "exit_code": ran.exit_code,
            }))
            .map_err(|_| {
                tool_error(
                    E2B_TRANSPORT_FAILED,
                    ErrorCategory::Internal,
                    "e2b result serialization failed",
                )
            })?;
            let result = ToolResult {
                output: RawJson::parse(output).map_err(|_| {
                    tool_error(
                        E2B_TRANSPORT_FAILED,
                        ErrorCategory::Internal,
                        "e2b result normalization failed",
                    )
                })?,
                is_error: ran.exit_code != 0,
            };
            Ok(Box::pin(stream::once(async move {
                Ok(ToolStreamItem::Completed(result))
            })) as ToolEventStream)
        })
    }
}

async fn post_json<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    api_key: &str,
    url: &str,
    body: &serde_json::Value,
    ctx: &ToolCallContext,
) -> Result<T, ToolError> {
    if ctx.run.cancellation.is_cancelled() || deadline_elapsed(ctx.run.deadline) {
        return Err(timeout_error());
    }
    let send = client
        .post(url)
        .header("X-API-Key", api_key)
        .header("Content-Type", "application/json")
        .json(body)
        .send();
    let response = tokio::select! {
        () = ctx.run.cancellation.cancelled() => return Err(timeout_error()),
        () = wait_deadline(ctx.run.deadline) => return Err(timeout_error()),
        result = send => result.map_err(|_| {
            tool_error(
                E2B_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "e2b request failed",
            )
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(tool_error(
            E2B_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "e2b endpoint rejected the request",
        ));
    }
    read_bounded_json(response).await
}

async fn read_bounded_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, ToolError> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            tool_error(
                E2B_TRANSPORT_FAILED,
                ErrorCategory::Tool,
                "e2b response is invalid",
            )
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_RESULT_BYTES {
            return Err(tool_error(
                E2B_LIMIT_EXCEEDED,
                ErrorCategory::Limit,
                "e2b response exceeds the configured byte limit",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| {
        tool_error(
            E2B_TRANSPORT_FAILED,
            ErrorCategory::Tool,
            "e2b response is invalid",
        )
    })
}

fn deadline_elapsed(deadline: Option<Timestamp>) -> bool {
    let Some(deadline) = deadline else {
        return false;
    };
    now_unix_ms() >= deadline.as_unix_ms()
}

async fn wait_deadline(deadline: Option<Timestamp>) {
    let Some(deadline) = deadline else {
        std::future::pending::<()>().await;
        return;
    };
    let remaining = deadline.as_unix_ms().saturating_sub(now_unix_ms());
    let millis = u64::try_from(remaining).unwrap_or(0);
    if millis == 0 {
        return;
    }
    tokio::time::sleep(Duration::from_millis(millis)).await;
}

fn now_unix_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis()),
    )
    .unwrap_or(i64::MAX)
}

fn timeout_error() -> ToolError {
    tool_error(
        E2B_TIMEOUT,
        ErrorCategory::Deadline,
        "e2b request was cancelled or exceeded its deadline",
    )
}

fn validate_endpoint(value: &str) -> Result<(), E2bSandboxError> {
    let Some((scheme, rest)) = value.split_once("://") else {
        return Err(E2bSandboxError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(E2bSandboxError::EndpointInvalid {
            reason: "endpoint must be an http or https URL",
        });
    }
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return Err(E2bSandboxError::EndpointInvalid {
            reason: "endpoint contains forbidden components",
        });
    }
    let host = endpoint_host(rest).ok_or(E2bSandboxError::EndpointInvalid {
        reason: "endpoint host is missing",
    })?;
    if scheme.eq_ignore_ascii_case("http") && !is_loopback_host(host) {
        return Err(E2bSandboxError::EndpointInvalid {
            reason: "plaintext HTTP is allowed only for loopback endpoints",
        });
    }
    Ok(())
}

fn endpoint_host(rest: &str) -> Option<&str> {
    if let Some(rest) = rest.strip_prefix('[') {
        return rest.split(']').next().filter(|host| !host.is_empty());
    }
    rest.split(['/', ':'])
        .next()
        .filter(|host| !host.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .map_or(host, |rest| rest.strip_suffix(']').unwrap_or(rest));
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|addr| addr.is_loopback())
}

fn tool_error(code: &'static str, category: ErrorCategory, message: &'static str) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_runtime::{
        AuthorizationContext, CancellationSignal, Digest, EffectId, EffectOutputContract,
        EffectOutputKind, LaneId, Metadata, OperationLocator, PrincipalRef, RawJson,
        RunCallContext, RunId, SessionId, ToolBatchId, ToolCallBlock, ToolCallId,
        ToolFailurePolicy, Toolset, ValidatedToolCall,
    };
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use super::{
        E2B_CREDENTIAL_REQUIRED, E2bSandboxConfig, E2bSandboxError, E2bSandboxToolset, TOOL_NAME,
    };

    const CANARY: &str = "e2b-secret-canary-045";

    #[test]
    fn construction_rejects_a_missing_api_key() {
        let error = E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key: String::new(),
            endpoint: "https://api.e2b.dev".into(),
            template: None,
        })
        .expect_err("missing key");
        assert_eq!(error, E2bSandboxError::CredentialRequired);
        assert!(error.to_string().contains(E2B_CREDENTIAL_REQUIRED));
    }

    #[test]
    fn construction_rejects_plaintext_non_loopback() {
        let error = E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key: CANARY.into(),
            endpoint: "http://8.8.8.8".into(),
            template: None,
        })
        .expect_err("plaintext");
        assert!(error.to_string().contains("plaintext HTTP"));
        assert!(!error.to_string().contains(CANARY));
    }

    #[test]
    fn debug_does_not_leak_the_api_key() {
        let tools = E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key: CANARY.into(),
            endpoint: "https://api.e2b.dev".into(),
            template: None,
        })
        .expect("tools");
        assert!(!format!("{tools:?}").contains(CANARY));
    }

    #[test]
    fn e2b_is_not_a_wasm_host_sdk_dependency() {
        let manifest = include_str!("../../../../crates/finstack-ai/Cargo.toml");
        let wasm_host = manifest
            .lines()
            .find(|line| line.contains("wasm-host ="))
            .expect("wasm-host feature");
        assert!(
            !wasm_host.contains("finstack-ai-sandbox-e2b"),
            "e2b must stay off the wasm-host feature graph"
        );
        assert!(manifest.contains("dep:finstack-ai-sandbox-e2b"));
    }

    #[test]
    fn loopback_ipv6_http_is_accepted() {
        E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key: CANARY.into(),
            endpoint: "http://[::1]".into(),
            template: None,
        })
        .expect("ipv6 loopback");
    }

    #[tokio::test]
    async fn cancelled_call_does_not_reach_the_fixture() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0_u8; 256];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let _ = seen_tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            }
        });
        let tools = E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key: CANARY.into(),
            endpoint: format!("http://{addr}"),
            template: None,
        })
        .expect("tools");
        let spec = &tools.tools()[0];
        let ctx = tool_context();
        ctx.run.cancellation.cancel();
        let call = ValidatedToolCall {
            call: ToolCallBlock::try_new(
                ToolCallId::from_bytes([6; 16]),
                TOOL_NAME,
                RawJson::parse(br#"{"command":"echo hi"}"#).expect("args"),
            )
            .expect("call"),
            tool_id: spec.id.clone(),
            component: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"{}"),
            },
            retry_safety: spec.retry_safety,
            deadline: None,
            execution: spec.execution,
            failure_policy: ToolFailurePolicy::ReturnToModel,
        };
        let Err(error) = tools.call(ctx, call).await else {
            panic!("cancelled");
        };
        assert_eq!(error.code(), crate::E2B_TIMEOUT);
        assert!(
            seen_rx.try_recv().is_err(),
            "no HTTP must reach the fixture"
        );
        server.abort();
    }

    #[tokio::test]
    async fn oversized_json_fails_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let huge = format!(r#"{{"sandbox_id":"{}","stdout":"x"}}"#, "s".repeat(70_000));
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0_u8; 8_192];
            let _ = stream.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{huge}",
                huge.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
        let tools = E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key: CANARY.into(),
            endpoint: format!("http://{addr}"),
            template: None,
        })
        .expect("tools");
        let spec = &tools.tools()[0];
        let call = ValidatedToolCall {
            call: ToolCallBlock::try_new(
                ToolCallId::from_bytes([6; 16]),
                TOOL_NAME,
                RawJson::parse(br#"{"command":"echo hi"}"#).expect("args"),
            )
            .expect("call"),
            tool_id: spec.id.clone(),
            component: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"{}"),
            },
            retry_safety: spec.retry_safety,
            deadline: None,
            execution: spec.execution,
            failure_policy: ToolFailurePolicy::ReturnToModel,
        };
        let Err(error) = tools.call(tool_context(), call).await else {
            panic!("oversize");
        };
        assert_eq!(error.code(), crate::E2B_LIMIT_EXCEEDED);
        server.abort();
    }

    #[tokio::test]
    async fn scripted_http_fixture_starts_one_sandbox() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            serve_scripted(listener, seen_tx).await;
        });

        let tools = E2bSandboxToolset::try_new(E2bSandboxConfig {
            api_key: CANARY.into(),
            endpoint: format!("http://{addr}"),
            template: Some("base".into()),
        })
        .expect("tools");
        let spec = &tools.tools()[0];
        let call = ValidatedToolCall {
            call: ToolCallBlock::try_new(
                ToolCallId::from_bytes([6; 16]),
                TOOL_NAME,
                RawJson::parse(br#"{"command":"echo hi"}"#).expect("args"),
            )
            .expect("call"),
            tool_id: spec.id.clone(),
            component: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"{}"),
            },
            retry_safety: spec.retry_safety,
            deadline: None,
            execution: spec.execution,
            failure_policy: ToolFailurePolicy::ReturnToModel,
        };
        let mut stream = tools
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let crate::ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        assert!(!result.is_error);
        let payload: serde_json::Value =
            serde_json::from_slice(result.output.as_bytes()).expect("json");
        assert_eq!(payload["sandbox_id"], "sbx-1");
        assert_eq!(payload["stdout"], "hi\n");
        assert_eq!(payload["exit_code"], 0);
        let first = seen_rx.recv().await.expect("create").to_ascii_lowercase();
        let second = seen_rx.recv().await.expect("run").to_ascii_lowercase();
        assert!(first.contains("post /sandboxes"));
        assert!(first.contains("e2b-secret-canary-045"));
        assert!(second.contains("post /sandboxes/sbx-1/run"));
        server.await.expect("server");
    }

    async fn serve_scripted(listener: TcpListener, seen: mpsc::UnboundedSender<String>) {
        respond(&listener, &seen, 201, r#"{"sandbox_id":"sbx-1"}"#).await;
        respond(
            &listener,
            &seen,
            200,
            "{\"stdout\":\"hi\\n\",\"exit_code\":0}",
        )
        .await;
    }

    async fn respond(
        listener: &TcpListener,
        seen: &mpsc::UnboundedSender<String>,
        status: u16,
        body: &str,
    ) {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0_u8; 8_192];
        let n = stream.read(&mut buf).await.expect("read");
        seen.send(String::from_utf8_lossy(&buf[..n]).into_owned())
            .expect("seen");
        let reason = if status == 201 { "Created" } else { "OK" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.expect("write");
        stream.shutdown().await.expect("shutdown");
    }

    fn tool_context() -> crate::ToolCallContext {
        crate::ToolCallContext {
            run: RunCallContext {
                locator: OperationLocator::try_new(
                    "tenant-a",
                    SessionId::from_bytes([1; 16]),
                    LaneId::from_bytes([2; 16]),
                    RunId::from_bytes([3; 16]),
                )
                .expect("locator"),
                authorization: AuthorizationContext {
                    principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                        .expect("principal"),
                    authentication_method: Arc::from("test"),
                    assurance_level: Arc::from("test"),
                    roles: Arc::from([]),
                    permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                    safe_claims: Metadata::empty(),
                    policy_version: Arc::from("policy-v1"),
                    decision_id: Arc::from("decision-v1"),
                },
                effect_id: EffectId::from_bytes([4; 16]),
                attempt: 1,
                deadline: None,
                budget_scope_id: None,
                cancellation: CancellationSignal::new(),
            },
            tool_batch_id: ToolBatchId::from_bytes([5; 16]),
            tool_call_id: ToolCallId::from_bytes([6; 16]),
        }
    }
}
