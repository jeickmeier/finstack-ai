use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RunId, SessionId,
};
use finstack_ai_runtime::ports::context::{
    ContextBudget, ContextCallContext, ContextOverflowPolicy, ContextProvider, ContextRequest,
};
use finstack_ai_runtime::ports::model::{AuthorizationContext, CancellationSignal, RunCallContext};
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
            relation_depth: 0,
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
    assert_eq!(
        error.code(),
        finstack_ai_runtime::ports::context::CONTEXT_BUDGET_EXCEEDED
    );
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

#[cfg(unix)]
#[test]
fn configuration_identity_includes_root_and_allowlist() {
    let first = TempDir::new().expect("first root");
    let second = TempDir::new().expect("second root");
    let first_provider = RepositoryContextProvider::try_new(first.path()).expect("first provider");
    let same_provider = RepositoryContextProvider::try_new(first.path()).expect("same provider");
    let second_provider =
        RepositoryContextProvider::try_new(second.path()).expect("second provider");
    let narrowed = RepositoryContextProvider::try_with_allowlist(first.path(), ["README.md"])
        .expect("narrowed provider");

    assert_eq!(
        first_provider.descriptor().invocation.configuration_digest,
        same_provider.descriptor().invocation.configuration_digest
    );
    assert_ne!(
        first_provider.descriptor().invocation.configuration_digest,
        second_provider.descriptor().invocation.configuration_digest
    );
    assert_ne!(
        first_provider.descriptor().invocation.configuration_digest,
        narrowed.descriptor().invocation.configuration_digest
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cache_identity_tracks_ordered_file_content() {
    let root = TempDir::new().expect("root");
    std::fs::write(root.path().join("README.md"), "first").expect("readme");
    let provider = RepositoryContextProvider::try_with_allowlist(root.path(), ["README.md"])
        .expect("provider");
    let first = provider
        .collect(context(), request(ContextOverflowPolicy::Reject, 1_000))
        .await
        .expect("first collect");

    std::fs::write(root.path().join("README.md"), "second").expect("readme update");
    let second = provider
        .collect(context(), request(ContextOverflowPolicy::Reject, 1_000))
        .await
        .expect("second collect");

    assert_ne!(first.cache_key, second.cache_key);
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_and_invalid_text_files_fail_closed() {
    let oversized_root = TempDir::new().expect("oversized root");
    std::fs::write(
        oversized_root.path().join("README.md"),
        vec![b'x'; MAX_FILE_BYTES + 1],
    )
    .expect("oversized file");
    let oversized =
        RepositoryContextProvider::try_with_allowlist(oversized_root.path(), ["README.md"])
            .expect("oversized provider")
            .collect(context(), request(ContextOverflowPolicy::Reject, 100_000))
            .await
            .expect_err("oversized file must fail");
    assert_eq!(oversized.code(), REPOSITORY_FILE_TOO_LARGE);

    let invalid_root = TempDir::new().expect("invalid root");
    std::fs::write(invalid_root.path().join("README.md"), [0xff, 0xfe]).expect("invalid file");
    let invalid = RepositoryContextProvider::try_with_allowlist(invalid_root.path(), ["README.md"])
        .expect("invalid provider")
        .collect(context(), request(ContextOverflowPolicy::Reject, 1_000))
        .await
        .expect_err("invalid text must fail");
    assert_eq!(invalid.code(), REPOSITORY_TEXT_INVALID);
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_allowlisted_file_fails_closed() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    std::fs::write(root.path().join("actual.md"), "secret").expect("actual");
    symlink("actual.md", root.path().join("README.md")).expect("symlink");
    let error = RepositoryContextProvider::try_with_allowlist(root.path(), ["README.md"])
        .expect("provider")
        .collect(context(), request(ContextOverflowPolicy::Reject, 1_000))
        .await
        .expect_err("symlink must fail");
    assert_eq!(error.code(), REPOSITORY_PATH_UNSAFE);
}

#[cfg(unix)]
#[test]
fn malformed_allowlists_are_rejected() {
    let root = TempDir::new().expect("root");
    for allowlist in [
        vec!["README.md", "README.md"],
        vec!["./README.md"],
        vec!["nested//README.md"],
        vec!["nested/../README.md"],
    ] {
        assert!(
            RepositoryContextProvider::try_with_allowlist(root.path(), allowlist).is_err(),
            "allowlist should be rejected"
        );
    }
}
