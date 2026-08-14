//! Journal v1 historical corpus, checksum known-answers, and A09 fault injection.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AppendRequest, AuthorizationEvidence, BudgetPropagation,
    CancellationPropagation, DeadlinePropagation, Digest, ExternalCommandKind,
    ExternalCommandRejected, ExternalCommandTarget, Id, IdTag, KernelInput, LaneTag,
    PrincipalPropagation, PrincipalRef, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
    RecordDraft, RecordTag, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
    RunSecurityContext, RunTag, SessionTag, Timestamp, TransitionEnv,
};
use finstack_ai_runtime::{CommitCoordinator, JournalStore, LoadRequest, StoreError};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{
    AmbiguousAckAfterCommitStore, all_activated_record_bodies, discover_journal_v1_fixtures,
    draft_for_body, run_journal_v1_corpus, write_journal_v1_fixtures,
};

#[test]
fn journal_v1_corpus_is_byte_identical_and_covers_every_family() {
    if discover_journal_v1_fixtures().expect("discover").is_empty() {
        write_journal_v1_fixtures().expect("materialize first historical corpus");
    }
    let count = run_journal_v1_corpus().expect("journal v1 corpus");
    assert_eq!(
        count,
        39 * 2 + 11 + 10 + 4,
        "expected payload+envelope known-answers plus profile/limit/tamper fixtures, found {count}"
    );
    let bodies = all_activated_record_bodies().expect("bodies");
    assert_eq!(bodies.len(), 39, "every activated RecordBody family");
    let kinds = bodies.iter().map(RecordBody::kind_name).collect::<Vec<_>>();
    let mut unique = kinds.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique, kinds, "known-answers must be unique by kind_name");
}

#[test]
fn runtime_and_sdk_stay_protocol_free() {
    for package in ["finstack-ai-runtime", "finstack-ai"] {
        let output = std::process::Command::new("cargo")
            .args([
                "tree", "-p", package, "--prefix", "none", "-e", "normal", "--locked",
            ])
            .output()
            .expect("cargo tree");
        assert!(output.status.success(), "cargo tree {package} failed");
        let tree = String::from_utf8_lossy(&output.stdout);
        assert!(
            !tree
                .lines()
                .any(|line| line.starts_with("finstack-ai-protocol ")),
            "{package} must stay protocol-free:\n{tree}"
        );
    }
}

#[test]
fn ambiguous_ack_after_commit_retries_original_receipt_and_conflicts() {
    let inner = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 8,
            records_per_session: 16,
            snapshot_bytes: 1024,
        })
        .expect("store"),
    );
    let store = AmbiguousAckAfterCommitStore::once(inner.clone());
    let first_req = request(1, 1, 1, vec![draft(1, 1)]);
    assert!(matches!(
        block_on(store.append(first_req.clone())),
        Err(StoreError::AmbiguousAcknowledgement)
    ));
    let original = block_on(store.append(first_req.clone())).expect("retry receipt");
    assert_eq!(
        block_on(store.append(first_req)).expect("idempotent"),
        original
    );
    let concurrent = request(2, 1, 1, vec![draft(2, 1)]);
    assert!(matches!(
        block_on(store.append(concurrent)),
        Err(StoreError::Conflict { .. })
    ));
    let loaded = block_on(inner.load(LoadRequest {
        session_id: id::<SessionTag>(1),
    }))
    .expect("load");
    assert_eq!(loaded.head_sequence, 1);
    assert_eq!(loaded.head_checksum, Some(original.records[0].checksum()));
}

#[test]
fn coordinator_retries_ambiguous_ack_from_memory_store_once() {
    let inner = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 8,
            records_per_session: 16,
            snapshot_bytes: 1024,
        })
        .expect("store"),
    );
    let store = Arc::new(AmbiguousAckAfterCommitStore::once(inner.clone()));
    let mut coordinator = CommitCoordinator::new(store);
    let outcome = block_on(coordinator.submit(
        env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
        accept_input(),
    ))
    .expect("ambiguous recovery");
    let committed = outcome.committed.expect("accept committed");
    assert_eq!(committed.first_sequence, 1);
    let concurrent = request(2, 1, 1, vec![draft(2, 1)]);
    assert!(matches!(
        block_on(inner.append(concurrent)),
        Err(StoreError::Conflict { .. })
    ));
}

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

fn draft(record_ordinal: u64, session_ordinal: u64) -> RecordDraft {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    let authorization =
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        format!("completion-{record_ordinal}"),
        ExternalCommandTarget::Effect(id(record_ordinal + 1000)),
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
        id::<RecordTag>(record_ordinal),
        id::<SessionTag>(session_ordinal),
        id::<LaneTag>(session_ordinal + 100),
        Some(id::<RunTag>(session_ordinal + 200)),
        Timestamp::from_unix_ms(i64::try_from(record_ordinal).expect("timestamp"))
            .expect("timestamp"),
        Vec::new(),
        RecordBody::ExternalCommandRejected(rejection),
    )
    .expect("draft")
}

fn request(
    batch_ordinal: u64,
    session_ordinal: u64,
    expected_sequence: u64,
    drafts: Vec<RecordDraft>,
) -> AppendRequest {
    AppendRequest::try_new(
        id(batch_ordinal),
        id::<SessionTag>(session_ordinal),
        expected_sequence,
        drafts,
    )
    .expect("append request")
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
        .expect("acceptance"),
    })
}

#[allow(dead_code)]
fn _draft_for_body_link() {
    let _ = draft_for_body;
}
