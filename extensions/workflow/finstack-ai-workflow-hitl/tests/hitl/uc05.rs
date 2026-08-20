//! UC-05 end to end, across a process-death boundary, on `SQLite`.
//!
//! Both tests run the whole sentence — a run parks on a tool-approval
//! interaction, the process dies, a fresh host rebuilds its stores from
//! paths alone, an operator (or the expiry sweep) settles the interaction,
//! and the very same worker drives the run to a terminal state — with every
//! timestamp supplied explicitly and no wall clock anywhere.
//!
//! ## Why a facade stand-in appears between the two ticks
//!
//! Settling an interaction that parked inside `Stage::BeforeToolBatch`
//! unconditionally lands the run's kernel phase on `RunPhase::AfterToolBatch`
//! (`crates/finstack-ai-kernel/src/reducer/apply/tools.rs::apply_tool_batch_closed`),
//! and advancing past that phase requires an externally submitted
//! `KernelInput::StageSettled` decision that no worker — and no
//! `RunTaskOwner` — makes on its own. This is documented at length in
//! `finstack-ai-workflow-worker`'s own end-to-end spec
//! (`tests/worker/inbox_resume.rs`), whose
//! `interaction_resolution_delivered_while_down_resumes_on_tick` has exactly
//! the same three-act shape: a first tick that applies the response but
//! cannot park (a counted failure with the response preserved), a stand-in
//! for the missing application-level facade, and a second tick that reaches
//! `sessions_resumed == 1`. These tests reuse that shape rather than
//! pretending a single tick can do both halves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration as StdDuration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
    ContentBlock, DeadlinePropagation, Digest, EffectOutputContract, EffectOutputKind,
    InteractionRequest, KernelInput, KernelState, Message, MessageRole, Metadata, OperationLocator,
    OutputSpec, PrincipalPropagation, PrincipalRef, ProviderIds, RawJson, ReducerStageOutcome,
    RetrySafety, RunAccepted, RunLimits, RunPhase, RunPropagationPolicy, RunRelation,
    RunSecurityContext, Stage, StageCursor, TextBlock, Timestamp, ToolExecutionMode,
    ToolFailurePolicy, ToolId, TransitionEnv, Usage,
};
use finstack_ai_runtime::{
    ApprovalGrantMode, ApprovalMetadata, ApprovalRequirement, CommitCoordinator, EventHubConfig,
    ExternalClock, IdGenerationError, JournalStore, JsonSchemaToolValidatorCompiler,
    LockedModelContextProfile, Model, ModelContextProfile, ModelName, ModelRequestDraft,
    ModelRequestLimits, ModelResponse, ModelSettings, ModelStreamItem, ModelStreamLimits,
    ModelTaskConfig, ModelToolCall, RandomSource, ResolvedToolCatalog, RunTaskConfig, RunTaskOwner,
    SameIdentityRetryPolicy, SideEffectClass, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    ToolCallDelta, ToolDeferralSupport, ToolExecutionPolicy, ToolPolicyDecision, ToolResult,
    ToolSpec, ToolStreamItem, ToolStreamLimits, ToolTaskConfig, Toolset, ToolsetRegistration,
    WorkflowSession, WorkflowWait, classify_wait, resolve_model_context_profile,
};
use finstack_ai_store_sqlite::{
    DEFAULT_BUSY_TIMEOUT, SqliteDurability, SqliteJournalStore, SqliteStoreConfig,
    SqliteStoreLimits,
};
use finstack_ai_test::{
    ScriptedModel, ScriptedModelAction, ScriptedModelPlan, ScriptedToolAction, ScriptedToolPlan,
    ScriptedToolset,
};
use finstack_ai_workflow_hitl::{
    ExpiryPolicy, ExpiryResolution, HitlError, HitlInboxStore, HitlRouter, InteractionRow,
    InteractionStatus, SqliteHitlStore, park,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxStore, PortsFactory, SqliteWorkerStore, WakeIndexStore, WorkerBuilder,
    WorkerError, WorkflowWorker,
};

use crate::capture::{id, timestamp};

/// Workflow kind the park is indexed under, and the key the worker's ports
/// factory is registered against.
const WORKFLOW_KIND: &str = "hitl-uc05";
const TENANT: &str = "tenant-a";

// ---------------------------------------------------------------------------
// Fixtures (mirroring finstack-ai-workflow-worker's tests/worker/helpers)
// ---------------------------------------------------------------------------

fn locator() -> OperationLocator {
    OperationLocator::try_new(TENANT, id(1), id(2), id(3)).expect("locator")
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

#[allow(clippy::too_many_arguments)]
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

/// Accepted root run for the fixture locator. `deadline` becomes the run's
/// effective deadline, which is exactly what the runtime copies into the
/// approval `InteractionRequest`'s `expires_at`
/// (`crates/finstack-ai-runtime/src/exec/settlement/interaction.rs::request_approval_interaction`),
/// so the expiry test chooses its deadline here rather than hand-editing an
/// inbox row.
fn accepted(deadline: Option<Timestamp>) -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            TENANT,
            PrincipalRef::try_new("issuer", "subject", Some(TENANT)).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        deadline,
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

fn stage_at(cycle: u64, stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle, stage },
        outcome,
    })
}

fn draft(messages: Arc<[Message]>, tools: Arc<[ToolSpec]>) -> ModelRequestDraft {
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

/// Poll the journal until `predicate` holds. Bounded by a tokio timeout so a
/// stuck fixture fails loudly instead of hanging; nothing here sleeps on the
/// wall clock.
async fn wait_state(
    store: &Arc<dyn JournalStore>,
    predicate: impl Fn(&KernelState) -> bool,
) -> CommitCoordinator {
    tokio::time::timeout(StdDuration::from_secs(5), async {
        loop {
            let recovered = CommitCoordinator::recover(Arc::clone(store), id(1))
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

// ---------------------------------------------------------------------------
// SQLite stores
// ---------------------------------------------------------------------------

/// Tempdir plus the two file paths a restarted host is handed: the kernel
/// journal, and one shared adapter file holding both the worker's tables and
/// the HITL inbox.
struct Paths {
    _dir: tempfile::TempDir,
    journal: PathBuf,
    adapters: PathBuf,
}

fn paths() -> Paths {
    let dir = tempfile::tempdir().expect("dir");
    let journal = dir.path().join("journal.sqlite");
    let adapters = dir.path().join("adapters.sqlite");
    Paths {
        _dir: dir,
        journal,
        adapters,
    }
}

fn open_journal(path: &Path) -> Arc<dyn JournalStore> {
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig {
            path: path.to_path_buf(),
            durability: SqliteDurability::Durable,
            limits: SqliteStoreLimits {
                sessions: 1,
                batches_per_session: 512,
                records_per_session: 4_096,
                snapshot_bytes: 256 * 1024,
            },
            busy_timeout: DEFAULT_BUSY_TIMEOUT,
        })
        .expect("journal"),
    )
}

// ---------------------------------------------------------------------------
// Model / toolset / catalog
// ---------------------------------------------------------------------------

fn echo_tools() -> Arc<[ToolSpec]> {
    Arc::from([ToolSpec {
        id: ToolId::parse("finstack.tools.echo").expect("tool id"),
        model_name: Arc::from("echo"),
        title: Arc::from("echo"),
        description: Arc::from("echo"),
        input_schema: RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"value":{"type":"integer"}},"required":["value"],"type":"object"}"#,
        )
        .expect("input"),
        output_schema: None,
        execution: ToolExecutionMode::Parallel,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    }])
}

/// One approval-gated `echo` toolset plus the catalog binding it. The
/// concrete toolset is handed back alongside the catalog so a test can assert
/// on [`ScriptedToolset::call_count`].
fn echo_catalog(tools: &Arc<[ToolSpec]>) -> (Arc<ScriptedToolset>, Arc<ResolvedToolCatalog>) {
    let toolset = Arc::new(ScriptedToolset::new(
        Arc::clone(tools),
        vec![ScriptedToolPlan {
            panic_on_call: None,
            actions: vec![ScriptedToolAction::Emit(Ok(ToolStreamItem::Completed(
                ToolResult {
                    output: RawJson::parse(r#"{"ok":true,"value":1}"#).expect("out"),
                    is_error: false,
                },
            )))],
        }],
    ));
    let policies = tools
        .iter()
        .map(|spec| {
            (
                spec.id.clone(),
                ToolExecutionPolicy {
                    failure_policy: ToolFailurePolicy::ReturnToModel,
                    approval: ToolPolicyDecision::RequireApproval,
                    max_concurrency: 1,
                },
            )
        })
        .collect();
    let catalog = Arc::new(
        ResolvedToolCatalog::try_new(
            [ToolsetRegistration {
                toolset: Arc::clone(&toolset) as Arc<dyn Toolset>,
                policies,
                components: BTreeMap::new(),
            }],
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .expect("catalog"),
    );
    (toolset, catalog)
}

/// Scripted model whose one plan calls `echo` — the call the approval policy
/// parks on.
fn tool_calling_model() -> Arc<dyn Model> {
    let arguments = RawJson::parse(br#"{"value":1}"#).expect("arguments");
    Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: 0,
                    name: Some(Arc::from("echo")),
                    arguments_delta: Arc::from(arguments.as_str()),
                    provider_call_id: None,
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([]),
                    tool_calls: Arc::from([ModelToolCall {
                        name: Arc::from("echo"),
                        arguments,
                        provider_call_id: None,
                    }]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::empty(),
                    completion_id: Arc::from("completion-tools"),
                    continuation_state: None,
                }))),
            ],
        }],
    ))
}

/// Scripted model for the restarted host: one plain-text completion, which
/// finalizes the second model cycle whatever the first cycle's tool outcome
/// was — an executed `echo` result after an approval, or the runtime's
/// synthetic `tool_approval_required` closure after a refusal.
fn finalizing_model() -> Arc<dyn Model> {
    Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("done"),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(ModelResponse {
                    assistant_content: Arc::from([ContentBlock::Text(
                        TextBlock::try_new("done").expect("text"),
                    )]),
                    tool_calls: Arc::from([]),
                    usage: Usage::empty(),
                    provider_ids: ProviderIds::try_new(
                        None::<&str>,
                        Some("response-1"),
                        None::<&str>,
                    )
                    .expect("provider ids"),
                    completion_id: Arc::from("completion-done"),
                    continuation_state: None,
                }))),
            ],
        }],
    ))
}

/// Binds the restarted host's model and catalog onto a session the worker
/// attached.
struct BindPorts {
    model: Arc<dyn Model>,
    catalog: Arc<ResolvedToolCatalog>,
}

impl PortsFactory for BindPorts {
    fn bind(&self, session: WorkflowSession) -> Result<WorkflowSession, WorkerError> {
        Ok(session.with_ports(
            Arc::clone(&self.model),
            locked_profile(),
            Some(Arc::clone(&self.catalog)),
        ))
    }
}

// ---------------------------------------------------------------------------
// Act 1: drive a fresh run onto a tool-approval park and index it
// ---------------------------------------------------------------------------

/// Accept a run, drive it through one model cycle that calls the
/// approval-gated `echo`, park the resulting `WorkflowWait::Interaction`
/// through this crate's [`park`], and drop every handle.
///
/// Returns the canonical interaction id. On return the only surviving state
/// is on disk under `paths`.
#[expect(
    clippy::too_many_lines,
    reason = "keeps the whole pre-death act — accept, model cycle, park — contiguous"
)]
async fn park_on_approval(paths: &Paths, deadline: Option<Timestamp>) -> Arc<str> {
    let journal = open_journal(&paths.journal);
    let tools = echo_tools();
    let (toolset, catalog) = echo_catalog(&tools);
    let model = tool_calling_model();
    let clock = ExternalClock::new(timestamp(2_500));

    let owner = RunTaskOwner::spawn_with_model_and_tools(
        CommitCoordinator::new(Arc::clone(&journal)),
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
            approval_grant: ApprovalGrantMode::PerCall,
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        ToolTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        Arc::clone(&model),
        locked_profile(),
        Arc::clone(&catalog),
        clock.clone(),
        CounterRandom(AtomicU64::new(700)),
    )
    .await
    .expect("owner");

    let handle = owner.handle();
    handle
        .submit(
            env(1_000, &[1], &[1], &[], &[], &[], &[], 101),
            KernelInput::AcceptRun(AcceptRun {
                session_id: id(1),
                lane_id: id(2),
                accepted: accepted(deadline),
            }),
        )
        .await
        .expect("accept");
    handle
        .submit(
            env(1_100, &[2], &[], &[], &[], &[], &[], 102),
            stage_at(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
        )
        .await
        .expect("before run");
    let message = user_message();
    handle
        .submit(
            env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
            stage_at(
                0,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::from([message.clone()]),
                },
            ),
        )
        .await
        .expect("context");
    let raw = RawJson::parse(
        draft(Arc::from([message]), Arc::clone(&tools))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    handle
        .submit(
            env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104),
            stage_at(
                0,
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
    wait_state(&journal, |state| state.phase == Some(RunPhase::AfterModel)).await;
    handle
        .submit(
            env(2_100, &[7], &[], &[], &[], &[], &[], 105),
            stage_at(0, Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model");
    wait_state(&journal, |state| {
        state.phase == Some(RunPhase::AwaitingInteraction)
    })
    .await;
    drop(handle);
    drop(owner);

    let mut session = WorkflowSession::trusted(Arc::clone(&journal), locator(), clock, 701)
        .await
        .expect("attach")
        .with_ports(model, locked_profile(), Some(Arc::clone(&catalog)));
    let WorkflowWait::Interaction { interaction_id, .. } =
        session.drive_until_wait().await.expect("interaction")
    else {
        panic!("expected an interaction wait");
    };

    let wake = SqliteWorkerStore::open(&paths.adapters).expect("worker store");
    let inbox = SqliteHitlStore::open(&paths.adapters).expect("hitl store");
    park(&mut session, &wake, &inbox, WORKFLOW_KIND, timestamp(2_600)).expect("park");
    assert!(!session.owner_is_live(), "park aborted the session owner");
    assert_eq!(
        toolset.call_count(),
        0,
        "the approval-gated tool has not run yet"
    );

    // Process death: every in-memory handle goes away. Only the two files
    // under `paths` survive.
    drop(session);
    drop(inbox);
    drop(wake);
    drop(catalog);
    drop(toolset);
    drop(journal);

    Arc::from(interaction_id.to_canonical_string())
}

// ---------------------------------------------------------------------------
// Act 2: the restarted host
// ---------------------------------------------------------------------------

/// Everything a host rebuilds from paths alone after the crash.
struct Reopened {
    journal: Arc<dyn JournalStore>,
    inbox: Arc<SqliteHitlStore>,
    adapters: Arc<SqliteWorkerStore>,
    worker: Arc<WorkflowWorker>,
    router: HitlRouter,
    model: Arc<dyn Model>,
    tools: Arc<[ToolSpec]>,
    toolset: Arc<ScriptedToolset>,
    catalog: Arc<ResolvedToolCatalog>,
    clock: ExternalClock,
}

impl Reopened {
    /// A second router over the same inbox/worker/wake index, with `policy`
    /// in place of [`finstack_ai_workflow_hitl::ApprovalExpiry`].
    fn router_with_expiry_policy(&self, policy: Arc<dyn ExpiryPolicy>) -> HitlRouter {
        HitlRouter::new(
            Arc::clone(&self.inbox) as Arc<dyn HitlInboxStore>,
            Arc::clone(&self.worker),
            Arc::clone(&self.adapters) as Arc<dyn WakeIndexStore>,
        )
        .with_expiry_policy(policy)
    }
}

/// Expiry policy that refuses under the *run's own* accepted principal and
/// authorization evidence.
///
/// The shipped default, [`finstack_ai_workflow_hitl::ApprovalExpiry`],
/// attributes its refusal to a synthetic
/// `("finstack.workflow.hitl", "expiry", Some(tenant))` principal with
/// `("hitl-expiry-v1", "refuse-on-expiry")` evidence. That resolution is
/// durably delivered to the worker inbox and the row is marked `Expired` —
/// but the runtime's interaction ingress
/// (`crates/finstack-ai-runtime/src/driver/ingress/shared.rs::authorization_matches`)
/// admits a resolution only when its principal *and* policy version *and*
/// decision id equal the ones on `RunAccepted`'s security context. A
/// synthetic expiry principal can never satisfy that, so the refusal is
/// rejected as `scope_mismatch`, the run stays parked on
/// `RunPhase::AwaitingInteraction` forever, and the inbox row is already
/// `Expired` and therefore invisible to `pending`. This test therefore
/// supplies a policy whose credentials the ingress accepts; see the report
/// for the defect write-up.
struct RunPrincipalExpiry;

impl ExpiryPolicy for RunPrincipalExpiry {
    fn expire(
        &self,
        _row: &InteractionRow,
        request: &InteractionRequest,
    ) -> Result<Option<ExpiryResolution>, HitlError> {
        assert!(
            matches!(
                request.kind(),
                finstack_ai_kernel::InteractionKind::Approval
            ),
            "the fixture only ever parks approvals"
        );
        Ok(Some(ExpiryResolution {
            principal: PrincipalRef::try_new("issuer", "subject", Some(TENANT)).expect("principal"),
            evidence: AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("evidence"),
            payload: RawJson::parse(r#"{"approved":false}"#).expect("payload"),
        }))
    }
}

fn reopen(paths: &Paths) -> Reopened {
    let journal = open_journal(&paths.journal);
    let adapters = Arc::new(SqliteWorkerStore::open(&paths.adapters).expect("worker store"));
    let inbox = Arc::new(SqliteHitlStore::open(&paths.adapters).expect("hitl store"));
    let tools = echo_tools();
    let (toolset, catalog) = echo_catalog(&tools);
    let model = finalizing_model();
    let clock = ExternalClock::new(timestamp(2_600));

    let worker = Arc::new(
        WorkerBuilder::new(
            Arc::clone(&journal),
            Arc::new(MemoryCronStore::new()),
            Arc::clone(&adapters) as Arc<dyn WakeIndexStore>,
            Arc::clone(&adapters) as Arc<dyn FireStore>,
            Arc::clone(&adapters) as Arc<dyn InboxStore>,
        )
        .clock(clock.clone())
        .drive_timeout(StdDuration::from_millis(500))
        .register_ports(
            WORKFLOW_KIND,
            Arc::new(BindPorts {
                model: Arc::clone(&model),
                catalog: Arc::clone(&catalog),
            }),
        )
        .build(),
    );
    let router = HitlRouter::new(
        Arc::clone(&inbox) as Arc<dyn HitlInboxStore>,
        Arc::clone(&worker),
        Arc::clone(&adapters) as Arc<dyn WakeIndexStore>,
    );

    Reopened {
        journal,
        inbox,
        adapters,
        worker,
        router,
        model,
        tools,
        toolset,
        catalog,
        clock,
    }
}

/// Stands in for the missing application-level facade (see the module doc):
/// submits the `AfterToolBatch -> PrepareContext -> BeforeModel -> AfterModel
/// -> BeforeFinalize` decisions that carry the run from `AfterToolBatch` —
/// where the worker's own tick left it — through the second model cycle to a
/// terminal candidate. Copied from `finstack-ai-workflow-worker`'s
/// `tests/worker/inbox_resume.rs::drive_past_missing_facade_decisions`.
#[expect(
    clippy::too_many_lines,
    reason = "replicates the full AfterToolBatch -> BeforeFinalize facade sequence"
)]
async fn drive_past_missing_facade_decisions(host: &Reopened) {
    let recovered = CommitCoordinator::recover(Arc::clone(&host.journal), locator().session_id)
        .await
        .expect("recover for facade");
    assert_eq!(
        recovered.state().phase,
        Some(RunPhase::AfterToolBatch),
        "the facade only needs to unblock AfterToolBatch"
    );
    let cycle = recovered.state().cycle;

    let facade = RunTaskOwner::spawn_with_model_and_tools(
        recovered,
        RunTaskConfig {
            command_capacity: 8,
            event_hub: EventHubConfig {
                source_capacity: 16,
                max_subscribers: 8,
            },
            shutdown_deadline: StdDuration::from_millis(500),
            approval_grant: ApprovalGrantMode::PerCall,
        },
        ModelTaskConfig {
            job_capacity: 2,
            result_capacity: 2,
            stream_limits: ModelStreamLimits::default(),
            warmup_deadline: None,
            warmup_metadata: Metadata::empty(),
            same_identity_retry: SameIdentityRetryPolicy::default(),
        },
        ToolTaskConfig {
            job_capacity: 8,
            result_capacity: 8,
            global_max_concurrency: 2,
            stream_limits: ToolStreamLimits::default(),
        },
        Arc::clone(&host.model),
        locked_profile(),
        Arc::clone(&host.catalog),
        host.clock.clone(),
        CounterRandom(AtomicU64::new(705)),
    )
    .await
    .expect("facade owner");

    facade
        .handle()
        .submit(
            env(3_100, &[300], &[], &[], &[], &[], &[], 301),
            stage_at(cycle, Stage::AfterToolBatch, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after tool batch");
    wait_state(&host.journal, |state| {
        state.phase == Some(RunPhase::PreparingContext)
    })
    .await;

    let next_cycle = cycle + 1;
    let messages: Arc<[Message]> = Arc::from(
        CommitCoordinator::recover(Arc::clone(&host.journal), locator().session_id)
            .await
            .expect("recover")
            .state()
            .messages
            .as_slice(),
    );
    facade
        .handle()
        .submit(
            env(3_200, &[302, 303], &[], &[], &[304], &[], &[], 305),
            stage_at(
                next_cycle,
                Stage::PrepareContext,
                ReducerStageOutcome::ContextPrepared {
                    messages: Arc::clone(&messages),
                },
            ),
        )
        .await
        .expect("context prepared");

    let raw = RawJson::parse(
        draft(messages, Arc::clone(&host.tools))
            .canonical_bytes()
            .expect("canonical"),
    )
    .expect("raw");
    facade
        .handle()
        .submit(
            env(3_300, &[306, 307], &[308], &[309], &[], &[310], &[], 311),
            stage_at(
                next_cycle,
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
    wait_state(&host.journal, |state| {
        state.phase == Some(RunPhase::AfterModel)
    })
    .await;

    facade
        .handle()
        .submit(
            env(3_400, &[312], &[], &[], &[], &[], &[], 313),
            stage_at(next_cycle, Stage::AfterModel, ReducerStageOutcome::Continue),
        )
        .await
        .expect("after model, cycle 2");
    wait_state(&host.journal, |state| {
        state.phase == Some(RunPhase::BeforeFinalize)
    })
    .await;

    facade
        .handle()
        .submit(
            env(3_500, &[314, 315], &[316], &[], &[], &[], &[], 317),
            stage_at(
                next_cycle,
                Stage::BeforeFinalize,
                ReducerStageOutcome::FinalizeAccepted,
            ),
        )
        .await
        .expect("finalize");
    wait_state(&host.journal, |state| state.terminal.is_some()).await;
    drop(facade);
}

/// Every committed message on the run, as one canonical JSON blob — enough
/// to assert what tool outcome the second model cycle actually observed.
async fn messages_json(host: &Reopened) -> String {
    let recovered = CommitCoordinator::recover(Arc::clone(&host.journal), locator().session_id)
        .await
        .expect("recover");
    serde_json::to_string(recovered.state().messages.as_slice()).expect("encode messages")
}

/// Attach a fresh session to the journal and assert its classified wait is
/// terminal.
async fn assert_terminal(host: &Reopened, seed: u64) {
    let session = WorkflowSession::trusted(
        Arc::clone(&host.journal),
        locator(),
        host.clock.clone(),
        seed,
    )
    .await
    .expect("attach for terminal check");
    let wait = classify_wait(session.last_state());
    assert!(
        matches!(wait, Some(WorkflowWait::Terminal { .. })),
        "expected a terminal wait, got {wait:?}"
    );
    let recovered = CommitCoordinator::recover(Arc::clone(&host.journal), locator().session_id)
        .await
        .expect("recover");
    assert!(
        recovered.state().terminal.is_some(),
        "the run reached a terminal state"
    );
    assert!(
        recovered.state().pending_interaction.is_none(),
        "the interaction is settled on the journal"
    );
}

// ---------------------------------------------------------------------------
// UC-05: the operator approves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn uc05_resolve_end_to_end() {
    let paths = paths();
    let interaction_id = Box::pin(park_on_approval(&paths, None)).await;

    // --- process death boundary ---
    let host = reopen(&paths);
    let router = &host.router;

    let pending = router.pending(TENANT).expect("pending");
    assert_eq!(pending.len(), 1, "one approval waits for this tenant");
    assert_eq!(pending[0].interaction_id, interaction_id);
    assert_eq!(pending[0].kind.as_ref(), "approval");
    assert_eq!(pending[0].status, InteractionStatus::Open);
    assert_eq!(pending[0].session_id, id(1));
    assert_eq!(pending[0].run_id, id(3));
    assert_eq!(pending[0].requested_at, timestamp(2_600));
    assert_eq!(
        pending[0].expires_at, None,
        "this run has no deadline, so the approval has none either"
    );

    router
        .resolve(
            TENANT,
            &interaction_id,
            "resolution-1",
            PrincipalRef::try_new("issuer", "subject", Some(TENANT)).expect("principal"),
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("evidence"),
            RawJson::parse(r#"{"approved":true}"#).expect("payload"),
            None,
            timestamp(3_000),
        )
        .expect("resolve");
    let row = host
        .inbox
        .load(TENANT, &interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Delivered);
    assert_eq!(row.resolved_by.as_deref(), Some("subject"));

    // First tick: the resolution is applied and the approved tool runs, but
    // the run lands on `AfterToolBatch`, which no worker can advance past
    // (see the module doc). The response is preserved for a later tick.
    let applied = Box::pin(host.worker.tick())
        .await
        .expect("apply resolution");
    assert_eq!(applied.sessions_resumed, 0);
    assert_eq!(applied.failures, 1);
    assert_eq!(
        host.toolset.call_count(),
        1,
        "the approved tool executed exactly once"
    );

    drive_past_missing_facade_decisions(&host).await;

    host.clock.jump(120_000).expect("past backoff");
    let completed = Box::pin(host.worker.tick())
        .await
        .expect("resume to completion");
    assert_eq!(
        completed.sessions_resumed, 1,
        "the very same worker drove the resumed run to a terminal state"
    );
    assert!(
        host.adapters.load_all().expect("inbox").is_empty(),
        "the consumed response is drained from the worker inbox"
    );
    assert!(
        host.adapters
            .load_tenant(TENANT)
            .expect("wake rows")
            .is_empty(),
        "the wake row is deleted once the run terminates"
    );

    assert_terminal(&host, 720).await;
    assert_eq!(
        host.toolset.call_count(),
        1,
        "the approved tool still executed exactly once"
    );
    let messages = messages_json(&host).await;
    assert!(
        messages.contains(r#"{"ok":true,"value":1}"#),
        "the approved tool's own result reached the model: {messages}"
    );
    assert!(
        !messages.contains("tool_approval_required"),
        "nothing was refused on the approval path: {messages}"
    );

    // The wake row is gone, so the final sweep reconciles the delivered row
    // to `Closed` rather than expiring it.
    let report = router.sweep(timestamp(200_000)).expect("sweep");
    assert_eq!(report.reconciled, 1);
    assert_eq!(report.expired, 0);
    let closed = host
        .inbox
        .load(TENANT, &interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(closed.status, InteractionStatus::Closed);
    assert_eq!(closed.updated_at, timestamp(200_000));
    assert!(router.pending(TENANT).expect("pending").is_empty());
}

// ---------------------------------------------------------------------------
// UC-05: nobody answers, and the sweep refuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn uc05_expiry_end_to_end() {
    let deadline = timestamp(600_000);
    let paths = paths();
    let interaction_id = Box::pin(park_on_approval(&paths, Some(deadline))).await;

    // --- process death boundary ---
    let host = reopen(&paths);
    let router = host.router_with_expiry_policy(Arc::new(RunPrincipalExpiry));

    let pending = router.pending(TENANT).expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].interaction_id, interaction_id);
    assert_eq!(
        pending[0].expires_at,
        Some(deadline),
        "the run's effective deadline is the approval's deadline"
    );

    // Nobody resolves. The sweep, told the deadline has passed, refuses.
    let early = router.sweep(timestamp(500_000)).expect("early sweep");
    assert_eq!(early.expired, 0, "before the deadline nothing expires");
    assert_eq!(early.reconciled, 0);

    let report = router.sweep(deadline).expect("sweep");
    assert_eq!(report.expired, 1);
    assert_eq!(report.reconciled, 0);
    let refused = host
        .inbox
        .load(TENANT, &interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(refused.status, InteractionStatus::Expired);
    assert_eq!(
        refused.resolved_by.as_deref(),
        Some("subject"),
        "the policy's refusing principal is recorded on the row"
    );

    // First tick: the refusal is applied. The runtime synthesizes the
    // `tool_approval_required` tool outcome for the model; the tool itself
    // never runs.
    let applied = Box::pin(host.worker.tick()).await.expect("apply refusal");
    assert_eq!(applied.sessions_resumed, 0);
    assert_eq!(applied.failures, 1);
    assert_eq!(
        host.toolset.call_count(),
        0,
        "a refused approval never executes the tool"
    );

    drive_past_missing_facade_decisions(&host).await;

    host.clock.jump(120_000).expect("past backoff");
    let completed = Box::pin(host.worker.tick())
        .await
        .expect("resume to completion");
    assert_eq!(completed.sessions_resumed, 1);
    assert!(host.adapters.load_all().expect("inbox").is_empty());
    assert!(
        host.adapters
            .load_tenant(TENANT)
            .expect("wake rows")
            .is_empty()
    );

    assert_terminal(&host, 730).await;
    assert_eq!(
        host.toolset.call_count(),
        0,
        "the refused tool never executed"
    );
    let messages = messages_json(&host).await;
    assert!(
        messages.contains("tool_approval_required"),
        "the runtime synthesized the refusal outcome for the model: {messages}"
    );
    assert!(
        !messages.contains(r#"{"ok":true,"value":1}"#),
        "no tool output exists, because the tool never ran: {messages}"
    );

    let final_row = host
        .inbox
        .load(TENANT, &interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(
        final_row.status,
        InteractionStatus::Expired,
        "the refused row stays Expired"
    );
    assert!(router.pending(TENANT).expect("pending").is_empty());
}
