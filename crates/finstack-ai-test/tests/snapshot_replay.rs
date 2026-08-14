//! PR-041 snapshot discard, mismatch, and replay-equivalence proofs.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
    ContentBlock, DeadlinePropagation, Digest, EffectCompleted, EffectOutputContract,
    EffectOutputKind, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget, Id,
    IdTag, KernelInput, KernelState, LaneTag, Message, MessageRole, Metadata, ModelSettled,
    ModelSettlement, PrincipalPropagation, PrincipalRef, ProviderIds, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RawJson, RecordBody, RecordDraft, RecordTag, ReducerStageOutcome,
    RetrySafety, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation, RunSecurityContext,
    RunTag, SessionTag, Stage, StageCursor, TextBlock, Timestamp, ToolCallBlock, ToolCallTag,
    TransitionEnv,
};
use finstack_ai_protocol::{CanonicalValue, decode_value, encode_snapshot, encode_value};
use finstack_ai_runtime::{
    CommitCoordinator, JournalStore, LoadRequest, OpaqueSnapshot, SnapshotRequest,
    StateSnapshotRequest,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

fn block_on<T>(future: impl Future<Output = T>) -> T {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => thread::yield_now(),
        }
    }
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 128,
            records_per_session: 128,
            snapshot_bytes: 256 * 1024,
        })
        .expect("store"),
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the coordinator TransitionEnv fixture"
)]
fn env(
    now: i64,
    records: &[u64],
    events: &[u64],
    effects: &[u64],
    turns: &[u64],
    model_requests: &[u64],
    messages: &[u64],
    append_batch: u64,
) -> TransitionEnv {
    TransitionEnv {
        now: Timestamp::from_unix_ms(now).expect("ts"),
        ids: AllocatedIds::try_new(
            records.iter().copied().map(id).collect(),
            events.iter().copied().map(id).collect(),
            effects.iter().copied().map(id).collect(),
            Vec::new(),
            messages.iter().copied().map(id).collect(),
            turns.iter().copied().map(id).collect(),
            model_requests.iter().copied().map(id).collect(),
            Vec::new(),
            Vec::new(),
            vec![id(append_batch)],
            Vec::new(),
        )
        .expect("allocated ids"),
    }
}

fn accept_input() -> KernelInput {
    let run_id = id::<RunTag>(3);
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"agent":"fixture"}"#),
            None,
        )
        .expect("accepted"),
    })
}

fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

fn context_message() -> Message {
    Message::try_new(
        id(4),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("Say hello.").expect("text"),
        )],
        Timestamp::from_unix_ms(900).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

fn drive_to_model_request(store: Arc<MemoryJournalStore>) {
    let mut coordinator = CommitCoordinator::new(store);
    block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("accept");
    block_on(coordinator.submit(
        env(1_100, &[2], &[], &[], &[], &[], &[], 102),
        stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
    ))
    .expect("before run");
    block_on(coordinator.submit(
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        stage(
            Stage::PrepareContext,
            ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([context_message()]),
            },
        ),
    ))
    .expect("context");
    let outcome = block_on(
        coordinator.submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: RawJson::parse(
                        r#"{"messages":[{"role":"user","text":"Say hello."}]}"#,
                    )
                    .expect("request"),
                    component: None,
                    output_contract: output_contract(),
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(Timestamp::from_unix_ms(8_000).expect("deadline")),
                },
            ),
        ),
    )
    .expect("model request");
    assert!(outcome.committed.is_some());
}

fn settle_model(store: Arc<MemoryJournalStore>) {
    let mut coordinator = block_on(CommitCoordinator::recover(store, id::<SessionTag>(1)))
        .expect("recover pending model");
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending model")
        .clone();
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        output_contract(),
        RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("cmpl-1"),
        None,
    )
    .expect("completed");
    let assistant = Message::try_new(
        id(7),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        Timestamp::from_unix_ms(1_400).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant");
    block_on(coordinator.submit(
        env(1_400, &[7, 8], &[3, 4], &[], &[], &[], &[7], 105),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            outcome: ModelSettlement::Completed {
                completion,
                assistant_message: assistant,
            },
        }),
    ))
    .expect("settle");
}

fn write_current_snapshot(store: &Arc<MemoryJournalStore>) {
    let recovered = block_on(CommitCoordinator::recover(
        Arc::clone(store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("recover for snapshot");
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    let head_checksum = loaded.head_checksum.expect("head");
    block_on(store.write_state_snapshot(StateSnapshotRequest {
        session_id: id::<SessionTag>(1),
        state: recovered.state().clone(),
        head_checksum,
        pending_timer_scheduled_at: None,
    }))
    .expect("write snapshot");
}

fn append_tail(store: &Arc<MemoryJournalStore>, start_sequence: u64, count: u64) {
    for offset in 0..count {
        let sequence = start_sequence + offset;
        let principal =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
        let authorization =
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
        let rejection = ExternalCommandRejected::try_new(
            ExternalCommandKind::EffectCompletion,
            format!("completion-{sequence}"),
            ExternalCommandTarget::Effect(id(sequence + 1000)),
            principal,
            authorization,
            "conflicting_completion",
            Digest::raw_json(b"{}"),
            None,
        )
        .expect("rejection");
        let draft = RecordDraft::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            id::<RecordTag>(sequence + 50),
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            Some(id::<RunTag>(3)),
            Timestamp::from_unix_ms(i64::try_from(sequence).expect("ts")).expect("ts"),
            Vec::new(),
            RecordBody::ExternalCommandRejected(rejection),
        )
        .expect("draft");
        block_on(
            store.append(
                finstack_ai_kernel::AppendRequest::try_new(
                    id(sequence + 200),
                    id::<SessionTag>(1),
                    sequence,
                    vec![draft],
                )
                .expect("request"),
            ),
        )
        .expect("append tail");
    }
}

fn populate_settled_session(store: &Arc<MemoryJournalStore>) {
    drive_to_model_request(Arc::clone(store));
    settle_model(Arc::clone(store));
}

fn env_tools(
    now: i64,
    records: &[u64],
    events: &[u64],
    messages: &[u64],
    tool_calls: &[u64],
    append_batch: u64,
) -> TransitionEnv {
    TransitionEnv {
        now: Timestamp::from_unix_ms(now).expect("ts"),
        ids: AllocatedIds::try_new(
            records.iter().copied().map(id).collect(),
            events.iter().copied().map(id).collect(),
            Vec::new(),
            Vec::new(),
            messages.iter().copied().map(id).collect(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            tool_calls.iter().copied().map(id).collect(),
            vec![id(append_batch)],
            Vec::new(),
        )
        .expect("allocated ids"),
    }
}

fn settle_model_with_tool_call(store: Arc<MemoryJournalStore>) {
    let mut coordinator = block_on(CommitCoordinator::recover(store, id::<SessionTag>(1)))
        .expect("recover pending model");
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending model")
        .clone();
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        output_contract(),
        RawJson::parse(r#"{"text":"calling tools"}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("cmpl-tools"),
        None,
    )
    .expect("completed");
    let call = ToolCallBlock::try_new(
        id::<ToolCallTag>(301),
        "echo",
        RawJson::parse(r#"{"value":301}"#).expect("args"),
    )
    .expect("tool call");
    let assistant = Message::try_new(
        id(104),
        MessageRole::Assistant,
        vec![
            ContentBlock::Text(TextBlock::try_new("calling tools").expect("text")),
            ContentBlock::ToolCall(call),
        ],
        Timestamp::from_unix_ms(1_400).expect("ts"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant");
    block_on(coordinator.submit(
        env_tools(1_400, &[7, 8], &[3, 4], &[104], &[301], 105),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            outcome: ModelSettlement::Completed {
                completion,
                assistant_message: assistant,
            },
        }),
    ))
    .expect("settle tools");
}

fn populate_tool_session(store: &Arc<MemoryJournalStore>) {
    drive_to_model_request(Arc::clone(store));
    settle_model_with_tool_call(Arc::clone(store));
}

fn assert_snapshot_matches_full_replay(store: &Arc<MemoryJournalStore>) {
    write_current_snapshot(store);
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert!(loaded.accelerated.is_some());
    let snapshot_sequence = loaded.snapshot.expect("snapshot").sequence();
    append_tail(store, snapshot_sequence + 1, 8);

    let accelerated = block_on(CommitCoordinator::recover(
        Arc::clone(store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("snapshot plus tail");
    store
        .discard_snapshot(id::<SessionTag>(1))
        .expect("discard");
    let full = block_on(CommitCoordinator::recover(
        Arc::clone(store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("full replay");
    assert_eq!(
        accelerated.state().state_hash().expect("hash"),
        full.state().state_hash().expect("hash")
    );
    assert_eq!(
        accelerated.state().model_settlements,
        full.state().model_settlements
    );
    assert_eq!(
        accelerated.state().completion_identities,
        full.state().completion_identities
    );
    assert_eq!(
        accelerated.state().tool_settlements,
        full.state().tool_settlements
    );
}

fn replace_snapshot_bytes(store: &Arc<MemoryJournalStore>, bytes: Vec<u8>, digest: Digest) {
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    let snapshot = loaded.snapshot.expect("snapshot");
    block_on(
        store.write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: OpaqueSnapshot::try_new(snapshot.sequence(), digest, bytes, 256 * 1024)
                .expect("replacement"),
        }),
    )
    .expect("write replacement");
}

fn recover_hash(store: &Arc<MemoryJournalStore>) -> Digest {
    block_on(CommitCoordinator::recover(
        Arc::clone(store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("recover")
    .state()
    .state_hash()
    .expect("hash")
}

fn load_session(store: &Arc<MemoryJournalStore>) -> finstack_ai_runtime::LoadedSession {
    block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load")
}

fn assert_snapshot_ignored(store: &Arc<MemoryJournalStore>, expected: Digest) {
    assert!(load_session(store).accelerated.is_none());
    assert_eq!(recover_hash(store), expected);
}

#[test]
fn deleting_snapshots_still_recovers_from_the_journal() {
    let store = store();
    populate_settled_session(&store);
    write_current_snapshot(&store);
    let expected = recover_hash(&store);
    store
        .discard_snapshot(id::<SessionTag>(1))
        .expect("discard");
    let loaded = block_on(store.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert!(loaded.snapshot.is_none());
    assert!(loaded.accelerated.is_none());
    let rebuilt = block_on(CommitCoordinator::recover(store, id::<SessionTag>(1)))
        .expect("recover without snapshot");
    assert_eq!(rebuilt.state().state_hash().expect("hash"), expected);
}

#[test]
fn corrupt_and_mismatched_snapshots_are_ignored() {
    let store = store();
    populate_settled_session(&store);
    write_current_snapshot(&store);
    let expected = recover_hash(&store);

    let snapshot = load_session(&store).snapshot.expect("snapshot");
    let mut corrupt = snapshot.bytes().to_vec();
    corrupt[0] ^= 0xff;
    replace_snapshot_bytes(&store, corrupt, snapshot.digest());
    assert!(load_session(&store).snapshot.is_some());
    assert_snapshot_ignored(&store, expected);

    write_current_snapshot(&store);
    let snapshot = load_session(&store).snapshot.expect("snapshot");
    replace_snapshot_bytes(
        &store,
        snapshot.bytes().to_vec(),
        Digest::raw_json(b"wrong-digest"),
    );
    assert_snapshot_ignored(&store, expected);

    write_current_snapshot(&store);
    let snapshot = load_session(&store).snapshot.expect("snapshot");
    let mut value = decode_value(snapshot.bytes()).expect("value");
    if let CanonicalValue::Map(entries) = &mut value {
        for (key, item) in entries.iter_mut() {
            if *key == CanonicalValue::Text("format_version".into()) {
                *item = CanonicalValue::Unsigned(2);
            }
        }
    }
    replace_snapshot_bytes(
        &store,
        encode_value(&value).expect("tamper"),
        snapshot.digest(),
    );
    assert_snapshot_ignored(&store, expected);

    write_current_snapshot(&store);
    let recovered_state = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("state for fork")
    .state()
    .clone();
    let (forked_bytes, forked_digest) = encode_snapshot(
        &recovered_state,
        recovered_state.last_applied_sequence,
        Digest::raw_json(b"forked-head"),
        None,
    )
    .expect("forked envelope");
    replace_snapshot_bytes(&store, forked_bytes, forked_digest);
    assert!(load_session(&store).accelerated.is_some());
    assert_eq!(recover_hash(&store), expected);
}

#[test]
fn empty_session_replay_matches_default_state() {
    let store = store();
    let recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("empty recover");
    assert_eq!(
        recovered.state().state_hash().expect("hash"),
        KernelState::default().state_hash().expect("hash")
    );
    assert!(recovered.state().model_settlements.is_empty());
    assert!(recovered.state().completion_identities.is_empty());
}

#[test]
fn snapshot_plus_tail_matches_full_replay_hashes_and_settlements() {
    let store = store();
    populate_settled_session(&store);
    assert_snapshot_matches_full_replay(&store);
    assert!(
        !block_on(CommitCoordinator::recover(
            Arc::clone(&store) as Arc<dyn JournalStore>,
            id::<SessionTag>(1),
        ))
        .expect("v1")
        .state()
        .model_settlements
        .is_empty()
    );
}

#[test]
fn tool_bearing_snapshot_plus_tail_matches_full_replay() {
    let store = store();
    populate_tool_session(&store);
    let recovered = block_on(CommitCoordinator::recover(
        Arc::clone(&store) as Arc<dyn JournalStore>,
        id::<SessionTag>(1),
    ))
    .expect("tool session");
    assert!(recovered.state().state_version >= 2);
    assert_snapshot_matches_full_replay(&store);
}
