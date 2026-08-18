//! PR-059-A01: Temporal-shaped worker does not reimplement the model loop.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, ContentBlock,
    DeadlinePropagation, Digest, EffectId, EffectOutputContract, EffectOutputKind, Id, IdTag,
    KernelInput, KernelState, Message, MessageRole, Metadata, OperationLocator, OutputSpec,
    PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, RecordBody, ReducerStageOutcome,
    RetrySafety, RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation,
    RunSecurityContext, Stage, StageCursor, TextBlock, Timestamp, TransitionEnv, Usage,
};
use finstack_ai_runtime::{
    Clock, CommitCoordinator, EventHubConfig, ExternalClock, IdGenerationError, JournalStore,
    LoadRequest, LockedModelContextProfile, ManualDriveAction, Model, ModelContextProfile,
    ModelName, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamItem, ModelStreamLimits, ModelTaskConfig, RandomSource, RunHandle, RunTaskConfig,
    RunTaskOwner, SameIdentityRetryPolicy, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    WorkflowSession, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_temporal::TemporalShapedWorker;

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

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: Arc::from("scripted"),
        model: ModelName::try_new("scripted-1").expect("model"),
        hard_input_bytes: 2_000_000,
        context_window_tokens: 3_000_000,
        max_output_tokens: 1_000,
        reserved_output_tokens: 1_000,
        provider_overhead_tokens: 0,
        estimator: TokenEstimatorRef {
            id: Arc::from("scripted-bytes-upper-bound"),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}

fn locked_profile() -> LockedModelContextProfile {
    resolve_model_context_profile(profile(), None, None, false).expect("locked profile")
}

fn memory_store() -> Arc<MemoryJournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 128,
            records_per_session: 512,
            snapshot_bytes: 4_096,
        })
        .expect("store"),
    )
}

struct CounterRandom(AtomicU64);

impl RandomSource for CounterRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), IdGenerationError> {
        for chunk in buf.chunks_mut(8) {
            let value = self.0.fetch_add(1, Ordering::AcqRel).to_be_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
        Ok(())
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors TransitionEnv allocation used by model_port proofs"
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

fn accepted() -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
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
        Digest::raw_json(b"agent"),
        None,
    )
    .expect("accepted")
}

fn user_message() -> Message {
    Message::try_new(
        id(44),
        MessageRole::User,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        timestamp(900),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle: 0, stage },
        outcome,
    })
}

fn draft(messages: Arc<[Message]>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: profile().model,
        messages,
        tools: Arc::from([]),
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 2_000_000,
            max_input_tokens: 2_000_000,
            max_output_tokens: 1_000,
        },
    }
}

fn completed(text: &str) -> ModelResponse {
    ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new(text).expect("text"))]),
        tool_calls: Arc::from([]),
        usage: Usage::empty(),
        provider_ids: ProviderIds::try_new(None::<&str>, Some("response-1"), None::<&str>)
            .expect("provider ids"),
        completion_id: Arc::from("completion-1"),
        continuation_state: None,
    }
}

fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
}

async fn spawn_model_owner(
    coordinator: CommitCoordinator,
    model: Arc<dyn Model>,
    clock: impl Clock + Send + Sync + 'static,
    random: u64,
) -> RunTaskOwner {
    RunTaskOwner::spawn_with_model(
        coordinator,
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        model,
        locked_profile(),
        clock,
        CounterRandom(AtomicU64::new(random)),
    )
    .await
    .expect("owner")
}

async fn drive_to_active_model_request(handle: &RunHandle) {
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id(1),
                lane_id: id(2),
                accepted: accepted(),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage(Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage(
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage(
                Stage::BeforeModel,
                ReducerStageOutcome::ModelRequestPrepared {
                    request: raw,
                    component: None,
                    output_contract: EffectOutputContract {
                        kind: EffectOutputKind::ModelResponse,
                        schema_version: 1,
                        schema_digest: Digest::raw_json(b"model-response"),
                    },
                    retry_safety: RetrySafety::SafeToRetry,
                    deadline: Some(timestamp(5_000)),
                },
            ),
        )
        .await
        .expect("model request");
}

async fn wait_state(
    store: &Arc<MemoryJournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let recovered = CommitCoordinator::recover(store.clone(), id(1))
                .await
                .expect("recover");
            if predicate(recovered.state()) {
                return recovered;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("state wait")
}

async fn journal_trace(store: &Arc<MemoryJournalStore>) -> Vec<(String, Option<EffectId>)> {
    let loaded = store
        .load(LoadRequest { session_id: id(1) })
        .await
        .expect("load");
    let mut rows = Vec::new();
    for batch in loaded.committed_batches.iter() {
        for envelope in batch.records.iter() {
            rows.push(match envelope.body() {
                RecordBody::EffectRequested(requested) => {
                    ("effect_requested".into(), Some(requested.effect_id()))
                }
                RecordBody::EffectCompleted(completed) => {
                    ("effect_completed".into(), Some(completed.effect_id()))
                }
                RecordBody::RunAccepted(_) => ("run_accepted".into(), None),
                RecordBody::StageOutcomeRecorded(_) => ("stage_outcome".into(), None),
                RecordBody::ContextPrepared(_) => ("context_prepared".into(), None),
                other => (
                    format!("{other:?}")
                        .split('(')
                        .next()
                        .unwrap_or("other")
                        .into(),
                    None,
                ),
            });
        }
    }
    rows
}

#[tokio::test]
async fn temporal_worker_matches_direct_owner_journal() {
    async fn run(through_adapter: bool) -> Vec<(String, Option<EffectId>)> {
        let store = memory_store();
        let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
            profile(),
            vec![ScriptedModelPlan {
                actions: vec![
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                        text: Arc::from("hello"),
                    }))),
                    ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("hello")))),
                ],
            }],
        ));
        let clock = ExternalClock::new(timestamp(2_000));
        let mut coordinator = CommitCoordinator::new(store.clone());
        let mut controller = coordinator.enable_manual_drive(1).expect("manual drive");
        let owner = spawn_model_owner(coordinator, Arc::clone(&model), clock.clone(), 500).await;
        let handle = owner.handle();
        let drive = tokio::spawn(async move { drive_to_active_model_request(&handle).await });
        let permit = tokio::time::timeout(StdDuration::from_secs(2), controller.next_effect())
            .await
            .expect("pause bound")
            .expect("paused execute");
        assert_eq!(permit.effect().action, ManualDriveAction::Execute);
        wait_state(&store, |state| state.pending_model_effect.is_some()).await;
        if through_adapter {
            drop(permit);
            drive.abort();
            let _ = drive.await;
            drop(owner);
            let session = WorkflowSession::trusted(store.clone(), locator(), clock, 500)
                .await
                .expect("attach")
                .with_ports(model, locked_profile(), None);
            let mut worker = TemporalShapedWorker::wrap(session, 5);
            worker.session_mut().ensure_owner().await.expect("spawn");
            wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
        } else {
            permit.continue_dispatch();
            drive.await.expect("drive");
            wait_state(&store, |state| state.phase == Some(RunPhase::AfterModel)).await;
            drop(owner);
        }
        journal_trace(&store).await
    }

    let direct = Box::pin(run(false)).await;
    let adapted = Box::pin(run(true)).await;
    assert_eq!(direct, adapted, "A01 temporal loop parity");
    assert!(
        direct.iter().any(|(kind, _)| kind == "effect_requested"),
        "model effect present"
    );
}
