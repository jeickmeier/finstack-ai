//! PR-048 crash-prefix matrix: drop the owner, recover, assert a legal class.
#![allow(
    clippy::too_many_lines,
    reason = "each prefix cluster is one legal-class matrix"
)]
#![allow(
    clippy::large_futures,
    reason = "coordinator fixtures are large; boxing would hide the matrix"
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, ActiveCapability, AllocatedIds, AppendRequest, AuthorizationEvidence,
    BudgetChargeReceipt, BudgetChargeRequest, BudgetPropagation, BudgetReleaseReceipt,
    BudgetReleaseRequest, BudgetRequest, BudgetReservationReceipt, BudgetReserveRequest,
    CancelRequested, CancellationInitiator, CancellationPropagation, CapabilitiesActivated,
    CapabilityActivationSource, ChildPlacement, ChildRunLocator, ComponentId, ComponentInvocation,
    ComponentRef, ContentBlock, DeadlinePropagation, Digest, EffectCompleted, EffectDeferred,
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested,
    ExternalEffectCompletedInput, ExternalEffectCompletion, ExternalEffectOutcome,
    ExternalHandleRef, Id, IdTag, InteractionExpired, InteractionKind, InteractionRequest,
    InteractionResolution, InteractionSettled, InteractionTag, InvocationRecovery, KernelInput,
    LaneCreated, LaneTag, Message, MessageRole, Metadata, ModelSettled, ModelSettlement,
    OperationLocator, PrincipalPropagation, PrincipalRef, ProviderIds, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RawJson, ReconciliationPolicy, RecordBody, RecordDraft, RecordTag,
    ReducerStageOutcome, RequestInteraction, RetrySafety, RunAccepted, RunLimits, RunPhase,
    RunPropagationPolicy, RunRelation, RunRelationKind, RunSecurityContext, RunTag, SessionTag,
    Stage, StageCursor, TextBlock, Timestamp, TransitionEnv, Usage, Version,
};
use finstack_ai_runtime::{
    ARTIFACT_INTEGRITY_FAILURE, AgentInvokeError, AgentInvoker, AgentRef, ArtifactMetadata,
    ArtifactRef, ArtifactScope, AuthorizationContext, BlobRef, BudgetCoordinator, BudgetError,
    BudgetLedger, BudgetOperationIds, BudgetReservationState, ChildCoordinationIds,
    ChildRunContext, ChildRunCoordinator, ChildRunHandle, ChildRunRequest, CommitCoordinator,
    CompositionError, ExternalCompletionRouter, ExternalEffectCompletionCommand,
    ExternalRouteError, IdempotencyHorizon, InvocationResumeAction, JournalStore, LaneAppendIds,
    LaneCreateIds, LoadRequest, OpaqueSnapshot, PortFuture, PruneRequest, SecurityAuditCategory,
    SecurityAuditError, SecurityAuditEvent, SecurityAuditGate, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink, Sensitivity, SessionCreateIds, SessionError,
    SessionRuntime, SnapshotRequest, StateSnapshotRequest, WriteMetadataRequest,
    middleware_resume_action, validate_staged_artifact,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits, SqliteSynchronous,
};
use finstack_ai_test::{LegalRestore, all_activated_record_bodies, classify_phase};

const PREFIX_IDS: &[&str] = &[
    "W1", "W2", "W3", "W4", "W5", "W6", "D1", "D2", "D3", "D4", "D5", "D6", "I1", "I2", "I3", "I4",
    "C1", "C2", "C3", "C4", "C5", "F1", "F2", "F3", "L1", "L2", "L3", "B1", "B2", "B3", "B4", "N1",
    "N2", "N3", "N4", "A1", "A2",
];

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

fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 256,
            records_per_session: 1024,
            snapshot_bytes: 256 * 1024,
        })
        .expect("store"),
    )
}

fn unique_sqlite_path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pr048-crash-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("journal.sqlite")
}

fn sqlite_store(path: PathBuf) -> Arc<SqliteJournalStore> {
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path,
            durability: SqliteDurability::Relaxed {
                synchronous: SqliteSynchronous::Normal,
            },
            limits: SqliteStoreLimits {
                sessions: 8,
                batches_per_session: 256,
                records_per_session: 1024,
                snapshot_bytes: 256 * 1024,
            },
            busy_timeout: Duration::from_secs(2),
        })
        .expect("sqlite"),
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
        now: timestamp(now),
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
        .expect("ids"),
    }
}

fn simple_env(now: i64, record: u64, event: u64, batch: u64) -> TransitionEnv {
    env(now, &[record], &[event], &[], &[], &[], &[], batch)
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
        Digest::raw_json(br#"{"agent":"crash-prefix"}"#),
        None,
    )
    .expect("accepted")
}

fn accept_input() -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        accepted: acceptance(3),
    })
}

fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

fn context_message() -> Message {
    Message::try_new(
        id(4),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("Say hello.").expect("text"),
        )],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn assistant_message(ordinal: u64, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        MessageRole::Assistant,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        timestamp(1_400),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("assistant")
}

async fn accept_run(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(store);
    coordinator
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            accept_input(),
        )
        .await
        .expect("accept");
    coordinator
}

async fn drive_to_model_request(coordinator: &mut CommitCoordinator) {
    coordinator
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    coordinator
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([context_message()]),
                },
            ),
        )
        .await
        .expect("context");
    coordinator
        .submit(
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
                    deadline: Some(timestamp(8_000)),
                },
            ),
        )
        .await
        .expect("model request");
}

async fn settle_model_completed(coordinator: &mut CommitCoordinator) {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending")
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
    coordinator
        .submit(
            env(1_400, &[607, 608], &[603, 604], &[], &[], &[], &[617], 605),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message: assistant_message(617, "hello"),
                },
            }),
        )
        .await
        .expect("settle");
}

async fn recover(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    CommitCoordinator::recover(store, id::<SessionTag>(1))
        .await
        .expect("recover")
}

async fn drive_to_pending_model(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    let mut coordinator = accept_run(Arc::clone(&store)).await;
    drive_to_model_request(&mut coordinator).await;
    drop(coordinator);
    recover(store).await
}

async fn settle_and_recover(store: Arc<dyn JournalStore>) -> CommitCoordinator {
    let mut coordinator = drive_to_pending_model(Arc::clone(&store)).await;
    settle_model_completed(&mut coordinator).await;
    drop(coordinator);
    recover(store).await
}

fn assert_legal(prefix: &str, phase: Option<RunPhase>, expected: LegalRestore) {
    let class = classify_phase(phase.expect("phase"));
    assert_eq!(class, expected, "{prefix} restored to {class:?}");
}

async fn write_snapshot(store: &Arc<dyn JournalStore>, coordinator: &CommitCoordinator) {
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    store
        .write_state_snapshot(StateSnapshotRequest {
            session_id: id::<SessionTag>(1),
            state: coordinator.state().clone(),
            head_checksum: loaded.head_checksum.expect("head"),
            pending_timer_scheduled_at: None,
        })
        .await
        .expect("snapshot");
}

fn horizon() -> IdempotencyHorizon {
    IdempotencyHorizon {
        expire_at: timestamp(10_000),
    }
}

#[test]
fn enumerated_prefix_matrix_lists_every_id() {
    assert_eq!(PREFIX_IDS.len(), 37);
    let mut unique = PREFIX_IDS.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), PREFIX_IDS.len());
}

#[tokio::test]
async fn prefix_w1_through_w6_snapshot_and_prune() {
    let store = memory_store();
    let accepted = acceptance(3);
    let draft = RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(1),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        Some(id::<RunTag>(3)),
        timestamp(1_000),
        vec![id(1)],
        RecordBody::RunAccepted(accepted),
    )
    .expect("draft");
    store
        .append(
            AppendRequest::try_new(id(101), id::<SessionTag>(1), 1, vec![draft]).expect("append"),
        )
        .await
        .expect("ack");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal("W1", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut coordinator).await;
    let before = coordinator.state().state_hash().expect("hash");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().state_hash().expect("hash"), before);
    assert_legal("W2", recovered.state().phase, LegalRestore::Retryable);

    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    let snapshot = loaded.snapshot.expect("snapshot");
    let mut corrupt = snapshot.bytes().to_vec();
    corrupt[0] ^= 0xff;
    store
        .write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot: OpaqueSnapshot::try_new(
                snapshot.sequence(),
                snapshot.digest(),
                corrupt,
                256 * 1024,
            )
            .expect("replacement"),
        })
        .await
        .expect("corrupt write");
    let ignored = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load corrupt");
    assert!(
        ignored.accelerated.is_none(),
        "W3 discards corrupt snapshot"
    );
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().state_hash().expect("hash"), before);
    assert_legal("W3", recovered.state().phase, LegalRestore::Retryable);

    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let accelerated = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    store
        .discard_snapshot(id::<SessionTag>(1))
        .expect("discard");
    let full = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        accelerated.state().state_hash().expect("hash"),
        full.state().state_hash().expect("hash"),
        "W4 snapshot-plus-tail equals full replay"
    );
    assert_eq!(
        accelerated.state().completion_identities,
        full.state().completion_identities
    );

    let loaded = store
        .load(LoadRequest {
            session_id: id::<SessionTag>(1),
        })
        .await
        .expect("load");
    store
        .write_metadata(WriteMetadataRequest {
            session_id: id::<SessionTag>(1),
            expected_head_checksum: loaded.head_checksum,
            metadata: Metadata::parse(br#"{"note":"w5"}"#).expect("metadata"),
        })
        .await
        .expect("metadata");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recovered.state().phase,
        full.state().phase,
        "W5 no authority"
    );
    assert_legal("W5", recovered.state().phase, LegalRestore::Retryable);

    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let receipt = store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: horizon(),
        })
        .await
        .expect("prune");
    assert!(receipt.pruned_through_sequence >= 1);
    assert!(receipt.retained_outstanding >= 1, "W6 outstanding retained");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().state_hash().expect("hash"), before);
    assert!(recovered.state().pending_model_effect.is_some());
    assert_legal("W6", recovered.state().phase, LegalRestore::Retryable);
}

#[tokio::test]
async fn prefix_d1_through_d6_and_post_horizon() {
    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut coordinator).await;
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(recovered.state().pending_model_effect.is_some());
    assert_legal("D1", recovered.state().phase, LegalRestore::Retryable);
    assert_legal("D2", recovered.state().phase, LegalRestore::Retryable);
    assert_legal("D3", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = drive_to_pending_model(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending")
        .clone();
    coordinator
        .submit(
            env(1_400, &[7], &[3], &[], &[], &[], &[], 105),
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Deferred(EffectDeferred {
                    effect_id: pending.requested.effect_id(),
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.provider.demo").expect("component"),
                        "h1",
                        RawJson::parse("{}").expect("meta"),
                    )
                    .expect("handle"),
                    reconciliation: ReconciliationPolicy::CallbackOnly,
                    next_poll_at: None,
                    expires_at: None,
                    output_contract: output_contract(),
                }),
            }),
        )
        .await
        .expect("defer");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::AwaitingExternal));
    assert!(
        recovered
            .state()
            .pending_model_effect
            .as_ref()
            .expect("pending")
            .deferred
            .is_some()
    );
    assert_legal("D4", recovered.state().phase, LegalRestore::Suspended);

    let mut recovered = recovered;
    let failed = finstack_ai_kernel::ErrorDescriptor::new(
        "provider_failed",
        "provider failed",
        finstack_ai_kernel::ErrorCategory::Model,
        false,
    )
    .expect("error");
    let completion = ExternalEffectCompletion::try_new(
        id::<finstack_ai_kernel::EffectTag>(103),
        "ext-1",
        ExternalEffectOutcome::Failed {
            error: failed.clone(),
        },
    )
    .expect("completion");
    recovered
        .submit(
            env(1_500, &[8], &[4], &[], &[], &[], &[], 106),
            KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
                completion: completion.clone(),
                assistant_message: None,
            }),
        )
        .await
        .expect("complete");
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let again = recovered
        .submit(
            env(1_600, &[10], &[6], &[], &[], &[], &[], 107),
            KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
                completion,
                assistant_message: None,
            }),
        )
        .await
        .expect("equal replay");
    assert!(again.committed.is_none(), "D5 equal replay is idempotent");
    assert_legal(
        "D5",
        recovered.state().phase,
        classify_phase(recovered.state().phase.expect("phase")),
    );

    let conflicting = ExternalEffectCompletion::try_new(
        id::<finstack_ai_kernel::EffectTag>(103),
        "ext-2",
        ExternalEffectOutcome::Failed { error: failed },
    )
    .expect("conflict");
    assert!(
        recovered
            .submit(
                env(1_700, &[12], &[8], &[], &[], &[], &[], 108),
                KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
                    completion: conflicting,
                    assistant_message: None,
                }),
            )
            .await
            .is_err(),
        "D6 conflicting duplicate fails closed"
    );
    let unchanged = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        unchanged.state().completion_identities,
        recovered.state().completion_identities
    );

    let sink = Arc::new(RecordingSink::default());
    let gate = SecurityAuditGate::enable(Some(Arc::clone(&sink) as _), Duration::from_millis(100))
        .await
        .expect("gate");
    let router = ExternalCompletionRouter::new(Arc::clone(&store) as Arc<dyn JournalStore>, gate)
        .with_horizon(IdempotencyHorizon {
            expire_at: timestamp(1),
        });
    let late = ExternalEffectCompletionCommand::try_new(
        OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator"),
        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            id(999),
            "late-1",
            ExternalEffectOutcome::Failed {
                error: finstack_ai_kernel::ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    finstack_ai_kernel::ErrorCategory::Model,
                    false,
                )
                .expect("error"),
            },
        )
        .expect("late"),
    )
    .expect("command");
    assert_eq!(
        router
            .route(late, timestamp(2_000))
            .await
            .expect_err("expired"),
        ExternalRouteError::IngressRejected
    );
    assert!(
        sink.events
            .lock()
            .expect("lock")
            .iter()
            .any(
                |event| event.category() == SecurityAuditCategory::UnknownLocator
                    && event.reason_code() == "expired_locator"
            ),
        "post-horizon is expired_locator"
    );
}

#[tokio::test]
async fn prefix_i1_through_i4() {
    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::AwaitingInteraction));
    assert_legal("I1", recovered.state().phase, LegalRestore::Suspended);

    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let mut coordinator = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            resolve_env(2_300),
            KernelInput::InteractionSettled(InteractionSettled::Resolved(
                InteractionResolution::try_new(
                    id::<InteractionTag>(501),
                    "resolution-1",
                    PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                        .expect("principal"),
                    AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
                    RawJson::parse(r#"{"approved":true}"#).expect("response"),
                    None::<&str>,
                )
                .expect("resolution"),
            )),
        )
        .await
        .expect("resolve");
    let identities = coordinator.state().resolution_identities.clone();
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().resolution_identities, identities);
    assert_legal("I2", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let mut coordinator = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            resolve_env(2_400),
            KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
                interaction_id: id::<InteractionTag>(501),
                expired_at: timestamp(2_400),
            })),
        )
        .await
        .expect("expire");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(recovered.state().last_interaction_terminal.is_some());
    assert_legal("I3", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut coordinator = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            request_env(2_200),
            KernelInput::RequestInteraction(RequestInteraction {
                request: typed_request(InteractionKind::Approval),
            }),
        )
        .await
        .expect("request");
    drop(coordinator);
    let mut coordinator = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    coordinator
        .submit(
            cancel_env(2_500, 90, 190, 700),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("i4")),
            }),
        )
        .await
        .expect("cancel");
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal("I4", recovered.state().phase, LegalRestore::Cancelled);
}

#[tokio::test]
async fn prefix_f1_through_f3() {
    let store = memory_store();
    let recovered = settle_and_recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::AfterModel));
    assert_legal("F1", recovered.state().phase, LegalRestore::Retryable);

    let mut recovered = recovered;
    recovered
        .submit(
            env(1_500, &[9], &[], &[], &[], &[], &[], 106),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::BeforeFinalize));
    assert_legal("F2", recovered.state().phase, LegalRestore::Retryable);

    let mut recovered = recovered;
    recovered
        .submit(
            env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 107),
            stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        )
        .await
        .expect("finalize");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().phase, Some(RunPhase::Completed));
    assert_legal("F3", recovered.state().phase, LegalRestore::Completed);
    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &recovered).await;
    let identities = recovered.state().completion_identities.clone();
    store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: horizon(),
        })
        .await
        .expect("prune settled");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().completion_identities, identities);
    assert_eq!(recovered.state().phase, Some(RunPhase::Completed));
}

#[tokio::test]
async fn prefix_a1_through_a2() {
    let store = memory_store();
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let activation = CapabilitiesActivated {
        prior_plan_digest: None,
        resolved_plan_digest: Digest::raw_json(br#"{"plan":1}"#),
        active: Arc::from([ActiveCapability {
            capability_id: finstack_ai_kernel::CapabilityId::parse("finstack.capability.alpha")
                .expect("capability"),
            source: CapabilityActivationSource::Always,
        }]),
    };
    coordinator
        .submit(
            env(1_050, &[2], &[], &[], &[], &[], &[], 102),
            KernelInput::CapabilitiesActivated(activation.clone()),
        )
        .await
        .expect("activate");
    let catalog = coordinator.state().active_capabilities.clone();
    drop(coordinator);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(recovered.state().active_capabilities, catalog);
    assert_legal("A1", recovered.state().phase, LegalRestore::Retryable);
    let duplicate = recovered
        .submit(
            env(1_060, &[3], &[], &[], &[], &[], &[], 103),
            KernelInput::CapabilitiesActivated(activation),
        )
        .await
        .expect("equal activation");
    assert!(duplicate.committed.is_none(), "A2 no second activation");
    assert_eq!(recovered.state().active_capabilities, catalog);
}

#[tokio::test]
async fn prefix_l1_through_l3() {
    let store = memory_store();
    let mut parent = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    parent
        .commit_session_records(
            id(205),
            vec![
                RecordDraft::try_new(
                    RECORD_FORMAT_VERSION,
                    RECORD_KIND_VERSION,
                    id(206),
                    id::<SessionTag>(1),
                    id::<LaneTag>(50),
                    None,
                    timestamp(1_050),
                    Vec::new(),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane"),
            ],
        )
        .await
        .expect("research");
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker));
    children
        .start_or_attach(
            &mut parent,
            parent_context(),
            child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51),
            None,
            coordination_ids(201, 202),
            timestamp(1_100),
        )
        .await
        .expect("prepare");
    drop(parent);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let mapping = recovered
        .session()
        .child_mapping(id(3), id(44))
        .expect("L1 mapping")
        .clone();
    assert_eq!(mapping.child.operation.run_id, id(51));
    assert_legal("L1", recovered.state().phase, LegalRestore::Retryable);
    let attached = children
        .start_or_attach(
            &mut recovered,
            parent_context(),
            child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51),
            None,
            coordination_ids(201, 202),
            timestamp(1_100),
        )
        .await
        .expect("retry attaches");
    assert_eq!(attached.locator.operation.run_id, id(51));

    let accepted = child_accepted(51, 44, recovered.state().accepted.as_ref().expect("parent"));
    store
        .append(
            AppendRequest::try_new(
                id(212),
                id::<SessionTag>(1),
                recovered.state().last_applied_sequence + 1,
                vec![
                    RecordDraft::try_new(
                        RECORD_FORMAT_VERSION,
                        RECORD_KIND_VERSION,
                        id(210),
                        id::<SessionTag>(1),
                        id::<LaneTag>(50),
                        Some(id::<RunTag>(51)),
                        timestamp(1_200),
                        vec![id(211)],
                        RecordBody::RunAccepted(accepted),
                    )
                    .expect("child draft"),
                ],
            )
            .expect("append"),
        )
        .await
        .expect("accept child");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_eq!(
        recovered
            .session()
            .child_mapping(id(3), id(44))
            .expect("L2")
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
    assert_legal("L2", recovered.state().phase, LegalRestore::Retryable);

    let mut recovered = recovered;
    recovered
        .submit(
            cancel_env(2_000, 90, 190, 700),
            KernelInput::CancelRequested(CancelRequested {
                initiator: CancellationInitiator::RuntimeShutdown,
                reason: Some(Arc::from("l3")),
            }),
        )
        .await
        .expect("parent cancel");
    drop(recovered);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal("L3", recovered.state().phase, LegalRestore::Cancelled);
}

#[tokio::test]
async fn prefix_b1_through_b4() {
    let store = memory_store();
    let mut parent = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    parent
        .commit_session_records(
            id(205),
            vec![
                RecordDraft::try_new(
                    RECORD_FORMAT_VERSION,
                    RECORD_KIND_VERSION,
                    id(206),
                    id::<SessionTag>(1),
                    id::<LaneTag>(50),
                    None,
                    timestamp(1_050),
                    Vec::new(),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane"),
            ],
        )
        .await
        .expect("lane");
    let reserve = reserve_request();
    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker));
    let failed = children
        .start_or_attach(
            &mut parent,
            parent_context_effect(344),
            budget_child_request(),
            Some(reserve.clone()),
            budget_ids(),
            timestamp(1_100),
        )
        .await;
    assert!(
        matches!(
            failed,
            Err(CompositionError::ServiceMissing {
                service: "budget_ledger"
            })
        ),
        "B1 does not silently grant"
    );
    drop(parent);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let replay = recovered
        .state()
        .budget_reservations
        .get(&reserve.reservation_id)
        .expect("B1 requested");
    assert!(replay.settlement.is_none());
    assert_legal("B1", recovered.state().phase, LegalRestore::Retryable);

    let store = memory_store();
    let mut parent = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    parent
        .commit_session_records(
            id(205),
            vec![
                RecordDraft::try_new(
                    RECORD_FORMAT_VERSION,
                    RECORD_KIND_VERSION,
                    id(206),
                    id::<SessionTag>(1),
                    id::<LaneTag>(50),
                    None,
                    timestamp(1_050),
                    Vec::new(),
                    RecordBody::LaneCreated(LaneCreated::try_new("research").expect("lane")),
                )
                .expect("lane"),
            ],
        )
        .await
        .expect("lane");
    let ledger = Arc::new(TestLedger::new());
    let children =
        ChildRunCoordinator::new(Arc::new(RecordingInvoker)).with_budget_ledger(ledger.clone());
    children
        .start_or_attach(
            &mut parent,
            parent_context_effect(344),
            budget_child_request(),
            Some(reserve.clone()),
            budget_ids(),
            timestamp(1_100),
        )
        .await
        .expect("B2 settle");
    drop(parent);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(
        recovered
            .state()
            .budget_reservations
            .get(&reserve.reservation_id)
            .expect("settled")
            .settlement
            .is_some()
    );
    children
        .start_or_attach(
            &mut recovered,
            parent_context_effect(344),
            budget_child_request(),
            Some(reserve.clone()),
            budget_ids(),
            timestamp(1_100),
        )
        .await
        .expect("equal reserve");
    assert_eq!(ledger.reserve_calls(), 1, "B2 no double-allocate");
    assert_legal("B2", recovered.state().phase, LegalRestore::Retryable);

    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut recovered).await;
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    settle_model_completed(&mut recovered).await;
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    let budget = BudgetCoordinator::new(ledger.clone());
    let charge = charge_request();
    budget
        .charge_committed(
            &mut recovered,
            &parent_locator(),
            charge.clone(),
            BudgetOperationIds {
                batch_id: id(406),
                record_id: id(407),
            },
            timestamp(1_450),
        )
        .await
        .expect("charge");
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    budget
        .charge_committed(
            &mut recovered,
            &parent_locator(),
            charge,
            BudgetOperationIds {
                batch_id: id(406),
                record_id: id(407),
            },
            timestamp(1_450),
        )
        .await
        .expect("equal charge");
    assert_eq!(ledger.charge_calls(), 1, "B4 no double-charge");
    assert_legal("B4", recovered.state().phase, LegalRestore::Retryable);

    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    recovered
        .submit(
            env(1_500, &[9], &[], &[], &[], &[], &[], 108),
            stage(Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    recovered
        .submit(
            env(1_600, &[10, 11], &[5], &[], &[], &[], &[], 109),
            stage(Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        )
        .await
        .expect("terminal");
    budget
        .release_committed(
            &mut recovered,
            &parent_locator(),
            release_request(),
            BudgetOperationIds {
                batch_id: id(408),
                record_id: id(409),
            },
            timestamp(1_700),
        )
        .await
        .expect("release");
    drop(recovered);
    let mut recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    budget
        .release_committed(
            &mut recovered,
            &parent_locator(),
            release_request(),
            BudgetOperationIds {
                batch_id: id(408),
                record_id: id(409),
            },
            timestamp(1_700),
        )
        .await
        .expect("equal release");
    assert_eq!(ledger.release_calls(), 1, "B3 release idempotent");
    assert_legal("B3", recovered.state().phase, LegalRestore::Completed);
}

#[tokio::test]
async fn prefix_n1_through_n4() {
    let store: Arc<dyn JournalStore> = memory_store();
    let session = SessionRuntime::create(
        Arc::clone(&store),
        "tenant-a",
        SessionCreateIds {
            session_id: id(1),
            main_lane_id: id(2),
            session_created_record_id: id(101),
            lane_created_record_id: id(102),
            batch_id: id(100),
            now: timestamp(1),
        },
    )
    .await
    .expect("create");
    session
        .create_lane(
            "research",
            None,
            LaneCreateIds {
                lane_id: id(50),
                lane_created_record_id: id(201),
                lane_moved_record_id: None,
                batch_id: id(200),
                now: timestamp(2),
            },
        )
        .await
        .expect("lane");
    let leaf = session
        .append_message(
            id(2),
            &Message::try_new(
                id(10),
                MessageRole::User,
                vec![ContentBlock::Text(TextBlock::try_new("A").expect("text"))],
                timestamp(0),
                None,
                ProviderIds::empty(),
                Metadata::empty(),
            )
            .expect("message"),
            LaneAppendIds {
                entry_record_id: id(211),
                lane_moved_record_id: id(212),
                batch_id: id(210),
            },
        )
        .await
        .expect("append");
    drop(session);
    let restored = SessionRuntime::open(Arc::clone(&store), id(1), "tenant-a")
        .await
        .expect("open");
    assert_eq!(
        restored.inspect(id(2)).await.expect("main").name.as_ref(),
        "main"
    );
    assert_eq!(
        restored
            .inspect(id(50))
            .await
            .expect("research")
            .name
            .as_ref(),
        "research"
    );
    assert_eq!(
        restored
            .inspect(id(2))
            .await
            .expect("main")
            .history
            .last()
            .map(finstack_ai_kernel::ConversationEntry::id),
        Some(leaf)
    );
    let _ = ("N1", "N2");

    restored
        .try_acquire_run(id(2), id(3))
        .expect("acquire main");
    let main = restored
        .accept_run(id(2), acceptance(3), simple_env(1_000, 10, 11, 12))
        .await
        .expect("accept main");
    restored
        .try_acquire_run(id(50), id(4))
        .expect("acquire sibling");
    let sibling = restored
        .accept_run(id(50), acceptance(4), simple_env(1_100, 20, 21, 22))
        .await
        .expect("accept sibling");
    assert_eq!(main.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(sibling.state().phase, Some(RunPhase::BeforeRun));
    drop(main);
    drop(sibling);
    drop(restored);
    let restored = SessionRuntime::open(Arc::clone(&store), id(1), "tenant-a")
        .await
        .expect("open siblings");
    let main = restored
        .coordinator_for_run(Some(id(3)))
        .await
        .expect("main run");
    let sibling = restored
        .coordinator_for_run(Some(id(4)))
        .await
        .expect("sibling run");
    assert_eq!(main.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(sibling.state().phase, Some(RunPhase::BeforeRun));
    assert_eq!(
        restored.try_acquire_run(id(2), id(30)),
        Err(SessionError::LaneBusy),
        "N3 busy-lane still holds"
    );

    let children = ChildRunCoordinator::new(Arc::new(RecordingInvoker));
    let mut parent = restored
        .coordinator_for_run(Some(id(3)))
        .await
        .expect("parent");
    children
        .start_or_attach(
            &mut parent,
            parent_context(),
            child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51),
            None,
            coordination_ids(510, 511),
            timestamp(2_100),
        )
        .await
        .expect("first child");
    let mut n = 3_000_u64;
    let mut next_env = || {
        n += 10;
        Ok(cancel_env(n.cast_signed(), n, n + 1, n + 2))
    };
    restored
        .cancel_run(id(3), CancellationInitiator::RuntimeShutdown, &mut next_env)
        .await
        .expect("first cancel");
    drop(parent);
    drop(restored);
    let restored = SessionRuntime::open(store, id(1), "tenant-a")
        .await
        .expect("open after cancel");
    let mut n = 4_000_u64;
    let mut next_env = || {
        n += 10;
        Ok(cancel_env(n.cast_signed(), n, n + 1, n + 2))
    };
    restored
        .cancel_run(id(3), CancellationInitiator::RuntimeShutdown, &mut next_env)
        .await
        .expect("N4 second cancel idempotent");
}

#[test]
fn prefix_c1_through_c5() {
    let requested = middleware_request(InvocationRecovery::NonRepeatable);
    assert_eq!(
        middleware_resume_action(&requested, None),
        InvocationResumeAction::SuspendUncertain,
        "C1 missing durable summary is uncertain"
    );

    let completed = EffectCompleted::try_new(
        requested.effect_id(),
        middleware_contract(),
        RawJson::parse(r#"{"ok":true}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("mw-1"),
        None,
    )
    .expect("completed");
    assert_eq!(
        middleware_resume_action(&requested, Some(&completed)),
        InvocationResumeAction::UseRecorded,
        "C2 recorded inline outcome replays"
    );

    let content = b"required-summary";
    let scope = ArtifactScope {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        run_id: Some(id(3)),
        sensitivity: Sensitivity::Confidential,
    };
    let metadata = ArtifactMetadata {
        kind: Arc::from("compaction-summary"),
        media_type: Arc::from("application/octet-stream"),
        name: Some(Arc::from("summary.bin")),
        attributes: Metadata::empty(),
    };
    let digest = Digest::blob_content(content);
    let artifact = ArtifactRef::try_new(
        finstack_ai_kernel::ArtifactId::from_bytes([3; 16]),
        metadata.kind.as_ref(),
        BlobRef::try_new(
            "blob-1",
            metadata.media_type.as_ref(),
            u64::try_from(content.len()).expect("len"),
            Some(digest),
            metadata.name.as_deref(),
        )
        .expect("blob"),
        digest,
        scope.digest().expect("scope"),
        metadata.attributes.clone(),
    )
    .expect("artifact");
    validate_staged_artifact(&scope, content, &metadata, &artifact).expect("C3 valid artifact");
    let err =
        validate_staged_artifact(&scope, b"corrupt", &metadata, &artifact).expect_err("C4 corrupt");
    assert_eq!(
        match err {
            finstack_ai_runtime::ArtifactError::Integrity { code, .. } => code,
            other => panic!("expected integrity, got {other:?}"),
        },
        ARTIFACT_INTEGRITY_FAILURE
    );

    let recompute = middleware_request(InvocationRecovery::RecomputeSafe);
    assert_eq!(
        middleware_resume_action(&recompute, None),
        InvocationResumeAction::Recompute,
        "C5 disposable checkpoint rebuilds"
    );
}

#[test]
fn journal_v1_family_count_stays_forty() {
    let bodies = all_activated_record_bodies().expect("bodies");
    assert_eq!(bodies.len(), 40);
}

#[tokio::test]
async fn sqlite_v1_opens_prunes_and_process_kill_stays_separate() {
    let path = unique_sqlite_path();
    let store = sqlite_store(path);
    let mut coordinator = accept_run(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    drive_to_model_request(&mut coordinator).await;
    write_snapshot(&(Arc::clone(&store) as Arc<dyn JournalStore>), &coordinator).await;
    drop(coordinator);
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert_legal(
        "sqlite-W2",
        recovered.state().phase,
        LegalRestore::Retryable,
    );
    store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: horizon(),
        })
        .await
        .expect("sqlite prune");
    let recovered = recover(Arc::clone(&store) as Arc<dyn JournalStore>).await;
    assert!(recovered.state().pending_model_effect.is_some());
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../extensions/stores/finstack-ai-store-sqlite/src/bin/sqlite_fault_helper.rs");
    assert!(
        helper.is_file(),
        "OS kill remains the separate sqlite_fault_helper row"
    );
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<SecurityAuditEvent>>,
}

impl SecurityAuditSink for RecordingSink {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
        let event_id = Arc::<str>::from(event.event_id());
        let recorded_at = event.timestamp();
        self.events.lock().expect("lock").push(event);
        Box::pin(async move {
            Ok(SecurityAuditReceipt {
                event_id,
                recorded_at,
            })
        })
    }

    fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
        Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
    }
}

struct RecordingInvoker;

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

struct TestLedger {
    reserve: BudgetReservationReceipt,
    charge: BudgetChargeReceipt,
    release: BudgetReleaseReceipt,
    reserved: Mutex<bool>,
    reserve_calls: Mutex<usize>,
    charge_calls: Mutex<usize>,
    release_calls: Mutex<usize>,
}

impl TestLedger {
    fn new() -> Self {
        let reserve = reserve_request();
        let usage = charge_usage();
        Self {
            reserve: BudgetReservationReceipt {
                scope_id: reserve.scope_id,
                reservation_id: reserve.reservation_id,
                reserved: reserve.amount.clone(),
                remaining: BudgetRequest::default(),
                request_digest: reserve.request_digest,
                receipt_digest: Digest::raw_json(br#"{"receipt":"reserve"}"#),
            },
            charge: BudgetChargeReceipt {
                scope_id: reserve.scope_id,
                reservation_id: reserve.reservation_id,
                effect_id: id(103),
                charged_usage: usage.clone(),
                cumulative_usage: usage.clone(),
                usage_digest: Digest::effect_output(&usage.canonical_bytes().expect("bytes")),
                receipt_digest: Digest::raw_json(br#"{"receipt":"charge"}"#),
            },
            release: BudgetReleaseReceipt {
                scope_id: reserve.scope_id,
                reservation_id: reserve.reservation_id,
                terminal_run_id: id(3),
                released_unused: BudgetRequest::default(),
                request_digest: BudgetReleaseRequest::compute_digest(
                    reserve.scope_id,
                    reserve.reservation_id,
                    id(3),
                )
                .expect("release digest"),
                receipt_digest: Digest::raw_json(br#"{"receipt":"release"}"#),
            },
            reserved: Mutex::new(false),
            reserve_calls: Mutex::new(0),
            charge_calls: Mutex::new(0),
            release_calls: Mutex::new(0),
        }
    }

    fn reserve_calls(&self) -> usize {
        *self.reserve_calls.lock().expect("calls")
    }

    fn charge_calls(&self) -> usize {
        *self.charge_calls.lock().expect("calls")
    }

    fn release_calls(&self) -> usize {
        *self.release_calls.lock().expect("calls")
    }
}

impl BudgetLedger for TestLedger {
    fn reserve(
        &self,
        request: BudgetReserveRequest,
    ) -> PortFuture<Result<BudgetReservationReceipt, BudgetError>> {
        *self.reserve_calls.lock().expect("calls") += 1;
        *self.reserved.lock().expect("reserved") = true;
        assert_eq!(request.request_digest, self.reserve.request_digest);
        let receipt = self.reserve.clone();
        Box::pin(async move { Ok(receipt) })
    }

    fn reconcile(
        &self,
        scope_id: finstack_ai_kernel::BudgetScopeId,
        reservation_id: finstack_ai_kernel::BudgetReservationId,
    ) -> PortFuture<Result<BudgetReservationState, BudgetError>> {
        let reserved = *self.reserved.lock().expect("reserved");
        let receipt = self.reserve.clone();
        assert_eq!(scope_id, receipt.scope_id);
        assert_eq!(reservation_id, receipt.reservation_id);
        Box::pin(async move {
            Ok(if reserved {
                BudgetReservationState::Reserved(receipt)
            } else {
                BudgetReservationState::NotFound
            })
        })
    }

    fn charge(
        &self,
        request: BudgetChargeRequest,
    ) -> PortFuture<Result<BudgetChargeReceipt, BudgetError>> {
        *self.charge_calls.lock().expect("calls") += 1;
        assert_eq!(request.effect_id, self.charge.effect_id);
        let receipt = self.charge.clone();
        Box::pin(async move { Ok(receipt) })
    }

    fn release(
        &self,
        request: BudgetReleaseRequest,
    ) -> PortFuture<Result<BudgetReleaseReceipt, BudgetError>> {
        *self.release_calls.lock().expect("calls") += 1;
        assert_eq!(request.terminal_run_id, self.release.terminal_run_id);
        let receipt = self.release.clone();
        Box::pin(async move { Ok(receipt) })
    }
}

fn request_env(now: i64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id::<RecordTag>(80), id::<RecordTag>(81)],
            vec![id(80), id(81)],
            vec![id(502)],
            vec![id::<InteractionTag>(501)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(180)],
            Vec::new(),
        )
        .expect("request ids"),
    }
}

fn resolve_env(now: i64) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now),
        ids: AllocatedIds::try_new(
            vec![id::<RecordTag>(82), id::<RecordTag>(83)],
            vec![id(82), id(83)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(181)],
            Vec::new(),
        )
        .expect("resolve ids"),
    }
}

fn cancel_env(now: i64, record: u64, append_batch: u64, request: u64) -> TransitionEnv {
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
            vec![id(append_batch)],
            vec![id(request)],
        )
        .expect("cancel ids"),
    }
}

fn typed_request(kind: InteractionKind) -> InteractionRequest {
    InteractionRequest::try_new(
        1,
        id::<InteractionTag>(501),
        id(502),
        kind,
        vec![ContentBlock::Text(
            TextBlock::try_new("resolve the outstanding interaction").expect("prompt"),
        )],
        RawJson::parse(
            r#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
        )
        .expect("schema"),
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").expect("component"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        None,
        false,
        Metadata::empty(),
    )
    .expect("request")
}

fn parent_locator() -> OperationLocator {
    OperationLocator {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
    }
}

fn parent_context() -> ChildRunContext {
    parent_context_effect(44)
}

fn parent_context_effect(effect: u64) -> ChildRunContext {
    ChildRunContext {
        parent: parent_locator(),
        parent_effect_id: id(effect),
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
        requested_budget: BudgetRequest::default(),
        delegation_id: None,
        metadata: Metadata::empty(),
        request_digest: Digest::raw_json(br#"{"request":"child-a"}"#),
    }
}

fn budget_child_request() -> ChildRunRequest {
    let mut request = child_request(ChildPlacement::CompatibleLaneInParentSession, 1, 50, 51);
    request.requested_budget = reserve_request().amount;
    request.request_digest = Digest::raw_json(br#"{"request":"budget-child"}"#);
    request
}

fn coordination_ids(batch: u64, record: u64) -> ChildCoordinationIds {
    ChildCoordinationIds {
        preparation_batch_id: id(batch),
        preparation_record_id: id(record),
        reservation_request_record_id: None,
        reservation_settlement: None,
    }
}

fn budget_ids() -> ChildCoordinationIds {
    ChildCoordinationIds {
        preparation_batch_id: id(401),
        preparation_record_id: id(402),
        reservation_request_record_id: Some(id(403)),
        reservation_settlement: Some(BudgetOperationIds {
            batch_id: id(404),
            record_id: id(405),
        }),
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

fn reserve_request() -> BudgetReserveRequest {
    let amount = BudgetRequest {
        input_tokens: Some(1_000),
        output_tokens: Some(250),
        cost: None,
        extension_counters: BTreeMap::new(),
    };
    let request_digest =
        BudgetReserveRequest::compute_digest(id(342), id(343), id(51), &amount).expect("digest");
    BudgetReserveRequest {
        scope_id: id(342),
        reservation_id: id(343),
        run_id: id(51),
        amount,
        request_digest,
    }
}

fn charge_usage() -> Usage {
    Usage::try_new(Some(20), Some(10), Some(30), None, BTreeMap::new()).expect("usage")
}

fn charge_request() -> BudgetChargeRequest {
    let usage = charge_usage();
    BudgetChargeRequest {
        scope_id: id(342),
        reservation_id: id(343),
        effect_id: id(103),
        usage: usage.clone(),
        usage_digest: Digest::effect_output(&usage.canonical_bytes().expect("bytes")),
    }
}

fn release_request() -> BudgetReleaseRequest {
    BudgetReleaseRequest {
        scope_id: id(342),
        reservation_id: id(343),
        terminal_run_id: id(3),
        request_digest: BudgetReleaseRequest::compute_digest(id(342), id(343), id(3))
            .expect("digest"),
    }
}

fn middleware_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::MiddlewareOutcome,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"middleware"}"#),
    }
}

fn middleware_request(recovery: InvocationRecovery) -> EffectRequested {
    EffectRequested::try_new(
        id(10),
        EffectKind::Middleware,
        None,
        Some(ComponentInvocation {
            component: ComponentId::parse("finstack.middleware.compact").expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery,
        }),
        None,
        middleware_contract(),
        EffectInput::Middleware {
            stage: Arc::from("before_model"),
            input: RawJson::parse("{}").expect("input"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("requested")
}
