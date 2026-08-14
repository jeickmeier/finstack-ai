//! PR-046 immutable conversation tree, main lane, and child-mapping restore.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, BudgetPropagation, CancellationPropagation,
    ChildPlacement, ChildRunLocator, ContentBlock, ContextPrepared, ConversationEntry,
    ConversationError, DeadlinePropagation, Digest, EntryId, Id, IdTag, KernelInput, LaneCreated,
    LaneMoved, LaneTag, Message, MessageRole, Metadata, OperationLocator, PrincipalPropagation,
    PrincipalRef, ProviderIds, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordDraft,
    RecordEnvelope, RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation,
    RunRelationKind, RunSecurityContext, SessionCreated, SessionTag, TextBlock, Timestamp,
    ToolCallBlock, ToolResultBlock, TransitionEnv,
};
use finstack_ai_runtime::{
    AgentInvokeError, AgentInvoker, AgentRef, AuthorizationContext, ChildCoordinationIds,
    ChildRunContext, ChildRunCoordinator, ChildRunHandle, ChildRunRequest, CommitCoordinator,
    CommitCoordinatorError, CompositionError, JournalStore, LoadRequest, PortFuture,
    SnapshotSchedule,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

fn store() -> Arc<MemoryJournalStore> {
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

fn env(now: i64, record: u64, event: u64, batch: u64) -> TransitionEnv {
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

fn security() -> RunSecurityContext {
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

fn propagation() -> RunPropagationPolicy {
    RunPropagationPolicy {
        cancellation: CancellationPropagation::Cascade,
        deadline: DeadlinePropagation::MinimumOfParentAndChild,
        budget: BudgetPropagation::SharedScope,
        principal: PrincipalPropagation::Inherit,
    }
}

fn acceptance(run: u64) -> RunAccepted {
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

fn text_message(ordinal: u64, role: MessageRole, text: &str) -> Message {
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

fn session_draft(record: u64, session: u64, lane: u64, body: RecordBody) -> RecordDraft {
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

async fn bootstrap(coordinator: &mut CommitCoordinator, session: u64, lane: u64, user: u64) {
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

async fn accept_parent(coordinator: &mut CommitCoordinator) {
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

struct RecordingInvoker {
    starts: Mutex<usize>,
}

impl AgentInvoker for RecordingInvoker {
    fn start_or_attach(
        &self,
        context: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>> {
        *self.starts.lock().expect("starts") += 1;
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

fn child_request(placement: ChildPlacement, session: u64, lane: u64, run: u64) -> ChildRunRequest {
    ChildRunRequest {
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
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    }
}

fn closed_tool_pair(parent: EntryId) -> (ConversationEntry, ConversationEntry) {
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

fn coordination_ids(batch: u64, record: u64) -> ChildCoordinationIds {
    ChildCoordinationIds {
        preparation_batch_id: id(batch),
        preparation_record_id: id(record),
        reservation_request_record_id: None,
        reservation_settlement: None,
    }
}

fn child_accepted(run: u64, parent_effect: u64, parent: &RunAccepted) -> RunAccepted {
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

fn parent_context() -> ChildRunContext {
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

#[tokio::test]
async fn parent_chain_is_immutable_and_equal_replay_is_idempotent() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store);
    bootstrap(&mut coordinator, 1, 2, 10).await;
    let user = coordinator
        .session()
        .entries()
        .values()
        .next()
        .expect("user")
        .clone();
    coordinator
        .commit_session_records(
            id(510),
            vec![session_draft(
                511,
                1,
                2,
                RecordBody::ConversationEntry(user.clone()),
            )],
        )
        .await
        .expect("equal replay");
    let rewritten = ConversationEntry::try_new(
        user.id(),
        Some(id(99)),
        user.lane_id(),
        0,
        user.body().clone(),
    )
    .expect("rewrite");
    assert!(matches!(
        coordinator
            .commit_session_records(
                id(512),
                vec![session_draft(
                    513,
                    1,
                    2,
                    RecordBody::ConversationEntry(rewritten)
                )],
            )
            .await,
        Err(CommitCoordinatorError::BoundaryFault {
            code: "session_records_invalid"
        })
    ));
    assert_eq!(
        coordinator
            .session()
            .entries()
            .get(&user.id())
            .expect("kept")
            .parent_id(),
        None
    );
}

#[tokio::test]
async fn branch_foundation_does_not_rewrite_the_shared_parent() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store);
    bootstrap(&mut coordinator, 1, 2, 10).await;
    let a = coordinator
        .session()
        .main_lane()
        .expect("main")
        .1
        .leaf_id
        .expect("leaf");
    let b_msg = text_message(11, MessageRole::Assistant, "b");
    let c_msg = text_message(12, MessageRole::User, "c");
    let d_msg = text_message(13, MessageRole::User, "d");
    let b = ConversationEntry::from_message(&b_msg, Some(a), id(2), 0).expect("b");
    let c = ConversationEntry::from_message(&c_msg, Some(b.id()), id(2), 0).expect("c");
    let d = ConversationEntry::from_message(&d_msg, Some(b.id()), id(2), 0).expect("d");
    coordinator
        .commit_session_records(
            id(520),
            vec![
                session_draft(521, 1, 2, RecordBody::ConversationEntry(b.clone())),
                session_draft(522, 1, 2, RecordBody::LaneMoved(LaneMoved::new(b.id()))),
            ],
        )
        .await
        .expect("b");
    coordinator
        .commit_session_records(
            id(523),
            vec![
                session_draft(524, 1, 2, RecordBody::ConversationEntry(c.clone())),
                session_draft(525, 1, 2, RecordBody::LaneMoved(LaneMoved::new(c.id()))),
            ],
        )
        .await
        .expect("c");
    coordinator
        .commit_session_records(
            id(526),
            vec![
                session_draft(527, 1, 2, RecordBody::ConversationEntry(d.clone())),
                session_draft(528, 1, 2, RecordBody::LaneMoved(LaneMoved::new(d.id()))),
            ],
        )
        .await
        .expect("d");
    assert_eq!(
        coordinator
            .session()
            .entries()
            .get(&b.id())
            .expect("b")
            .parent_id(),
        Some(a)
    );
    let history_d = coordinator.session().history(d.id()).expect("d");
    let history_c = coordinator.session().history(c.id()).expect("c");
    assert_eq!(
        history_d
            .iter()
            .map(ConversationEntry::id)
            .collect::<Vec<_>>(),
        vec![a, b.id(), d.id()]
    );
    assert_eq!(
        history_c
            .iter()
            .map(ConversationEntry::id)
            .collect::<Vec<_>>(),
        vec![a, b.id(), c.id()]
    );
}

#[tokio::test]
async fn drop_snapshot_leaves_the_conversation_tree() {
    let store = store();
    let mut coordinator =
        CommitCoordinator::new(store.clone()).with_snapshot_schedule(SnapshotSchedule {
            every_n_records: 1,
            write_timeout: Duration::from_millis(50),
        });
    bootstrap(&mut coordinator, 1, 2, 10).await;
    accept_parent(&mut coordinator).await;
    let before = coordinator.session().clone();
    store.discard_snapshot(id(1)).expect("discard");
    let loaded = store
        .load(LoadRequest { session_id: id(1) })
        .await
        .expect("load");
    assert!(loaded.snapshot.is_none());
    assert!(loaded.accelerated.is_none());
    let recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover");
    assert_eq!(recovered.session().entries(), before.entries());
    assert_eq!(
        recovered.session().main_lane().expect("main").1.leaf_id,
        before.main_lane().expect("main").1.leaf_id
    );
    assert_eq!(
        recovered.session().operations().keys().collect::<Vec<_>>(),
        before.operations().keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn main_lane_restores_leaf_and_rejects_a_second_root() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store.clone());
    bootstrap(&mut coordinator, 1, 2, 10).await;
    accept_parent(&mut coordinator).await;
    let leaf = coordinator
        .session()
        .main_lane()
        .expect("main")
        .1
        .leaf_id
        .expect("leaf");
    let recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover");
    assert_eq!(
        recovered.session().main_lane().expect("main").1.leaf_id,
        Some(leaf)
    );
    assert_eq!(
        recovered.state().accepted.as_ref().expect("run").run_id(),
        id(3)
    );
    assert_eq!(recovered.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(recovered.session().active_on_lane(id(2)), Some(id(3)));
    let mut recovered = recovered;
    assert!(matches!(
        recovered
            .submit(
                env(2_000, 20, 21, 22),
                KernelInput::AcceptRun(AcceptRun {
                    session_id: id::<SessionTag>(1),
                    lane_id: id::<LaneTag>(2),
                    accepted: acceptance(30),
                }),
            )
            .await,
        Err(CommitCoordinatorError::Decision { .. })
    ));
}

#[tokio::test]
async fn extract_history_keeps_tool_pairs_and_ignores_compaction() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store);
    bootstrap(&mut coordinator, 1, 2, 10).await;
    let user = coordinator
        .session()
        .main_lane()
        .expect("main")
        .1
        .leaf_id
        .expect("user");
    let (assistant_entry, tool_entry) = closed_tool_pair(user);
    coordinator
        .commit_session_records(
            id(530),
            vec![
                session_draft(
                    531,
                    1,
                    2,
                    RecordBody::ConversationEntry(assistant_entry.clone()),
                ),
                session_draft(532, 1, 2, RecordBody::ConversationEntry(tool_entry.clone())),
                session_draft(
                    533,
                    1,
                    2,
                    RecordBody::LaneMoved(LaneMoved::new(tool_entry.id())),
                ),
            ],
        )
        .await
        .expect("pair");
    let history = coordinator
        .session()
        .history(tool_entry.id())
        .expect("history");
    assert_eq!(history.len(), 3);
    let canonical = serde_json_canonicalizer::to_vec(&Vec::<Message>::new()).expect("json");
    let digest = Digest::domain_separated("model-context", 1, &canonical).expect("digest");
    let compacted = ContextPrepared::try_new(0, id(30), Vec::new(), digest).expect("context");
    let envelope = RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id(540),
        id(1),
        id(2),
        Some(id(3)),
        99,
        timestamp(0),
        None,
        Digest::raw_json(b"p"),
        None,
        Digest::raw_json(b"c"),
        Vec::new(),
        RecordBody::ContextPrepared(compacted),
    )
    .expect("envelope");
    let mut projection = coordinator.session().clone();
    projection.apply_envelope(&envelope).expect("ignore");
    assert_eq!(projection.history(tool_entry.id()).expect("still"), history);
    let split = assistant_entry.clone();
    let mut incomplete = BTreeMap::new();
    incomplete.insert(user, coordinator.session().entries()[&user].clone());
    incomplete.insert(split.id(), split.clone());
    assert_eq!(
        finstack_ai_kernel::extract_history(&incomplete, split.id()),
        Err(ConversationError::InvalidToolPair)
    );
}

#[tokio::test]
async fn pre046_journals_without_session_created_still_recover() {
    let store = store();
    let mut coordinator = CommitCoordinator::new(store.clone());
    accept_parent(&mut coordinator).await;
    let recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover");
    assert!(recovered.session().main_lane().is_none());
    assert_eq!(
        recovered.state().accepted.as_ref().expect("run").run_id(),
        id(3)
    );
}

async fn append_foreign_accept(
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

#[tokio::test]
async fn compatible_lane_child_mapping_survives_recover_and_rejects_remap() {
    let store = store();
    let mut parent = CommitCoordinator::new(store.clone());
    bootstrap(&mut parent, 1, 2, 10).await;
    accept_parent(&mut parent).await;
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker {
        starts: Mutex::new(0),
    }));
    let context = parent_context();
    let compatible = child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51);
    let ids = coordination_ids(201, 202);
    let first = children
        .start_or_attach(
            &mut parent,
            context.clone(),
            compatible.clone(),
            None,
            ids,
            timestamp(1_100),
        )
        .await
        .expect("prepare compatible");
    let accepted = child_accepted(51, 44, parent.state().accepted.as_ref().expect("parent"));
    append_foreign_accept(&store, &parent, accepted, 50, 210, 211, 212).await;
    let mut recovered = CommitCoordinator::recover(store, id(1))
        .await
        .expect("recover parent");
    assert_eq!(
        recovered
            .session()
            .child_mapping(id(3), id(44))
            .expect("mapping")
            .child
            .operation
            .run_id,
        id(51)
    );
    assert_eq!(
        recovered
            .session()
            .operations()
            .get(&id(51))
            .expect("child op")
            .relation
            .parent_effect_id(),
        Some(id(44))
    );
    let attached = children
        .start_or_attach(
            &mut recovered,
            context.clone(),
            compatible.clone(),
            None,
            ids,
            timestamp(1_100),
        )
        .await
        .expect("equal retry");
    assert_eq!(first, attached);
    let mut conflicting = compatible;
    conflicting.request_digest = Digest::raw_json(br#"{"request":"child-b"}"#);
    assert!(matches!(
        children
            .start_or_attach(
                &mut recovered,
                context,
                conflicting,
                None,
                ids,
                timestamp(1_100)
            )
            .await,
        Err(CompositionError::Commit(
            CommitCoordinatorError::SidecarConflict
        ))
    ));
}

#[tokio::test]
async fn isolated_child_session_mapping_matches_recovered_relation() {
    let store = store();
    let mut parent = CommitCoordinator::new(store.clone());
    bootstrap(&mut parent, 1, 2, 10).await;
    accept_parent(&mut parent).await;
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker {
        starts: Mutex::new(0),
    }));
    let isolated = child_request(ChildPlacement::IsolatedChildSession, 80, 81, 82);
    children
        .start_or_attach(
            &mut parent,
            ChildRunContext {
                parent_effect_id: id(45),
                ..parent_context()
            },
            isolated,
            None,
            coordination_ids(221, 222),
            timestamp(1_300),
        )
        .await
        .expect("prepare isolated");
    let isolated_accepted =
        child_accepted(82, 45, parent.state().accepted.as_ref().expect("parent"));
    let mut child_session = CommitCoordinator::new(store.clone());
    child_session
        .submit(
            env(3_000, 70, 71, 72),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(80),
                lane_id: id::<LaneTag>(81),
                accepted: isolated_accepted,
            }),
        )
        .await
        .expect("accept isolated");
    let parent_again = CommitCoordinator::recover(store.clone(), id(1))
        .await
        .expect("recover parent");
    let child_again = CommitCoordinator::recover(store, id(80))
        .await
        .expect("recover child");
    let isolated_map = parent_again
        .session()
        .child_mapping(id(3), id(45))
        .expect("isolated mapping");
    let child_relation = child_again
        .state()
        .accepted
        .as_ref()
        .expect("child run")
        .relation();
    assert_eq!(isolated_map.child.operation.run_id, id(82));
    assert_eq!(child_relation.parent_run_id(), Some(id(3)));
    assert_eq!(child_relation.parent_effect_id(), Some(id(45)));
    assert_eq!(child_relation.root_run_id(), id(3));
    assert_eq!(child_relation.depth(), 1);
}

#[test]
fn conversation_entry_is_structural_and_emits_zero_events() {
    let message = text_message(10, MessageRole::User, "hello");
    let entry = ConversationEntry::from_message(&message, None, id(2), 1).expect("entry");
    let body = RecordBody::ConversationEntry(entry);
    assert_eq!(body.kind_name(), "conversation_entry");
    assert!(body.is_structural());
    assert_eq!(
        body.derived_event_count(RECORD_KIND_VERSION)
            .expect("count"),
        0
    );
}
