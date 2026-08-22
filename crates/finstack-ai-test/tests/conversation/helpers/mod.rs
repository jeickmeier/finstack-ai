//! conversation-tree contract immutable conversation tree, main lane, and child-mapping restore.

use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancellationPropagation,
    ChildPlacement, ChildRunLocator, ContentBlock, ConversationEntry, DeadlinePropagation, Digest,
    EntryId, Id, IdTag, KernelInput, LaneCreated, LaneMoved, LaneTag, Message, MessageRole,
    Metadata, OperationLocator, PrincipalPropagation, PrincipalRef, ProviderIds,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft, RunAccepted, RunLimits,
    RunPropagationPolicy, RunRelation, RunRelationKind, RunSecurityContext, SessionCreated,
    SessionTag, TextBlock, Timestamp, ToolCallBlock, ToolResultBlock, TransitionEnv,
};
use finstack_ai_runtime::child::{
    AgentInvokeError, AgentInvoker, AgentRef, ChildCoordinationIds, ChildRunContext,
    ChildRunHandle, ChildRunRequest,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::AuthorizationContext;
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

pub(crate) fn store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 64,
            records_per_session: 256,
            snapshot_bytes: 64 * 1024,
        })
        .expect("store"),
    )
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

pub(crate) fn security() -> RunSecurityContext {
    RunSecurityContext::try_new(
        "tenant-a",
        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
        "oidc",
        "high",
        "policy-v1",
        "decision-v1",
        None,
    )
    .expect("security")
}

pub(crate) fn propagation() -> RunPropagationPolicy {
    RunPropagationPolicy {
        cancellation: CancellationPropagation::Cascade,
        deadline: DeadlinePropagation::MinimumOfParentAndChild,
        budget: BudgetPropagation::SharedScope,
        principal: PrincipalPropagation::Inherit,
    }
}

pub(crate) fn acceptance(run: u64) -> RunAccepted {
    let run_id = id(run);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        security(),
        None,
        RunLimits::empty(),
        propagation(),
        Digest::raw_json(br#"{"agent":"conversation"}"#),
        None,
    )
    .expect("acceptance")
}

pub(crate) fn text_message(ordinal: u64, role: MessageRole, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        role,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(0),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

pub(crate) fn session_draft(record: u64, session: u64, lane: u64, body: RecordBody) -> RecordDraft {
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id(record),
        id(session),
        id(lane),
        None,
        timestamp(0),
        Vec::new(),
        body,
    )
    .expect("draft")
}

pub(crate) async fn bootstrap(
    coordinator: &mut CommitCoordinator,
    session: u64,
    lane: u64,
    user: u64,
) {
    let message = text_message(user, MessageRole::User, "hello");
    let entry = ConversationEntry::from_message(&message, None, id(lane), 0).expect("entry");
    let leaf = entry.id();
    coordinator
        .commit_session_records(
            id(500),
            vec![
                session_draft(
                    501,
                    session,
                    lane,
                    RecordBody::SessionCreated(SessionCreated::new(Metadata::empty())),
                ),
                session_draft(
                    502,
                    session,
                    lane,
                    RecordBody::LaneCreated(LaneCreated::try_new("main").expect("lane")),
                ),
                session_draft(503, session, lane, RecordBody::ConversationEntry(entry)),
                session_draft(
                    504,
                    session,
                    lane,
                    RecordBody::LaneMoved(LaneMoved::new(leaf)),
                ),
            ],
        )
        .await
        .expect("bootstrap");
}

pub(crate) async fn accept_parent(coordinator: &mut CommitCoordinator) {
    coordinator
        .submit(
            env(1_000, 10, 11, 12),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: acceptance(3),
            }),
        )
        .await
        .expect("accept");
}

pub(crate) struct RecordingInvoker {
    pub(crate) starts: Mutex<usize>,
}

impl AgentInvoker for RecordingInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        *self.starts.lock().expect("starts") += 1;
        let relation_digest =
            finstack_ai_runtime::child::child_relation_digest(&context, &request).expect("digest");
        let locator = request.locator;
        Box::pin(async move {
            Ok(ChildRunHandle {
                locator,
                relation_digest,
            })
        })
    }
}

pub(crate) fn child_request(
    placement: ChildPlacement,
    session: u64,
    lane: u64,
    run: u64,
) -> ChildRunRequest {
    let mut request = ChildRunRequest {
        agent: AgentRef {
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
        request_digest: Digest::raw_json(br"null"),
    };
    request.request_digest = request.canonical_digest().expect("child request digest");
    request
}

pub(crate) fn closed_tool_pair(parent: EntryId) -> (ConversationEntry, ConversationEntry) {
    let call_id = id::<finstack_ai_kernel::ToolCallTag>(20);
    let call = ToolCallBlock::try_new(
        call_id,
        "lookup",
        finstack_ai_kernel::RawJson::parse(r#"{"q":1}"#).expect("json"),
    )
    .expect("call");
    let assistant = Message::try_new(
        id(11),
        MessageRole::Assistant,
        vec![ContentBlock::ToolCall(call)],
        timestamp(0),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant");
    let result = ToolResultBlock::try_new(
        call_id,
        vec![ContentBlock::Text(TextBlock::try_new("ok").expect("text"))],
        false,
    )
    .expect("result");
    let tool = Message::try_new(
        id(12),
        MessageRole::Tool,
        vec![ContentBlock::ToolResult(result)],
        timestamp(0),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("tool");
    let assistant_entry =
        ConversationEntry::from_message(&assistant, Some(parent), id(2), 0).expect("asst");
    let tool_entry =
        ConversationEntry::from_message(&tool, Some(assistant_entry.id()), id(2), 0).expect("tool");
    (assistant_entry, tool_entry)
}

pub(crate) fn coordination_ids(batch: u64, record: u64) -> ChildCoordinationIds {
    ChildCoordinationIds {
        preparation_batch_id: id(batch),
        preparation_record_id: id(record),
        reservation_request_record_id: None,
        reservation_settlement: None,
    }
}

pub(crate) fn child_accepted(run: u64, parent_effect: u64, parent: &RunAccepted) -> RunAccepted {
    RunAccepted::try_new(
        id(run),
        RunRelation::try_new(
            id(3),
            Some(id(3)),
            Some(id(parent_effect)),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("relation"),
        security(),
        None,
        RunLimits::empty(),
        propagation(),
        Digest::raw_json(br#"{"agent":"child"}"#),
        Some(parent),
    )
    .expect("child accepted")
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

pub(crate) async fn append_foreign_accept(
    store: &Arc<MemoryJournalStore>,
    parent: &CommitCoordinator,
    accepted: RunAccepted,
    lane: u64,
    record: u64,
    event: u64,
    batch: u64,
) {
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id(record),
        id(1),
        id(lane),
        Some(accepted.run_id()),
        timestamp(1_200),
        vec![id(event)],
        RecordBody::RunAccepted(accepted),
    )
    .expect("child draft");
    store
        .append(
            AppendRequest::try_new(
                id(batch),
                id(1),
                parent.state().last_applied_sequence + 1,
                vec![draft],
            )
            .expect("append request"),
        )
        .await
        .expect("append child");
}
