use std::sync::Arc;

use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, ContextBudget, ContextCallContext,
    ContextOverflowPolicy, ContextProvider, ContextRequest, Digest, EffectId, LaneId, Metadata,
    OperationLocator, PrincipalRef, RunCallContext, RunId, SessionId,
};
use tempfile::TempDir;

use super::*;

fn id<T>(value: u64, parse: impl FnOnce(&str) -> T) -> T {
    parse(&format!("00000000-0000-7000-8000-{value:012x}"))
}

fn context() -> ContextCallContext {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    ContextCallContext {
        run: RunCallContext {
            locator: OperationLocator::try_new(
                "tenant-a",
                id(1, |value| SessionId::parse(value).expect("session")),
                id(2, |value| LaneId::parse(value).expect("lane")),
                id(3, |value| RunId::parse(value).expect("run")),
            )
            .expect("locator"),
            authorization: AuthorizationContext {
                principal,
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: id(4, |value| EffectId::parse(value).expect("effect")),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        provider_index: 0,
        chain_digest: Digest::raw_json(b"chain"),
    }
}

fn request(overflow: ContextOverflowPolicy, max_tokens: u64) -> ContextRequest {
    ContextRequest {
        session_id: id(1, |value| SessionId::parse(value).expect("session")),
        lane_id: id(2, |value| LaneId::parse(value).expect("lane")),
        run_id: id(3, |value| RunId::parse(value).expect("run")),
        user_input: Arc::from([]),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 8,
            max_tokens,
            max_bytes: 64 * 1024,
            overflow,
        },
        active_capabilities: Arc::from([]),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn collect_returns_allowlisted_files_with_provenance() {
    let root = TempDir::new().expect("root");
    std::fs::write(root.path().join("README.md"), "readme body").expect("readme");
    std::fs::write(root.path().join("AGENTS.md"), "agent body").expect("agents");
    std::fs::write(root.path().join("secret.env"), "do-not-ingest").expect("secret");
    let provider = RepositoryContextProvider::try_new(root.path()).expect("provider");
    let contribution = provider
        .collect(context(), request(ContextOverflowPolicy::Reject, 1_000))
        .await
        .expect("collect");
    assert_eq!(contribution.items.len(), 2);
    assert!(
        contribution
            .items
            .iter()
            .all(|item| item.kind == ContextItemKind::QuotedSource
                && item.authority == ContextAuthority::Untrusted
                && item.provenance.external)
    );
    let joined = contribution
        .items
        .iter()
        .flat_map(|item| item.content.iter())
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("readme body"));
    assert!(joined.contains("agent body"));
    assert!(!joined.contains("do-not-ingest"));
}

#[cfg(unix)]
#[tokio::test]
async fn budget_reject_does_not_silently_exceed() {
    let root = TempDir::new().expect("root");
    std::fs::write(root.path().join("README.md"), "word ".repeat(200)).expect("readme");
    let provider = RepositoryContextProvider::try_new(root.path()).expect("provider");
    let error = provider
        .collect(context(), request(ContextOverflowPolicy::Reject, 4))
        .await
        .expect_err("budget");
    assert_eq!(error.code(), finstack_ai_runtime::CONTEXT_BUDGET_EXCEEDED);
}

#[cfg(unix)]
#[tokio::test]
async fn budget_truncate_omits_overflow_items() {
    let root = TempDir::new().expect("root");
    std::fs::write(root.path().join("AGENTS.md"), "tiny").expect("agents");
    std::fs::write(root.path().join("README.md"), "word ".repeat(200)).expect("readme");
    let provider = RepositoryContextProvider::try_new(root.path()).expect("provider");
    let contribution = provider
        .collect(
            context(),
            request(ContextOverflowPolicy::TruncateWithDiagnostic, 8),
        )
        .await
        .expect("truncate");
    assert!(!contribution.items.is_empty());
    assert!(contribution.estimated_tokens <= 8);
}
