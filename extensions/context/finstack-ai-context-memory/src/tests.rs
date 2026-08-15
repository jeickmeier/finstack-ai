use std::sync::{Arc, Mutex};

use finstack_ai_runtime::{
    ArtifactError, ArtifactId, ArtifactMetadata, ArtifactRef, ArtifactScope, ArtifactStore,
    AuthorizationContext, BlobRef, Bytes, CancellationSignal, ContentBlock, ContextAuthority,
    ContextBudget, ContextCallContext, ContextItemKind, ContextOverflowPolicy, ContextProvider,
    ContextRequest, Digest, EffectId, LaneId, OperationLocator, PortFuture, PrincipalRef,
    RunCallContext, RunId, Sensitivity, SessionId, TextBlock,
};

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

fn request(query: &str) -> ContextRequest {
    ContextRequest {
        session_id: id(1, |value| SessionId::parse(value).expect("session")),
        lane_id: id(2, |value| LaneId::parse(value).expect("lane")),
        run_id: id(3, |value| RunId::parse(value).expect("run")),
        user_input: Arc::from([ContentBlock::Text(
            TextBlock::try_new(query).expect("query"),
        )]),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 8,
            max_tokens: 1_000,
            max_bytes: 64 * 1024,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    }
}

#[derive(Clone, Default)]
struct CaptureArtifactStore {
    staged: Arc<Mutex<Vec<Bytes>>>,
}

impl ArtifactStore for CaptureArtifactStore {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>> {
        let staged = Arc::clone(&self.staged);
        Box::pin(async move {
            staged.lock().expect("lock").push(content.clone());
            let digest = Digest::blob_content(&content);
            let blob = BlobRef::try_new(
                "memory-blob",
                metadata.media_type.as_ref(),
                u64::try_from(content.len()).expect("len"),
                Some(digest),
                metadata.name.as_deref(),
            )
            .expect("blob");
            Ok(ArtifactRef::try_new(
                ArtifactId::from_bytes([3; 16]),
                metadata.kind.as_ref(),
                blob,
                digest,
                scope.digest()?,
                metadata.attributes,
            )
            .expect("artifact"))
        })
    }

    fn get(
        &self,
        _scope: ArtifactScope,
        _artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>> {
        Box::pin(async {
            Err(ArtifactError::NotFound {
                code: finstack_ai_runtime::ARTIFACT_NOT_FOUND,
            })
        })
    }
}

#[tokio::test]
async fn keyword_and_exact_id_retrieval_use_artifact_store() {
    let store = CaptureArtifactStore::default();
    let provider = MemoryContextProvider::try_with_store(Arc::new(store.clone()), "tenant-a")
        .expect("provider");
    provider
        .stage(
            &context(),
            "note-1",
            ["alpha", "ledger"],
            "remembered ledger note",
            Sensitivity::Internal,
        )
        .await
        .expect("stage");
    assert_eq!(store.staged.lock().expect("lock").len(), 1);

    let by_keyword = provider
        .collect(context(), request("please recall ledger"))
        .await
        .expect("keyword");
    assert_eq!(by_keyword.items.len(), 1);
    assert_eq!(by_keyword.items[0].kind, ContextItemKind::Reference);
    assert_eq!(by_keyword.items[0].authority, ContextAuthority::Untrusted);
    assert_eq!(
        by_keyword.items[0].provenance.source_ref.as_deref(),
        Some("note-1")
    );

    let by_id = provider
        .collect(context(), request("note-1"))
        .await
        .expect("id");
    assert_eq!(by_id.items.len(), 1);

    let miss = provider
        .collect(context(), request("unrelated"))
        .await
        .expect("miss");
    assert!(miss.items.is_empty());
}

#[tokio::test]
async fn foreign_tenant_cannot_collect_or_stage() {
    let provider = MemoryContextProvider::try_with_store(
        Arc::new(CaptureArtifactStore::default()),
        "tenant-b",
    )
    .expect("provider");
    let error = provider
        .stage(
            &context(),
            "note-1",
            ["alpha"],
            "secret",
            Sensitivity::Confidential,
        )
        .await
        .expect_err("stage denied");
    assert_eq!(
        error.code(),
        finstack_ai_runtime::CONTEXT_CONTRIBUTION_INVALID
    );
}
