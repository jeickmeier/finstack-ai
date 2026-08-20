//! Executable spec for capturing interaction parks into the HITL inbox.

use std::sync::Arc;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, ComponentId, ComponentRef,
    ContentBlock, DeadlinePropagation, Digest, EffectTag, Id, IdTag, InteractionKind,
    InteractionRequest, InteractionTag, KernelInput, Metadata, OperationLocator,
    PrincipalPropagation, PrincipalRef, RawJson, RequestInteraction, RunAccepted, RunLimits,
    RunPropagationPolicy, RunRelation, RunSecurityContext, TextBlock, Timestamp, TransitionEnv,
    Version,
};
use finstack_ai_runtime::{
    CommitCoordinator, ExternalClock, JournalStore, LockedModelContextProfile, Model,
    ModelContextProfile, ModelName, TokenEstimatorRef, TokenEstimatorSource, WorkflowCheckpoint,
    WorkflowSession, WorkflowWait, resolve_model_context_profile,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::ScriptedModel;
use finstack_ai_workflow_hitl::{
    HitlInboxStore, InteractionStatus, MemoryHitlStore, capture, park,
};
use finstack_ai_workflow_worker::{MemoryWorkerStore, WakeIndexStore, WakeReason};

// Fixtures below mirror finstack-ai-workflow-worker's executable spec
// (tests/worker/helpers/mod.rs), which itself copies
// finstack-ai-workflow-local's tests/local_workflow/restart.rs.

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

fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
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

/// Approval-profile request built exactly as `request_approval_interaction`
/// (`crates/finstack-ai-runtime/src/exec/settlement/interaction.rs`) builds
/// the envelope the runtime commits for a paid-tool approval park.
fn approval_request(
    interaction_id: Id<InteractionTag>,
    effect_id: Id<EffectTag>,
    expires_at: Option<Timestamp>,
) -> InteractionRequest {
    InteractionRequest::try_new(
        1,
        interaction_id,
        effect_id,
        InteractionKind::Approval,
        vec![ContentBlock::Text(
            TextBlock::try_new("approve the next tool action:\necho {\"value\":1}").expect("prompt"),
        )],
        RawJson::parse(
            br#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
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
        expires_at,
        false,
        Metadata::empty(),
    )
    .expect("interaction request")
}

fn checkpoint() -> WorkflowCheckpoint {
    WorkflowCheckpoint {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
        last_applied_seq: 7,
        external_handles: std::collections::BTreeMap::new(),
    }
}

#[test]
fn capture_writes_one_open_row_for_an_interaction_wait() {
    let interaction_id: Id<InteractionTag> = id(50);
    let request = approval_request(interaction_id, id(51), Some(timestamp(9_000)));
    let wait = WorkflowWait::Interaction {
        interaction_id,
        request: request.clone(),
    };
    let inbox = MemoryHitlStore::new();

    let captured = capture(&inbox, &checkpoint(), &wait, timestamp(2_000)).expect("capture");

    assert!(captured);
    let row = inbox
        .load("tenant-a", &interaction_id.to_canonical_string())
        .expect("load")
        .expect("row");
    assert_eq!(
        row.interaction_id.as_ref(),
        interaction_id.to_canonical_string()
    );
    assert_eq!(row.kind.as_ref(), "approval");
    assert_eq!(row.expires_at, Some(timestamp(9_000)));
    assert_eq!(row.requested_at, timestamp(2_000));
    assert_eq!(row.updated_at, timestamp(2_000));
    assert_eq!(row.status, InteractionStatus::Open);
    assert_eq!(row.resolved_by, None);
    assert_eq!(row.session_id, id(1));
    assert_eq!(row.lane_id, id(2));
    assert_eq!(row.run_id, id(3));
    let decoded: InteractionRequest =
        serde_json::from_slice(row.request.as_ref()).expect("round-trip");
    assert_eq!(decoded, request);
}

#[test]
fn capture_ignores_non_interaction_waits() {
    let inbox = MemoryHitlStore::new();
    let wait = WorkflowWait::Timer {
        effect_id: id(51),
        due_at: timestamp(5_000),
    };

    let captured = capture(&inbox, &checkpoint(), &wait, timestamp(2_000)).expect("capture");

    assert!(!captured);
    assert!(inbox.load_active().expect("active").is_empty());
}

#[test]
fn capture_is_idempotent_on_recapture() {
    let interaction_id: Id<InteractionTag> = id(50);
    let wait = WorkflowWait::Interaction {
        interaction_id,
        request: approval_request(interaction_id, id(51), None),
    };
    let inbox = MemoryHitlStore::new();

    assert!(capture(&inbox, &checkpoint(), &wait, timestamp(2_000)).expect("first"));
    assert!(capture(&inbox, &checkpoint(), &wait, timestamp(4_000)).expect("second"));

    let rows = inbox.load_open("tenant-a").expect("open");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].requested_at, timestamp(4_000));
    assert_eq!(rows[0].expires_at, None);
}

/// Commit an interaction request directly onto a freshly accepted run,
/// bypassing the model/tool-batch pipeline. Copied from
/// `finstack-ai-workflow-worker`'s `tests/worker/helpers/mod.rs`
/// `request_interaction`, which reaches the same
/// `WorkflowWait::Interaction` as `restart.rs`'s scripted approval park
/// without needing a model, catalog, or owner.
async fn request_interaction(
    store: &Arc<MemoryJournalStore>,
    now: Timestamp,
) -> Id<InteractionTag> {
    let dyn_store = Arc::clone(store) as Arc<dyn JournalStore>;
    let mut coordinator = CommitCoordinator::new(dyn_store);
    coordinator
        .submit(
            TransitionEnv {
                now: timestamp(1_000),
                ids: AllocatedIds::try_new(
                    vec![id(1)],
                    vec![id(1)],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![id(101)],
                    Vec::new(),
                )
                .expect("ids"),
            },
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
    let request = approval_request(interaction_id, effect_id, None);
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

#[tokio::test]
async fn park_indexes_the_wake_row_and_captures_the_inbox_row() {
    let journal = memory_store();
    let interaction_id = request_interaction(&journal, timestamp(2_000)).await;
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(profile(), Vec::new()));
    let clock = ExternalClock::new(timestamp(2_000));
    let mut session = WorkflowSession::trusted(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator(),
        clock,
        830,
    )
    .await
    .expect("attach")
    .with_ports(model, locked_profile(), None);
    let wait = session.drive_until_wait().await.expect("interaction wait");
    assert!(matches!(wait, WorkflowWait::Interaction { .. }));

    let wake = MemoryWorkerStore::new();
    let inbox = MemoryHitlStore::new();
    let checkpoint =
        park(&mut session, &wake, &inbox, "hitl-demo", timestamp(2_500)).expect("park");

    assert!(!session.owner_is_live());
    let wake_rows = wake.load_tenant("tenant-a").expect("wake rows");
    assert_eq!(wake_rows.len(), 1);
    assert_eq!(wake_rows[0].reason, WakeReason::Interaction);
    assert_eq!(wake_rows[0].workflow_kind.as_ref(), "hitl-demo");
    assert_eq!(wake_rows[0].session_id, checkpoint.session_id);

    let inbox_rows = inbox.load_open("tenant-a").expect("inbox rows");
    assert_eq!(inbox_rows.len(), 1);
    assert_eq!(inbox_rows[0].status, InteractionStatus::Open);
    assert_eq!(inbox_rows[0].kind.as_ref(), "approval");
    assert_eq!(inbox_rows[0].requested_at, timestamp(2_500));
    assert_eq!(
        inbox_rows[0].interaction_id.as_ref(),
        interaction_id.to_canonical_string()
    );
    assert_eq!(inbox_rows[0].interaction_id, wake_rows[0].pending_id);
}
