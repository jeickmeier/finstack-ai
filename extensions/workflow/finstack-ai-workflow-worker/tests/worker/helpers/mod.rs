//! Shared fixtures copied from finstack-ai-workflow-local's executable spec.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
    ComponentId, ComponentRef, ContentBlock, DeadlinePropagation, Digest,
    Duration as KernelDuration, EffectId, EffectOutputContract, EffectOutputKind, EffectTag,
    ErrorCategory, ExternalEffectCompletion, ExternalEffectCompletionCommand,
    ExternalEffectOutcome, ExternalHandleRef, Id, IdTag, InteractionKind, InteractionRequest,
    InteractionTag, KernelInput, KernelState, Message, MessageRole, Metadata, OperationLocator,
    OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, ReconciliationPolicy,
    ReducerStageOutcome, RequestInteraction, RetryClassification, RetryDirective,
    RetrySafety, RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation,
    RunSecurityContext, Stage, StageCursor, TextBlock, Timestamp, TransitionEnv, Usage, Version,
};
use finstack_ai_runtime::{
    Clock, CommitCoordinator, EventHubConfig, ExternalClock, IdGenerationError, JournalStore,
    LockedModelContextProfile, Model, ModelContextProfile, ModelDeferral, ModelError,
    ModelName, ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings,
    ModelStreamItem, ModelStreamLimits, ModelTaskConfig, RandomSource, RunHandle, RunTaskConfig,
    RunTaskOwner, SameIdentityRetryPolicy, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    ToolSpec, WorkflowSession, WorkflowWait, classify_wait, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};

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

pub(crate) fn profile() -> ModelContextProfile {
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

pub(crate) fn locked_profile() -> LockedModelContextProfile {
    resolve_model_context_profile(profile(), None, None, false).expect("locked profile")
}

pub(crate) fn memory_store() -> Arc<MemoryJournalStore> {
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

pub(crate) struct CounterRandom(pub(crate) AtomicU64);

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
pub(crate) fn env(
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

pub(crate) fn accepted() -> RunAccepted {
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

pub(crate) fn user_message() -> Message {
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

pub(crate) fn stage(stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    stage_at(0, stage, outcome)
}

/// Same as [`stage`] but for an explicit model cycle, needed once a run has
/// looped back through `Stage::AfterToolBatch` and its cycle has advanced
/// past `0`.
pub(crate) fn stage_at(cycle: u64, stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle, stage },
        outcome,
    })
}

pub(crate) fn draft(messages: Arc<[Message]>, tools: Arc<[ToolSpec]>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: profile().model,
        messages,
        tools,
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

pub(crate) fn completed(text: &str) -> ModelResponse {
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

pub(crate) fn retryable_failure() -> ModelError {
    ModelError::try_new(
        "temporary_model_failure",
        ErrorCategory::Model,
        true,
        "temporary model failure",
        Metadata::empty(),
    )
    .expect("retryable error")
}

pub(crate) fn completed_plan(text: &str) -> ScriptedModelPlan {
    ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed(text)))),
        ],
    }
}

pub(crate) fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
}

pub(crate) async fn spawn_model_owner(
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

pub(crate) async fn drive_to_active_model_request(handle: &RunHandle) {
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
        draft(Arc::from([message]), Arc::from([]))
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

pub(crate) async fn drive_to_after_model(
    handle: &RunHandle,
    store: &Arc<MemoryJournalStore>,
    tools: Arc<[ToolSpec]>,
) {
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
        draft(Arc::from([message]), tools)
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
    wait_state(store, |state| state.phase == Some(RunPhase::AfterModel)).await;
}

pub(crate) async fn wait_state_on(
    store: Arc<dyn JournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(2), async {
        loop {
            let recovered = CommitCoordinator::recover(Arc::clone(&store), id(1))
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

pub(crate) async fn wait_state(
    store: &Arc<MemoryJournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    wait_state_on(Arc::clone(store) as Arc<dyn JournalStore>, predicate).await
}

pub(crate) async fn attach_session(
    store: Arc<MemoryJournalStore>,
    model: Arc<dyn Model>,
    clock: ExternalClock,
    seed: u64,
) -> WorkflowSession {
    WorkflowSession::trusted(store, locator(), clock, seed)
        .await
        .expect("attach")
        .with_ports(model, locked_profile(), None)
}

/// Scripted model whose single request defers to an external handle.
pub(crate) fn deferring_model() -> Arc<dyn Model> {
    Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
                ModelDeferral {
                    handle: ExternalHandleRef::try_new(
                        ComponentId::parse("finstack.model.scripted").expect("component"),
                        "job-1",
                        RawJson::parse(b"{}").expect("metadata"),
                    )
                    .expect("handle"),
                    reconciliation: ReconciliationPolicy::CallbackOnly,
                    next_poll_at: None,
                    expires_at: None,
                },
            )))],
        }],
    ))
}

/// Drive a fresh run onto a deferred external effect and hand back the
/// attached session, parked but not yet indexed.
///
/// Mirrors `deferred_survives_worker_restart` in `finstack-ai-workflow-local`.
pub(crate) async fn park_on_deferred_effect(
    store: &Arc<MemoryJournalStore>,
    model: &Arc<dyn Model>,
    clock: &ExternalClock,
    seed: u64,
) -> WorkflowSession {
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(model),
        clock.clone(),
        seed,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(store, |state| {
        state.phase == Some(RunPhase::AwaitingExternal)
    })
    .await;
    drop(owner);

    let mut session =
        attach_session(Arc::clone(store), Arc::clone(model), clock.clone(), seed).await;
    let wait = session.drive_until_wait().await.expect("deferred");
    assert!(
        matches!(wait, WorkflowWait::DeferredEffect { .. }),
        "expected a deferred wait, got {wait:?}"
    );
    session
}

/// Completion command settling `effect_id` on the fixture locator.
pub(crate) fn completion_command(effect_id: EffectId) -> ExternalEffectCompletionCommand {
    ExternalEffectCompletionCommand::try_new(
        locator(),
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        ExternalEffectCompletion::try_new(
            effect_id,
            "ext-1",
            ExternalEffectOutcome::Completed {
                output: RawJson::parse(r#"{"ok":true}"#).expect("output"),
                usage: None,
                artifacts: Arc::from([]),
            },
        )
        .expect("completion"),
    )
    .expect("command")
}

/// Drive a fresh run onto a retry timer and hand back the attached session.
///
/// Mirrors `timer_survives_worker_restart` in `finstack-ai-workflow-local`:
/// spawn an owner, drive to the model request, submit the `Retry` directive,
/// wait for `Sleeping`, drop the owner, then re-attach and drive until the
/// timer wait is classified. The returned session is parked but not yet
/// indexed — callers pass it to `park`.
pub(crate) async fn park_on_retry_timer(
    store: &Arc<MemoryJournalStore>,
    model: &Arc<dyn Model>,
    clock: &ExternalClock,
    seed: u64,
) -> WorkflowSession {
    let owner = spawn_model_owner(
        CommitCoordinator::new(store.clone()),
        Arc::clone(model),
        clock.clone(),
        seed,
    )
    .await;
    drive_to_active_model_request(&owner.handle()).await;
    wait_state(store, |state| state.phase == Some(RunPhase::BeforeFinalize)).await;
    owner
        .handle()
        .submit(
            env(2_300, &[7, 8, 9], &[3], &[4], &[], &[], &[], 105),
            stage(
                Stage::BeforeFinalize,
                ReducerStageOutcome::Retry(
                    RetryDirective::try_new(
                        RetryClassification::Model,
                        KernelDuration::from_millis(10),
                        "retry-v1",
                    )
                    .expect("directive"),
                ),
            ),
        )
        .await
        .expect("schedule retry");
    wait_state(store, |state| state.phase == Some(RunPhase::Sleeping)).await;
    drop(owner);

    let mut session =
        attach_session(Arc::clone(store), Arc::clone(model), clock.clone(), seed).await;
    let wait = session.drive_until_wait().await.expect("timer");
    assert!(
        matches!(wait, finstack_ai_runtime::WorkflowWait::Timer { .. }),
        "expected a timer wait, got {wait:?}"
    );
    session
}

/// Drive `session` past its currently classified wait, out of band from any
/// worker.
///
/// Unlike [`WorkflowSession::drive_until_wait`] — which returns the very
/// first wait it classifies, even one already recorded on the journal before
/// this session ever attached, without ever spawning an owner to advance
/// past it — this respawns the owner first (mirroring
/// `finstack-ai-workflow-worker`'s own `resume_row`, which calls
/// `respawn_owner` unconditionally before its poll loop) and only then polls
/// until a *different* wait is classified. Respawning unconditionally is
/// what actually lets an already-parked timer fire: the owner is what
/// checks the clock and commits `TimerFired`; `ensure_owner` alone would
/// never spawn one, since its own gate is "no owner and no classified
/// wait" — and a parked session always has a classified wait.
///
/// `timeout` is a "give up and fall back to the manual facade path" budget,
/// not a correctness bound — a case that never reaches a new classified
/// wait (e.g. one stuck on a genuine facade decision) always consumes the
/// whole timeout before returning `Err`. Do not shrink it for test speed;
/// that only trades a slower test for a flakier one.
///
/// # Errors
///
/// Returns `Err(())` when no new wait is classified within `timeout`.
pub(crate) async fn drive_past_current_wait(
    session: &mut WorkflowSession,
    timeout: StdDuration,
) -> Result<WorkflowWait, ()> {
    let initial = classify_wait(session.last_state());
    session.respawn_owner().await.expect("respawn owner");
    tokio::time::timeout(timeout, async {
        loop {
            session.ensure_owner().await.expect("ensure owner");
            if let Some(wait) = classify_wait(session.last_state())
                && Some(&wait) != initial.as_ref()
            {
                return wait;
            }
            tokio::time::sleep(StdDuration::from_millis(1)).await;
        }
    })
    .await
    .map_err(|_| ())
}

/// Commit an interaction request directly onto a freshly accepted run,
/// bypassing the model/tool-batch pipeline entirely.
///
/// `RequestInteraction` (`crates/finstack-ai-kernel/src/reducer/interaction.rs`)
/// only requires a requestable stage — `RunPhase::BeforeRun`, the phase right
/// after `AcceptRun`, qualifies — so no model or middleware chain is needed
/// to reach `WorkflowWait::Interaction`. Uses a bare, dispatcher-less
/// `CommitCoordinator`: both commits here (`AcceptRun`, `RequestInteraction`)
/// produce no `PostCommitAction`, so no host dispatch is ever required.
/// Returns the interaction id a caller then attaches a [`WorkflowSession`]
/// to and drives to its `WorkflowWait::Interaction`.
pub(crate) async fn request_interaction(
    store: &Arc<MemoryJournalStore>,
    now: Timestamp,
) -> finstack_ai_kernel::InteractionId {
    let dyn_store = Arc::clone(store) as Arc<dyn JournalStore>;
    let mut coordinator = CommitCoordinator::new(dyn_store);
    coordinator
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

    let interaction_id: Id<InteractionTag> = id(50);
    let effect_id: Id<EffectTag> = id(51);
    let request = InteractionRequest::try_new(
        1,
        interaction_id,
        effect_id,
        InteractionKind::Approval,
        vec![ContentBlock::Text(
            TextBlock::try_new("approve the next action").expect("prompt"),
        )],
        RawJson::parse(b"{}").expect("schema"),
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").expect("component"),
            None,
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
    .expect("interaction request");
    coordinator
        .submit(
            TransitionEnv {
                now,
                ids: AllocatedIds::try_new(
                    vec![id(2), id(3)],
                    vec![id(2), id(3)],
                    vec![effect_id],
                    vec![interaction_id],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![id(102)],
                    Vec::new(),
                )
                .expect("ids"),
            },
            KernelInput::RequestInteraction(RequestInteraction { request }),
        )
        .await
        .expect("request interaction");
    interaction_id
}

/// Drives a run parked on `RunPhase::PreparingContext` (mid-flight after a
/// `BeforeFinalize` retry timer has fired: the kernel restarts the model
/// cycle from context preparation, which is a genuine facade decision, not
/// something `RunTaskOwner` supplies on its own) forward through a full
/// model round to `RunPhase::BeforeFinalize`, out of band from any worker.
/// Mirrors the `PrepareContext -> BeforeModel` legs of
/// `drive_past_missing_facade_decisions` in `tests/worker/inbox_resume.rs`.
/// Leaves the run parked on `BeforeFinalize`; pair with
/// [`finalize_out_of_band`] to reach Terminal.
pub(crate) async fn continue_retry_cycle_out_of_band(
    store: &Arc<MemoryJournalStore>,
    model: &Arc<dyn Model>,
    clock: &ExternalClock,
    seed: u64,
) {
    let dyn_store = Arc::clone(store) as Arc<dyn JournalStore>;
    let recovered = CommitCoordinator::recover(Arc::clone(&dyn_store), locator().session_id)
        .await
        .expect("recover to continue the retried cycle");
    assert_eq!(
        recovered.state().phase,
        Some(RunPhase::PreparingContext),
        "continue_retry_cycle_out_of_band expects the retried cycle parked on PreparingContext"
    );
    let cycle = recovered.state().cycle;
    let messages: Arc<[Message]> = Arc::from(recovered.state().messages.as_slice());

    let owner = spawn_model_owner(recovered, Arc::clone(model), clock.clone(), seed).await;
    owner
        .handle()
        .submit(
            env(4_200, &[950, 951], &[], &[], &[952], &[], &[], 953),
            stage_at(
                cycle,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::clone(&messages),
                },
            ),
        )
        .await
        .expect("context prepared");

    let raw = RawJson::parse(
        draft(messages, Arc::from([]))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    owner
        .handle()
        .submit(
            env(4_300, &[954, 955], &[956], &[957], &[], &[958], &[], 959),
            stage_at(
                cycle,
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
                    deadline: Some(timestamp(9_000)),
                },
            ),
        )
        .await
        .expect("model request");
    wait_state(store, |state| {
        state.phase == Some(RunPhase::BeforeFinalize)
    })
    .await;
    drop(owner);
}

/// Commit the finalize acceptance a facade would normally supply, out of
/// band from any [`WorkflowSession`]/`RunTaskOwner`, using a bare,
/// dispatcher-less `CommitCoordinator` (mirrors [`request_interaction`]:
/// `FinalizeAccepted` at `Stage::BeforeFinalize` produces no
/// `PostCommitAction` once a terminal candidate is already recorded, so no
/// host dispatch is needed here either). The run must already be sitting on
/// `RunPhase::BeforeFinalize` with a terminal candidate — the state a
/// completed, tool-free model cycle reaches on its own once its retry timer
/// has fired.
pub(crate) async fn finalize_out_of_band(store: &Arc<MemoryJournalStore>) {
    let dyn_store = Arc::clone(store) as Arc<dyn JournalStore>;
    let mut coordinator = CommitCoordinator::recover(Arc::clone(&dyn_store), locator().session_id)
        .await
        .expect("recover for out-of-band finalize");
    assert_eq!(
        coordinator.state().phase,
        Some(RunPhase::BeforeFinalize),
        "finalize_out_of_band expects the run parked on BeforeFinalize"
    );
    let cycle = coordinator.state().cycle;
    coordinator
        .submit(
            env(3_000, &[900, 901], &[902], &[], &[], &[], &[], 903),
            stage_at(cycle, Stage::BeforeFinalize, ReducerStageOutcome::FinalizeAccepted),
        )
        .await
        .expect("finalize out of band");
    drop(coordinator);
    wait_state(store, |state| state.terminal.is_some()).await;
}
