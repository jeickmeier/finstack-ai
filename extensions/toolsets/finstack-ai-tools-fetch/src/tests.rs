use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata, OperationLocator,
    PrincipalRef, RawJson, RunId, SessionId, ToolBatchId, ToolCallBlock, ToolCallId,
    ToolFailurePolicy, ValidatedToolCall,
};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, RunCallContext, ToolError, Toolset,
};
use futures_util::StreamExt;

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
    match toolset.call(tool_context(), call).await {
        Err(error) => error,
        Ok(mut stream) => stream
            .next()
            .await
            .expect("stream item")
            .expect_err("expected an error"),
    }
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
