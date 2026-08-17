//! PR-047 multi-lane session APIs, concurrency, and lineage fan-out.

use std::path::PathBuf;
use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, BudgetPropagation, CancellationPropagation, ChildPlacement, ChildRunLocator,
    ContentBlock, DeadlinePropagation, Digest, Id, IdTag, Message, MessageRole, Metadata,
    OperationLocator, PrincipalPropagation, PrincipalRef, ProviderIds, RunAccepted, RunLimits,
    RunPropagationPolicy, RunRelation, RunRelationKind, RunSecurityContext, TextBlock, Timestamp,
    TransitionEnv,
};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, AuthorizationContext, ChildRunContext, ChildRunHandle,
    ChildRunRequest, JournalStore, LaneAppendIds, LaneCreateIds, PortFuture, SessionCreateIds,
    SessionRuntime,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

pub(crate) fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

pub(crate) fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

pub(crate) fn memory_store() -> Arc<dyn JournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 256,
            records_per_session: 1024,
            snapshot_bytes: 64 * 1024,
        })
        .expect("store"),
    )
}

pub(crate) fn create_ids(session: u64, lane: u64, batch: u64) -> SessionCreateIds {
    SessionCreateIds {
        session_id: id(session),
        main_lane_id: id(lane),
        session_created_record_id: id(batch + 1),
        lane_created_record_id: id(batch + 2),
        batch_id: id(batch),
        now: timestamp(1),
    }
}

pub(crate) fn lane_ids(lane: u64, batch: u64, fork: bool) -> LaneCreateIds {
    LaneCreateIds {
        lane_id: id(lane),
        lane_created_record_id: id(batch + 1),
        lane_moved_record_id: fork.then_some(id(batch + 2)),
        batch_id: id(batch),
        now: timestamp(2),
    }
}

pub(crate) fn append_ids(batch: u64) -> LaneAppendIds {
    LaneAppendIds {
        entry_record_id: id(batch + 1),
        lane_moved_record_id: id(batch + 2),
        batch_id: id(batch),
    }
}

pub(crate) fn text_message(ordinal: u64, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        MessageRole::User,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(0),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

pub(crate) fn cancel_env(now: i64, record: u64, batch: u64, request: u64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id(record)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(batch)],
            vec![id(request)],
        )
        .expect("cancel ids"),
    }
}

pub(crate) fn env(now: i64, record: u64, event: u64, batch: u64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id(record)],
            vec![id(event)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(batch)],
            Vec::new(),
        )
        .expect("ids"),
    }
}

pub(crate) fn security(decision: &str) -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
        "oidc",
        "high",
        "policy-v1",
        decision,
        None,
    )
    .expect("security")
}

pub(crate) fn propagation(cancellation: CancellationPropagation) -> RunPropagationPolicy {
    RunPropagationPolicy {
        cancellation,
        deadline: DeadlinePropagation::MinimumOfParentAndChild,
        budget: BudgetPropagation::SharedScope,
        principal: PrincipalPropagation::Inherit,
    }
}

pub(crate) fn root_acceptance(run: u64) -> RunAccepted {
    let run_id = id(run);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        security("decision-v1"),
        None,
        RunLimits::empty(),
        propagation(CancellationPropagation::Cascade),
        Digest::raw_json(br#"{"agent":"lanes"}"#),
        None,
    )
    .expect("acceptance")
}

pub(crate) fn child_acceptance(
    run: u64,
    parent: &RunAccepted,
    parent_effect: u64,
    cancellation: CancellationPropagation,
    decision: &str,
) -> RunAccepted {
    RunAccepted::try_new(
        id(run),
        RunRelation::try_new(
            parent.run_id(),
            Some(parent.run_id()),
            Some(id(parent_effect)),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("relation"),
        security(decision),
        None,
        RunLimits::empty(),
        propagation(cancellation),
        Digest::raw_json(br#"{"agent":"child"}"#),
        Some(parent),
    )
    .expect("child accepted")
}

pub(crate) async fn open_session(store: Arc<dyn JournalStore>) -> Arc<SessionRuntime> {
    SessionRuntime::create(store, "tenant-a", create_ids(1, 2, 100))
        .await
        .expect("create")
}

pub(crate) struct RecordingInvoker;

impl AgentInvoker for RecordingInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        let relation_digest =
            finstack_ai_runtime::child_relation_digest(&context, &request).expect("digest");
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

pub(crate) fn parent_context() -> ChildRunContext {
    ChildRunContext {
        parent: OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: id(1),
            lane_id: id(2),
            run_id: id(3),
        },
        parent_effect_id: id(44),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authentication_method: Arc::from("oidc"),
            assurance_level: Arc::from("high"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("policy-v1"),
            decision_id: Arc::from("decision-v1"),
        },
    }
}

pub(crate) fn child_request(
    placement: ChildPlacement,
    session: u64,
    lane: u64,
    run: u64,
) -> ChildRunRequest {
    ChildRunRequest {
        agent: finstack_ai_runtime::AgentRef {
            id: finstack_ai_kernel::AgentId::parse("finstack.agent.child").expect("agent"),
            bundle: None,
            spec_digest: Digest::raw_json(br#"{"agent":"child"}"#),
        },
        input: Arc::from([ContentBlock::Text(
            TextBlock::try_new("work").expect("text"),
        )]),
        placement,
        locator: ChildRunLocator {
            operation: OperationLocator {
                tenant_scope: Arc::from("tenant-a"),
                session_id: id(session),
                lane_id: id(lane),
                run_id: id(run),
            },
            remote: None,
        },
        requested_deadline: None,
        requested_budget: finstack_ai_kernel::BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    }
}

pub(crate) fn unique_sqlite_path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pr047-a04-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("journal.sqlite")
}
