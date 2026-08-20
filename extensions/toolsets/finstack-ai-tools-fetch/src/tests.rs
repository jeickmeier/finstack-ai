use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata, OperationLocator,
    PrincipalRef, RawJson, RunId, SessionId, Timestamp, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolFailurePolicy, ValidatedToolCall,
};
use finstack_ai_context_memory::InProcessArtifactStore;
use finstack_ai_net_guard::{HostResolver, UrlPolicy, parse_and_vet_url};
use finstack_ai_runtime::{
    ArtifactStore, AuthorizationContext, CancellationSignal, RunCallContext, ToolError, Toolset,
};
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::{HostPattern, HttpFetchConfig, HttpFetchToolset};

const HEADER_CANARY: &str = "fetch-secret-canary-091";

/// Verbatim copy of the helper from
/// `finstack-ai-tools-openrouter-media/src/lib.rs:583-636`, adjusted to this
/// crate's `crate::` re-exports.
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

/// `tool_context()` with a caller-supplied deadline and cancellation signal,
/// for exercising the pipeline's cancellation/deadline gate.
fn tool_context_with(deadline: Option<Timestamp>, cancellation: CancellationSignal) -> crate::ToolCallContext {
    crate::ToolCallContext {
        run: RunCallContext {
            deadline,
            cancellation,
            ..tool_context().run
        },
        ..tool_context()
    }
}

/// Verbatim copy of the helper from
/// `finstack-ai-tools-openrouter-media/src/lib.rs:583-636`, adjusted to this
/// crate's `crate::` re-exports.
fn call_for(spec: &crate::ToolSpec, args: &[u8]) -> ValidatedToolCall {
    ValidatedToolCall {
        call: ToolCallBlock::try_new(
            ToolCallId::from_bytes([6; 16]),
            spec.model_name.as_ref(),
            RawJson::parse(args).expect("args"),
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
    }
}

/// Drive one `call()` to its terminal error, whether it surfaces directly
/// from the future or from the first stream item.
async fn drive_to_error(toolset: &HttpFetchToolset, call: ValidatedToolCall) -> ToolError {
    drive_to_error_with_ctx(toolset, tool_context(), call).await
}

/// Like [`drive_to_error`], but with a caller-supplied `ToolCallContext`.
async fn drive_to_error_with_ctx(
    toolset: &HttpFetchToolset,
    ctx: crate::ToolCallContext,
    call: ValidatedToolCall,
) -> ToolError {
    match toolset.call(ctx, call).await {
        Err(error) => error,
        Ok(mut stream) => stream
            .next()
            .await
            .expect("stream item")
            .expect_err("expected an error"),
    }
}

/// Drive one `call()` to its terminal success value.
async fn drive_to_success(
    toolset: &HttpFetchToolset,
    ctx: crate::ToolCallContext,
    call: ValidatedToolCall,
) -> serde_json::Value {
    let mut stream = toolset.call(ctx, call).await.expect("call succeeded");
    let item = stream
        .next()
        .await
        .expect("stream item")
        .expect("expected success");
    let finstack_ai_runtime::ToolStreamItem::Completed(result) = item else {
        panic!("expected a completed result");
    };
    assert!(!result.is_error);
    serde_json::from_slice(result.output.as_bytes()).expect("json output")
}

fn config_with(hosts: &[&str]) -> HttpFetchConfig {
    HttpFetchConfig {
        allowlist: hosts.iter().map(|h| (*h).to_owned()).collect(),
        ..HttpFetchConfig::default()
    }
}

#[test]
fn empty_allowlist_refuses_to_construct() {
    HttpFetchToolset::try_new(config_with(&[])).expect_err("deny by default");
}

#[test]
fn out_of_ceiling_values_are_errors_not_clamps() {
    for config in [
        HttpFetchConfig {
            max_response_bytes: 9 * 1_048_576,
            ..config_with(&["docs.rs"])
        },
        HttpFetchConfig {
            max_response_bytes: 0,
            ..config_with(&["docs.rs"])
        },
        HttpFetchConfig {
            request_timeout: Duration::from_secs(121),
            ..config_with(&["docs.rs"])
        },
        HttpFetchConfig {
            request_timeout: Duration::ZERO,
            ..config_with(&["docs.rs"])
        },
        HttpFetchConfig {
            max_redirects: 6,
            ..config_with(&["docs.rs"])
        },
    ] {
        HttpFetchToolset::try_new(config).expect_err("ceiling");
    }
}

#[test]
fn host_patterns_match_exact_and_wildcard() {
    let exact = HostPattern::parse("docs.rs").unwrap();
    assert!(exact.matches("docs.rs"));
    assert!(exact.matches("DOCS.RS"));
    assert!(!exact.matches("sub.docs.rs"));
    let wild = HostPattern::parse("*.wikipedia.org").unwrap();
    assert!(wild.matches("en.wikipedia.org"));
    assert!(wild.matches("a.b.wikipedia.org"));
    assert!(!wild.matches("wikipedia.org")); // never the bare apex
    assert!(!wild.matches("evilwikipedia.org"));
    for bad in [
        "",
        "*",
        "*.",
        "*.*.x",
        "docs.rs/path",
        "https://docs.rs",
        "doc s.rs",
    ] {
        HostPattern::parse(bad).expect_err(bad);
    }
}

#[test]
fn debug_redacts_per_host_headers() {
    let mut config = config_with(&["docs.rs"]);
    config.per_host_headers.insert(
        "docs.rs".to_owned(),
        vec![("Cookie".to_owned(), HEADER_CANARY.to_owned())],
    );
    assert!(!format!("{config:?}").contains(HEADER_CANARY));
    let toolset = HttpFetchToolset::try_new(config).unwrap();
    assert!(!format!("{toolset:?}").contains(HEADER_CANARY));
}

#[test]
fn wildcard_per_host_header_key_is_a_construction_error() {
    let mut config = config_with(&["docs.rs", "*.wikipedia.org"]);
    config.per_host_headers.insert(
        "*.wikipedia.org".to_owned(),
        vec![("Cookie".to_owned(), HEADER_CANARY.to_owned())],
    );
    HttpFetchToolset::try_new(config).expect_err("wildcard keys are not exact hosts");
}

#[test]
fn per_host_header_keys_are_normalized_to_lowercase() {
    let mut config = config_with(&["docs.rs"]);
    config.per_host_headers.insert(
        "Docs.RS".to_owned(),
        vec![("X-Test".to_owned(), "value".to_owned())],
    );
    let toolset = HttpFetchToolset::try_new(config).unwrap();
    let debug = format!("{toolset:?}");
    assert!(debug.contains("docs.rs"));
    assert!(!debug.contains("Docs.RS"));
}

#[test]
fn tool_spec_is_valid_and_read_only() {
    let toolset = HttpFetchToolset::try_new(config_with(&["docs.rs"])).unwrap();
    let tools = toolset.tools();
    assert_eq!(tools.len(), 1);
    let spec = &tools[0];
    assert_eq!(spec.model_name.as_ref(), "http_fetch");
    assert!(spec.validate().is_ok());
}

#[tokio::test]
async fn unknown_argument_fields_are_rejected() {
    let toolset = HttpFetchToolset::try_new(config_with(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let call = call_for(&spec, br#"{"url":"https://docs.rs/","surprise":1}"#);
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_INVALID_ARGUMENTS);
}

// --- Task 7: request pipeline -------------------------------------------

/// A fixture config with `allow_loopback_http: true`, letting loopback
/// fixtures skip the allowlist per `HttpFetchConfig::allow_loopback_http`'s
/// documented bypass.
fn loopback_config(hosts: &[&str]) -> HttpFetchConfig {
    HttpFetchConfig {
        allow_loopback_http: true,
        ..config_with(hosts)
    }
}

/// Verbatim-shape copy of `serve_once` from
/// `finstack-ai-net-guard/src/tests.rs:143-156`, extended to capture the raw
/// request bytes it received onto `seen` (mirroring the
/// `finstack-ai-tools-openrouter-media` `respond()` mpsc pattern).
async fn serve_once(
    listener: TcpListener,
    seen: Option<mpsc::UnboundedSender<String>>,
    status: u16,
    headers: String,
    body: Vec<u8>,
) {
    let (mut stream, _) = listener.accept().await.expect("accept");
    let mut buf = vec![0_u8; 16_384];
    let n = stream.read(&mut buf).await.expect("read");
    if let Some(seen) = seen {
        seen.send(String::from_utf8_lossy(&buf[..n]).into_owned())
            .expect("seen");
    }
    let mut response = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    stream.write_all(&response).await.expect("write");
    stream.shutdown().await.expect("shutdown");
}

struct ScriptedResolver(Vec<SocketAddr>);

impl HostResolver for ScriptedResolver {
    fn resolve(
        &self,
        _host: &str,
        _port: u16,
    ) -> Pin<Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + '_>> {
        let addrs = self.0.clone();
        Box::pin(async move { Ok(addrs) })
    }
}

#[tokio::test]
async fn fetch_returns_inline_text() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_once(listener, None, 200, "Content-Type: text/plain\r\n".to_owned(), b"hello".to_vec()));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let output = drive_to_success(&toolset, tool_context(), call).await;

    assert_eq!(output["url"], url);
    assert_eq!(output["final_url"], url);
    assert_eq!(output["status"], 200);
    assert_eq!(output["media_type"], "text/plain");
    assert_eq!(output["byte_length"], 5);
    assert_eq!(output["content"], "hello");
}

#[tokio::test]
async fn non_allowlisted_host_is_denied_before_any_connection() {
    // No fixture server at all: a denied host must fail without I/O.
    let toolset = HttpFetchToolset::try_new(config_with(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let call = call_for(&spec, br#"{"url":"https://example.com/"}"#);
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_HOST_NOT_ALLOWLISTED);
}

#[tokio::test]
async fn private_destination_is_blocked() {
    let toolset = HttpFetchToolset::try_new(config_with(&["internal.example"]))
        .unwrap()
        .with_resolver(Arc::new(ScriptedResolver(vec![SocketAddr::new(
            "10.0.0.1".parse().unwrap(),
            443,
        )])));
    let spec = toolset.tools()[0].clone();
    let call = call_for(&spec, br#"{"url":"https://internal.example/"}"#);
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_DESTINATION_BLOCKED);
}

#[tokio::test]
async fn oversize_body_is_a_limit_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![b'x'; 64];
    tokio::spawn(serve_once(listener, None, 200, String::new(), body));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(
        &spec,
        format!(r#"{{"url":"{url}","max_bytes":8}}"#).as_bytes(),
    );
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_LIMIT_EXCEEDED);
}

#[tokio::test]
async fn invalid_utf8_body_without_store_is_an_error() {
    // Task 9 semantics: within an inline-text essence (`text/plain` here),
    // a body that fails `String::from_utf8` falls through to the binary
    // path rather than being force-decoded with `String::from_utf8_lossy`
    // (that lossy path is now `mode: "text"` only). This exercises the
    // `Err` arm of `deliver_auto_or_markdown`'s `String::from_utf8` match
    // (deliver.rs), which only fires when the essence is in the
    // inline-text set to begin with — hence the explicit Content-Type,
    // distinct from an untyped/empty-essence body (already covered by
    // `binary_body_without_store_is_a_limit_error`). With no artifact
    // store attached, binary content is refused rather than staged. 60 raw
    // bytes of 0xFF fit the byte cap but are not valid UTF-8.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![0xFF_u8; 60];
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: text/plain\r\n".to_owned(),
        body,
    ));

    let config = HttpFetchConfig {
        max_response_bytes: 64,
        ..loopback_config(&["docs.rs"])
    };
    let toolset = HttpFetchToolset::try_new(config).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_LIMIT_EXCEEDED);
    assert!(error.to_string().contains("binary"), "{error}");
}

#[tokio::test]
async fn invalid_utf8_body_under_text_essence_with_store_is_staged() {
    // Other arm of the same `Err` branch covered above: with a store
    // attached, the same invalid-UTF-8-under-`text/plain` body is staged
    // as an artifact instead of erroring.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![0xFF_u8; 60];
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: text/plain\r\n".to_owned(),
        body,
    ));

    let config = HttpFetchConfig {
        max_response_bytes: 64,
        ..loopback_config(&["docs.rs"])
    };
    let store = Arc::new(InProcessArtifactStore::default());
    let toolset = HttpFetchToolset::try_new(config)
        .unwrap()
        .with_artifact_store(Arc::clone(&store) as Arc<dyn ArtifactStore>);
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let output = drive_to_success(&toolset, tool_context(), call).await;
    assert!(output.get("artifact").is_some(), "{output}");
    assert!(output.get("content").is_none(), "{output}");
}

// --- Task 9: mode handling and artifact staging -------------------------

#[tokio::test]
async fn binary_body_with_store_is_staged_as_an_artifact() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![0_u8, 159, 146, 150];
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: application/octet-stream\r\n".to_owned(),
        body,
    ));

    let store = Arc::new(InProcessArtifactStore::default());
    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"]))
        .unwrap()
        .with_artifact_store(Arc::clone(&store) as Arc<dyn ArtifactStore>);
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let output = drive_to_success(&toolset, tool_context(), call).await;

    assert_eq!(output["byte_length"], 4);
    assert!(output.get("artifact").is_some(), "{output}");
    assert!(output.get("content").is_none(), "{output}");
}

#[tokio::test]
async fn binary_body_without_store_is_a_limit_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![0_u8, 159, 146, 150];
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: application/octet-stream\r\n".to_owned(),
        body,
    ));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_LIMIT_EXCEEDED);
    assert!(error.to_string().contains("binary"), "{error}");
}

#[tokio::test]
async fn artifact_mode_stages_a_text_body_when_a_store_is_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: text/plain\r\n".to_owned(),
        b"hello".to_vec(),
    ));

    let store = Arc::new(InProcessArtifactStore::default());
    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"]))
        .unwrap()
        .with_artifact_store(Arc::clone(&store) as Arc<dyn ArtifactStore>);
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(
        &spec,
        format!(r#"{{"url":"{url}","mode":"artifact"}}"#).as_bytes(),
    );
    let output = drive_to_success(&toolset, tool_context(), call).await;

    assert!(output.get("artifact").is_some(), "{output}");
    assert!(output.get("content").is_none(), "{output}");
}

#[tokio::test]
async fn json_body_inlines_under_auto_mode() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: application/json\r\n".to_owned(),
        b"{\"a\":1}".to_vec(),
    ));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let output = drive_to_success(&toolset, tool_context(), call).await;

    assert_eq!(output["content"], "{\"a\":1}");
    assert!(output.get("artifact").is_none(), "{output}");
}

#[tokio::test]
async fn max_bytes_argument_below_config_cap_is_honored_as_the_inline_budget() {
    // Body is valid UTF-8 (so it would otherwise inline) and sized between
    // the caller's `max_bytes` and the config's `max_response_bytes`, so the
    // raw-read cap (`effective_cap = min(config, max_bytes)`) both bounds the
    // read and becomes the inline-result budget: a 40-byte body must be
    // rejected against a `max_bytes: 8` argument even though it is well
    // under the 64-byte config cap.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![b'x'; 40];
    tokio::spawn(serve_once(listener, None, 200, String::new(), body));

    let config = HttpFetchConfig {
        max_response_bytes: 64,
        ..loopback_config(&["docs.rs"])
    };
    let toolset = HttpFetchToolset::try_new(config).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(
        &spec,
        format!(r#"{{"url":"{url}","max_bytes":8}}"#).as_bytes(),
    );
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_LIMIT_EXCEEDED);
}

#[tokio::test]
async fn text_mode_on_binary_body_inlines_lossy_text() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![0_u8, 159, 146, 150];
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: application/octet-stream\r\n".to_owned(),
        body,
    ));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(
        &spec,
        format!(r#"{{"url":"{url}","mode":"text"}}"#).as_bytes(),
    );
    let output = drive_to_success(&toolset, tool_context(), call).await;

    let content = output["content"].as_str().expect("content string");
    assert!(content.contains('\u{FFFD}'), "{content}");
    assert!(output.get("artifact").is_none(), "{output}");
}

#[tokio::test]
async fn text_mode_lossy_expansion_past_the_inline_budget_is_a_limit_error() {
    // `mode: "text"` always inlines via `String::from_utf8_lossy`, which
    // expands each invalid byte to a 3-byte U+FFFD replacement. 60 raw
    // bytes of 0xFF pass the raw-read cap (`effective_cap = min(config,
    // max_bytes) = 64`) but expand to 180 bytes of lossy text, which must
    // be rejected against that same cap as the inline-result budget
    // (`deliver::inline_within_budget`'s error branch).
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = vec![0xFF_u8; 60];
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: application/octet-stream\r\n".to_owned(),
        body,
    ));

    let config = HttpFetchConfig {
        max_response_bytes: 1_048_576,
        ..loopback_config(&["docs.rs"])
    };
    let toolset = HttpFetchToolset::try_new(config).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(
        &spec,
        format!(r#"{{"url":"{url}","mode":"text","max_bytes":64}}"#).as_bytes(),
    );
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_LIMIT_EXCEEDED);
}

#[tokio::test]
async fn non_success_status_reports_bounded_detail() {
    const TAIL_MARKER: &str = "TAIL-MARKER-CANARY";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut body = vec![b'e'; 10 * 1024];
    body.extend_from_slice(TAIL_MARKER.as_bytes());
    tokio::spawn(serve_once(listener, None, 503, String::new(), body));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_TRANSPORT_FAILED);
    let message = error.to_string();
    assert!(message.contains("503"), "{message}");
    assert!(message.len() < 500, "{message}");
    assert!(!message.contains(TAIL_MARKER), "{message}");
}

#[tokio::test]
async fn cancelled_or_deadline_expired_calls_return_fetch_timeout() {
    let toolset = HttpFetchToolset::try_new(config_with(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();

    // Pre-cancelled: must fail without any connection attempt.
    let cancellation = CancellationSignal::new();
    cancellation.cancel();
    let ctx = tool_context_with(None, cancellation);
    let call = call_for(&spec, br#"{"url":"https://docs.rs/"}"#);
    let error = drive_to_error_with_ctx(&toolset, ctx, call).await;
    assert_eq!(error.code(), super::FETCH_TIMEOUT);

    // Deadline already in the past.
    let ctx = tool_context_with(
        Some(Timestamp::from_unix_ms(1).unwrap()),
        CancellationSignal::new(),
    );
    let call = call_for(&spec, br#"{"url":"https://docs.rs/"}"#);
    let error = drive_to_error_with_ctx(&toolset, ctx, call).await;
    assert_eq!(error.code(), super::FETCH_TIMEOUT);
}

#[tokio::test]
async fn per_host_headers_are_sent_to_the_matching_host() {
    let listener_with = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_with = listener_with.local_addr().unwrap();
    let (tx_with, mut rx_with) = mpsc::unbounded_channel();
    tokio::spawn(serve_once(
        listener_with,
        Some(tx_with),
        200,
        "Content-Type: text/plain\r\n".to_owned(),
        b"ok".to_vec(),
    ));

    let listener_without = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_without = listener_without.local_addr().unwrap();
    let (tx_without, mut rx_without) = mpsc::unbounded_channel();
    tokio::spawn(serve_once(
        listener_without,
        Some(tx_without),
        200,
        "Content-Type: text/plain\r\n".to_owned(),
        b"ok".to_vec(),
    ));

    let mut config = loopback_config(&["docs.rs"]);
    config.per_host_headers.insert(
        "127.0.0.1".to_owned(),
        vec![("X-Api".to_owned(), "canary".to_owned())],
    );
    // Per-host headers are scoped to the exact-host key ("127.0.0.1", no
    // port). The "with" fixture is addressed by that literal, so it
    // receives the header; the "without" fixture is addressed as
    // "localhost" (also loopback, but a different host string), so it must
    // not. The literal-IP "with" request skips DNS entirely, so scripting
    // the resolver to answer the "without" fixture's address only affects
    // the "localhost" lookup, keeping this deterministic regardless of
    // whether the system resolver prefers ::1 or 127.0.0.1 for "localhost".
    let toolset = HttpFetchToolset::try_new(config)
        .unwrap()
        .with_resolver(Arc::new(ScriptedResolver(vec![addr_without])));
    let spec = toolset.tools()[0].clone();

    let url_with = format!("http://127.0.0.1:{}/x", addr_with.port());
    let call_with = call_for(&spec, format!(r#"{{"url":"{url_with}"}}"#).as_bytes());
    let _ = drive_to_success(&toolset, tool_context(), call_with).await;
    let request_with = rx_with.recv().await.expect("request seen");
    assert!(request_with.contains("x-api: canary"), "{request_with}");

    let url_without = format!("http://localhost:{}/x", addr_without.port());
    let call_without = call_for(&spec, format!(r#"{{"url":"{url_without}"}}"#).as_bytes());
    let _ = drive_to_success(&toolset, tool_context(), call_without).await;
    let request_without = rx_without.recv().await.expect("request seen");
    assert!(!request_without.contains("x-api"), "{request_without}");
}

// --- Task 8: manual redirects with per-hop re-vetting -------------------

/// Serve `responses.len()` requests in sequence on one listener, each as
/// `(status, headers, body)`. Optionally reports each raw request onto
/// `seen`, one message per accepted connection, in order.
async fn serve_sequence(
    listener: TcpListener,
    seen: Option<mpsc::UnboundedSender<String>>,
    responses: Vec<(u16, String, Vec<u8>)>,
) {
    for (status, headers, body) in responses {
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0_u8; 16_384];
        let n = stream.read(&mut buf).await.expect("read");
        if let Some(seen) = &seen {
            seen.send(String::from_utf8_lossy(&buf[..n]).into_owned())
                .expect("seen");
        }
        let mut response = format!(
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(&body);
        stream.write_all(&response).await.expect("write");
        stream.shutdown().await.expect("shutdown");
    }
}

#[test]
fn loopback_redirect_bypass_requires_loopback_origin() {
    // Regression for a remote-steered-loopback finding: with
    // `allow_loopback_http: true`, the loopback allowlist bypass must only
    // apply when the *original* request was itself loopback — otherwise an
    // allowlisted public host could 302 a caller into a local-only service
    // via `Location: http://127.0.0.1:.../`. `host_allowed` takes
    // `origin_is_loopback` as a caller-supplied fact (fixed for the whole
    // redirect chain, computed once from hop 0) rather than re-deriving it
    // from `vetted`, so this test exercises both true and false directly.
    let config = loopback_config(&["docs.rs"]);
    let policy = UrlPolicy {
        allow_loopback_http: true,
    };
    let loopback = parse_and_vet_url("http://127.0.0.1:1/", &policy).expect("loopback url vets");
    assert!(loopback.is_loopback, "sanity: fixture url is loopback");

    // Loopback origin (hop 0 was itself loopback): the bypass applies.
    assert!(crate::pipeline::host_allowed(&loopback, true, &config, &[]));

    // Non-loopback origin (e.g. hop 0 was an allowlisted public host that
    // redirected to loopback): the bypass must NOT apply, and an empty
    // pattern list must deny.
    assert!(!crate::pipeline::host_allowed(&loopback, false, &config, &[]));
}

#[tokio::test]
async fn same_host_redirect_is_followed() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_sequence(
        listener,
        None,
        vec![
            (302, "Location: /b\r\n".to_owned(), Vec::new()),
            (200, "Content-Type: text/plain\r\n".to_owned(), b"moved-ok".to_vec()),
        ],
    ));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/a", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let output = drive_to_success(&toolset, tool_context(), call).await;

    assert_eq!(output["url"], url);
    assert!(
        output["final_url"].as_str().unwrap().ends_with("/b"),
        "{output}"
    );
    assert_eq!(output["content"], "moved-ok");
}

#[tokio::test]
async fn cross_host_redirect_to_unlisted_host_is_denied() {
    // No server exists for evil.example: if the pipeline tried to connect,
    // resolution/connection would fail with a transport error instead, so
    // asserting the exact allowlist code proves the gate fired first.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_sequence(
        listener,
        None,
        vec![(
            302,
            "Location: https://evil.example/\r\n".to_owned(),
            Vec::new(),
        )],
    ));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"])).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/a", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_HOST_NOT_ALLOWLISTED);
}

#[tokio::test]
async fn redirect_limit_is_enforced() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    // max_redirects: 3 permits 3 follows (4 requests total); the pipeline
    // must error after the 4th response without attempting a 5th request,
    // so the server only has 4 responses queued.
    let responses = std::iter::repeat_with(|| (302, "Location: /a\r\n".to_owned(), Vec::new()))
        .take(4)
        .collect();
    tokio::spawn(serve_sequence(listener, Some(tx), responses));

    let config = HttpFetchConfig {
        max_redirects: 3,
        ..loopback_config(&["docs.rs"])
    };
    let toolset = HttpFetchToolset::try_new(config).unwrap();
    let spec = toolset.tools()[0].clone();
    let url = format!("http://127.0.0.1:{}/a", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let error = drive_to_error(&toolset, call).await;
    assert_eq!(error.code(), super::FETCH_REDIRECT_DENIED);

    let mut count = 0;
    while rx.recv().await.is_some() {
        count += 1;
    }
    assert_eq!(count, 4, "expected exactly 4 requests total");
}

#[tokio::test]
async fn headers_do_not_cross_hosts_on_redirect() {
    // Fixture A is addressed literally as "127.0.0.1" (matches the
    // per-host-headers key, so it gets the header); it 302s to fixture B,
    // addressed as "localhost" (a distinct host string with no entry in
    // per_host_headers, resolved via the scripted resolver as in
    // `pinned_address_overrides_dns_for_hostnames`), which must not see it.
    let listener_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_a = listener_a.local_addr().unwrap();
    let (tx_a, mut rx_a) = mpsc::unbounded_channel();

    let listener_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_b = listener_b.local_addr().unwrap();
    let (tx_b, mut rx_b) = mpsc::unbounded_channel();

    let location = format!("http://localhost:{}/y", addr_b.port());
    tokio::spawn(serve_sequence(
        listener_a,
        Some(tx_a),
        vec![(302, format!("Location: {location}\r\n"), Vec::new())],
    ));
    tokio::spawn(serve_sequence(
        listener_b,
        Some(tx_b),
        vec![(200, "Content-Type: text/plain\r\n".to_owned(), b"ok".to_vec())],
    ));

    let mut config = loopback_config(&["docs.rs"]);
    config.per_host_headers.insert(
        "127.0.0.1".to_owned(),
        vec![("X-Api".to_owned(), "canary".to_owned())],
    );
    let toolset = HttpFetchToolset::try_new(config)
        .unwrap()
        .with_resolver(Arc::new(ScriptedResolver(vec![addr_b])));
    let spec = toolset.tools()[0].clone();

    let url_a = format!("http://127.0.0.1:{}/x", addr_a.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url_a}"}}"#).as_bytes());
    let _ = drive_to_success(&toolset, tool_context(), call).await;

    let request_a = rx_a.recv().await.expect("request seen");
    assert!(request_a.contains("x-api: canary"), "{request_a}");
    let request_b = rx_b.recv().await.expect("request seen");
    assert!(!request_b.contains("x-api"), "{request_b}");
}

#[tokio::test]
async fn pinned_address_overrides_dns_for_hostnames() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve_once(
        listener,
        None,
        200,
        "Content-Type: text/plain\r\n".to_owned(),
        b"pinned".to_vec(),
    ));

    let toolset = HttpFetchToolset::try_new(loopback_config(&["docs.rs"]))
        .unwrap()
        .with_resolver(Arc::new(ScriptedResolver(vec![addr])));
    let spec = toolset.tools()[0].clone();
    let url = format!("http://localhost:{}/x", addr.port());
    let call = call_for(&spec, format!(r#"{{"url":"{url}"}}"#).as_bytes());
    let output = drive_to_success(&toolset, tool_context(), call).await;
    assert_eq!(output["content"], "pinned");
}
