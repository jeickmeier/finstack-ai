//! Public six-port helper coverage beyond the leaf Model/Toolset samples.

use std::sync::Arc;

use finstack_ai_kernel::{
    AppendBatchTag, AuthorizationEvidence, ComponentId, ComponentInvocation, ComponentRef, Digest,
    EffectTag, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget, Id, IdTag,
    InvocationRecovery, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION,
    RecordBody, RecordDraft, RecordTag, RunTag, SessionTag, Timestamp, Version,
};
use finstack_ai_kernel::{OperationLocator, RawJson, Stage};
use finstack_ai_runtime::{
    AuthorizationContext, CancellationSignal, ContextBudget, ContextCallContext,
    ContextContribution, ContextOverflowPolicy, ContextProviderDescriptor, ContextRequest,
    JournalStore, MiddlewareContext, MiddlewareDescriptor, MiddlewareOrder, MiddlewareRole,
    ObserverDescriptor, ObserverPayloadMode, OrderTier, RunCallContext, StageInput, StageMask,
    StageOutcome, StoreError,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    ContextConformanceCase, FaultJournalStore, JournalStoreConformanceCase,
    MiddlewareConformanceCase, ScriptedContextAction, ScriptedContextProvider, ScriptedMiddleware,
    ScriptedMiddlewareAction, ScriptedObserver, StoreOperation, check_context_conformance,
    check_journal_store_conformance, check_middleware_conformance, check_observer_conformance,
};

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn version() -> Version {
    Version {
        major: 1,
        minor: 0,
        patch: 0,
    }
}

fn invocation(component: &str) -> ComponentInvocation {
    ComponentInvocation {
        component: ComponentId::parse(component).expect("component"),
        version: version(),
        configuration_digest: Digest::raw_json(b"{}"),
        recovery: InvocationRecovery::RecomputeSafe,
    }
}

fn run_context() -> RunCallContext {
    RunCallContext {
        locator: OperationLocator::try_new(
            "tenant-a",
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            id::<RunTag>(3),
        )
        .expect("locator"),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                .expect("principal"),
            authentication_method: Arc::from("test"),
            assurance_level: Arc::from("test"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([Arc::from("tenant-a")]),
            safe_claims: finstack_ai_kernel::Metadata::empty(),
            policy_version: Arc::from("policy-v1"),
            decision_id: Arc::from("decision-v1"),
        },
        effect_id: id::<EffectTag>(4),
        attempt: 1,
        deadline: None,
        budget_scope_id: None,
        cancellation: CancellationSignal::new(),
    }
}

fn context_request() -> ContextRequest {
    ContextRequest {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        run_id: id::<RunTag>(3),
        user_input: Arc::from([]),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 8,
            max_tokens: 1_024,
            max_bytes: 4_096,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    }
}

#[tokio::test]
async fn context_middleware_and_observer_helpers_use_only_public_contracts() {
    let contribution =
        ContextContribution::try_new(Vec::new(), None::<&str>).expect("empty contribution");
    let context = ScriptedContextProvider::new(
        ContextProviderDescriptor {
            invocation: invocation("fixture.context"),
            trusted_application_instructions: false,
            metadata: finstack_ai_kernel::Metadata::empty(),
        },
        vec![ScriptedContextAction::Return(Ok(contribution.clone()))],
    );
    let context_result = check_context_conformance(
        &context,
        ContextConformanceCase {
            context: ContextCallContext {
                run: run_context(),
                provider_index: 0,
                chain_digest: Digest::raw_json(b"context-chain"),
            },
            request: context_request(),
            expected: contribution,
        },
    )
    .await
    .expect("context conformance");
    assert!(context_result.items.is_empty());

    let descriptor = MiddlewareDescriptor {
        invocation: invocation("fixture.middleware"),
        stages: StageMask::from_stages([Stage::BeforeRun]),
        order: MiddlewareOrder {
            tier: OrderTier::Standard,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        role: MiddlewareRole::Standard,
        metadata: finstack_ai_kernel::Metadata::empty(),
    };
    let middleware = ScriptedMiddleware::new(
        descriptor,
        vec![ScriptedMiddlewareAction::Return(Ok(StageOutcome::Continue))],
    );
    let outcome = check_middleware_conformance(
        &middleware,
        MiddlewareConformanceCase {
            context: MiddlewareContext {
                run: run_context(),
                chain_digest: Digest::raw_json(b"middleware-chain"),
                chain_index: 0,
                compaction_resume: None,
            },
            input: StageInput::BeforeRun {
                value: RawJson::parse(b"{}").expect("input"),
            },
            expected: StageOutcome::Continue,
        },
    )
    .await
    .expect("middleware conformance");
    assert_eq!(outcome, StageOutcome::Continue);

    let observer = ScriptedObserver::try_new(
        ObserverDescriptor {
            component: ComponentRef::new(
                ComponentId::parse("fixture.observer").expect("component"),
                Some(version()),
            ),
            payload_mode: ObserverPayloadMode::MetadataOnly,
            metadata: finstack_ai_kernel::Metadata::empty(),
        },
        8,
        Vec::new(),
    )
    .expect("observer");
    check_observer_conformance(&observer, Arc::from([]))
        .await
        .expect("observer conformance");
    assert_eq!(observer.batches().expect("batches").len(), 1);
}

#[tokio::test]
async fn conformance_failure_names_port_and_stable_contract() {
    let context = ScriptedContextProvider::new(
        ContextProviderDescriptor {
            invocation: invocation("fixture.context"),
            trusted_application_instructions: false,
            metadata: finstack_ai_kernel::Metadata::empty(),
        },
        Vec::new(),
    );
    let error = check_context_conformance(
        &context,
        ContextConformanceCase {
            context: ContextCallContext {
                run: run_context(),
                provider_index: 0,
                chain_digest: Digest::raw_json(b"context-chain"),
            },
            request: context_request(),
            expected: ContextContribution::try_new(Vec::new(), None::<&str>).expect("contribution"),
        },
    )
    .await
    .expect_err("script exhausted");
    assert_eq!(error.port, "ContextProvider");
    assert_eq!(error.contract, "context.collect.completed");
    assert!(error.to_string().contains("scripted_context_exhausted"));
}

fn store_draft() -> RecordDraft {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    let authorization =
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        "completion-1",
        ExternalCommandTarget::Effect(id::<EffectTag>(40)),
        principal,
        authorization,
        "conflicting_completion",
        Digest::raw_json(b"{}"),
        None,
    )
    .expect("rejection");
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(10),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        Some(id::<RunTag>(3)),
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Vec::new(),
        RecordBody::ExternalCommandRejected(rejection),
    )
    .expect("record")
}

#[tokio::test]
async fn journal_store_helper_proves_atomic_equal_retry_and_fault_injection() {
    let inner: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 2,
            batches_per_session: 4,
            records_per_session: 8,
            snapshot_bytes: 1_024,
        })
        .expect("store"),
    );
    let store = FaultJournalStore::new(inner);
    let request = finstack_ai_kernel::AppendRequest::try_new(
        id::<AppendBatchTag>(20),
        id::<SessionTag>(1),
        1,
        vec![store_draft()],
    )
    .expect("append");
    let committed = check_journal_store_conformance(
        &store,
        JournalStoreConformanceCase {
            request,
            expected_first_sequence: 1,
            expected_last_sequence: 1,
        },
    )
    .await
    .expect("store conformance");
    assert_eq!(committed.records.len(), 1);

    store.fail_next(
        StoreOperation::Health,
        StoreError::Unavailable {
            reason_code: "fixture_outage",
        },
    );
    let error = store.health().await.expect_err("injected fault");
    assert_eq!(error.code(), "store_unavailable");
}
