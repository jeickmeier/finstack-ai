//! Executable spec for the HITL router's expiry + reconcile sweep.
//!
//! Rows are seeded through [`finstack_ai_workflow_hitl::capture`] and paired
//! with a hand-written wake row, because the sweep joins the inbox against
//! the worker's wake index rather than the journal.

use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, Id, InteractionKind, InteractionRequest, InteractionResolutionCommand,
    InteractionTag, PrincipalRef, RawJson, Timestamp,
};
use finstack_ai_runtime::{JournalStore, WorkflowWait};
use finstack_ai_workflow_hitl::{
    ExpiryPolicy, ExpiryResolution, HitlError, HitlInboxStore, HitlRouter, InteractionRow,
    InteractionStatus, MemoryHitlStore, capture,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxKind, InboxStore, MemoryWorkerStore, WakeIndexStore, WakeReason, WakeRow,
    WorkerBuilder, WorkflowWorker,
};

use crate::capture::{checkpoint, id, interaction_request, memory_store, timestamp};

struct Harness {
    inbox: Arc<MemoryHitlStore>,
    worker_store: Arc<MemoryWorkerStore>,
    router: HitlRouter,
}

/// Empty inbox plus a live worker over memory adapter tables.
fn harness() -> Harness {
    let inbox = Arc::new(MemoryHitlStore::new());
    let worker_store = Arc::new(MemoryWorkerStore::new());
    let worker = Arc::new(
        WorkerBuilder::new(
            memory_store() as Arc<dyn JournalStore>,
            Arc::new(MemoryCronStore::new()),
            Arc::clone(&worker_store) as Arc<dyn WakeIndexStore>,
            Arc::clone(&worker_store) as Arc<dyn FireStore>,
            Arc::clone(&worker_store) as Arc<dyn InboxStore>,
        )
        .build(),
    );
    let router = HitlRouter::new(
        Arc::clone(&inbox) as Arc<dyn HitlInboxStore>,
        Arc::clone(&worker) as Arc<WorkflowWorker>,
        Arc::clone(&worker_store) as Arc<dyn WakeIndexStore>,
    );
    Harness {
        inbox,
        worker_store,
        router,
    }
}

impl Harness {
    /// Capture one `Open` row for `tenant-a` and index its wake row.
    fn seed(&self, kind: InteractionKind, expires_at: Option<Timestamp>) -> Arc<str> {
        let interaction_id: Id<InteractionTag> = id(50);
        let request = interaction_request(interaction_id, id(51), kind, expires_at);
        let wait = WorkflowWait::Interaction {
            interaction_id,
            request,
        };
        assert!(
            capture(self.inbox.as_ref(), &checkpoint(), &wait, timestamp(2_000)).expect("capture")
        );
        let pending_id: Arc<str> = Arc::from(interaction_id.to_canonical_string());
        self.worker_store
            .upsert(&WakeRow {
                tenant_scope: Arc::from("tenant-a"),
                session_id: id(1),
                lane_id: id(2),
                run_id: id(3),
                workflow_kind: Arc::from("hitl-demo"),
                reason: WakeReason::Interaction,
                wake_at: None,
                pending_id: Arc::clone(&pending_id),
                leased_by: None,
                lease_expires_at: None,
                attempts: 0,
            })
            .expect("wake row");
        pending_id
    }

    /// Drop the wake row, standing in for a tick that already consumed it.
    fn forget_wake(&self) {
        WakeIndexStore::delete(self.worker_store.as_ref(), "tenant-a", id(1)).expect("delete wake");
    }

    fn row(&self, interaction_id: &str) -> InteractionRow {
        self.inbox
            .load("tenant-a", interaction_id)
            .expect("load")
            .expect("row")
    }
}

/// The shape of policy a host installs: it refuses an expired approval under
/// the run's *own* accepted principal and evidence — the only credential the
/// runtime's interaction ingress admits — and declines every other kind.
/// `("issuer", "subject", Some("tenant-a"))` / `("policy-v1", "decision-v1")`
/// are what the shared fixture's `accepted()` run was accepted with; the
/// UC-05 spec proves this shape through a real tick.
struct RunPrincipalExpiry;

impl ExpiryPolicy for RunPrincipalExpiry {
    fn expire(
        &self,
        row: &InteractionRow,
        request: &InteractionRequest,
    ) -> Result<Option<ExpiryResolution>, HitlError> {
        if !matches!(request.kind(), InteractionKind::Approval) {
            return Ok(None);
        }
        Ok(Some(ExpiryResolution {
            principal: PrincipalRef::try_new("issuer", "subject", Some(row.tenant_scope.as_ref()))
                .expect("principal"),
            evidence: AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("evidence"),
            payload: RawJson::parse(r#"{"approved":false}"#).expect("payload"),
        }))
    }
}

#[test]
fn sweep_before_the_deadline_changes_nothing() {
    let harness = harness();
    let interaction_id = harness.seed(InteractionKind::Approval, Some(timestamp(9_000)));
    let router = harness
        .router
        .with_expiry_policy(Arc::new(RunPrincipalExpiry));

    let report = router.sweep(timestamp(5_000)).expect("sweep");

    assert_eq!(report.expired, 0);
    assert_eq!(report.reconciled, 0);
    assert_eq!(
        harness
            .inbox
            .load("tenant-a", &interaction_id)
            .expect("load")
            .expect("row")
            .status,
        InteractionStatus::Open
    );
    assert!(
        harness
            .worker_store
            .load_all()
            .expect("worker inbox")
            .is_empty()
    );
}

/// The shipped default never forges a resolution: it holds no credential the
/// runtime's interaction ingress would admit
/// (`crates/finstack-ai-runtime/src/driver/ingress/shared.rs:85`), and a row
/// marked `Expired` behind a resolution the tick rejects would strand the run
/// while hiding it from `pending`.
#[test]
fn the_default_policy_expires_nothing_and_delivers_nothing() {
    let harness = harness();
    let interaction_id = harness.seed(InteractionKind::Approval, Some(timestamp(9_000)));

    let report = harness.router.sweep(timestamp(9_000)).expect("sweep");

    assert_eq!(report.expired, 0);
    assert_eq!(report.reconciled, 0);
    assert!(
        harness
            .worker_store
            .load_all()
            .expect("worker inbox")
            .is_empty(),
        "the default policy delivers nothing"
    );

    let row = harness.row(&interaction_id);
    assert_eq!(row.status, InteractionStatus::Open);
    assert_eq!(row.resolved_by, None);
    assert_eq!(
        harness.router.pending("tenant-a").expect("pending").len(),
        1,
        "an unexpired-by-policy interaction stays operator-visible"
    );
}

#[test]
fn a_host_policy_expires_an_approval_and_delivers_its_refusal() {
    let harness = harness();
    let interaction_id = harness.seed(InteractionKind::Approval, Some(timestamp(9_000)));
    let router = harness
        .router
        .with_expiry_policy(Arc::new(RunPrincipalExpiry));

    let report = router.sweep(timestamp(9_000)).expect("sweep");

    assert_eq!(report.expired, 1);
    assert_eq!(report.reconciled, 0);

    let delivered = harness.worker_store.load_all().expect("worker inbox");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].kind, InboxKind::Interaction);
    assert_eq!(delivered[0].pending_id, interaction_id);
    assert_eq!(delivered[0].received_at, timestamp(9_000));
    let command: InteractionResolutionCommand =
        serde_json::from_slice(delivered[0].payload.as_ref()).expect("command");
    assert_eq!(
        command.resolution.response().as_str(),
        r#"{"approved":false}"#
    );
    assert_eq!(command.resolution.principal().subject(), "subject");
    assert_eq!(command.resolution.principal().issuer(), "issuer");
    assert_eq!(
        command.resolution.resolution_id(),
        format!("hitl-expiry-{interaction_id}")
    );

    let row = harness
        .inbox
        .load("tenant-a", &interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Expired);
    assert_eq!(row.resolved_by.as_deref(), Some("subject"));
    assert_eq!(row.updated_at, timestamp(9_000));
    assert!(
        harness
            .inbox
            .load_open("tenant-a")
            .expect("open")
            .is_empty(),
        "an expired interaction leaves the pending view"
    );
}

#[test]
fn a_policy_that_declines_leaves_the_row_open() {
    let harness = harness();
    let interaction_id = harness.seed(InteractionKind::FreeText, Some(timestamp(9_000)));
    let router = harness
        .router
        .with_expiry_policy(Arc::new(RunPrincipalExpiry));

    let report = router.sweep(timestamp(9_000)).expect("sweep");

    assert_eq!(report.expired, 0);
    assert_eq!(report.reconciled, 0);
    assert_eq!(
        harness
            .inbox
            .load("tenant-a", &interaction_id)
            .expect("load")
            .expect("row")
            .status,
        InteractionStatus::Open,
        "a policy that declines leaves the row for the next sweep"
    );
    assert!(
        harness
            .worker_store
            .load_all()
            .expect("worker inbox")
            .is_empty()
    );
}

#[test]
fn sweep_closes_a_delivered_row_whose_wake_row_is_gone() {
    let harness = harness();
    let interaction_id = harness.seed(InteractionKind::Approval, None);
    harness
        .inbox
        .set_status(
            "tenant-a",
            &interaction_id,
            InteractionStatus::Delivered,
            Some("subject"),
            timestamp(3_000),
        )
        .expect("deliver");
    harness.forget_wake();

    let report = harness.router.sweep(timestamp(5_000)).expect("sweep");

    assert_eq!(report.reconciled, 1);
    assert_eq!(report.expired, 0);
    let row = harness.row(&interaction_id);
    assert_eq!(row.status, InteractionStatus::Closed);
    assert_eq!(row.updated_at, timestamp(5_000));
}

#[test]
fn reconcile_beats_expiry_for_a_row_whose_wake_row_is_gone() {
    let harness = harness();
    let interaction_id = harness.seed(InteractionKind::Approval, Some(timestamp(9_000)));
    harness.forget_wake();
    // A policy that *would* expire this row, so the ordering is what decides.
    let router = harness
        .router
        .with_expiry_policy(Arc::new(RunPrincipalExpiry));

    let report = router.sweep(timestamp(9_000)).expect("sweep");

    assert_eq!(report.reconciled, 1);
    assert_eq!(report.expired, 0);
    assert_eq!(
        harness
            .inbox
            .load("tenant-a", &interaction_id)
            .expect("load")
            .expect("row")
            .status,
        InteractionStatus::Closed,
        "an out-of-band resolution closes the row instead of refusing it"
    );
    assert!(
        harness
            .worker_store
            .load_all()
            .expect("worker inbox")
            .is_empty(),
        "a reconciled row is never delivered"
    );
}

#[test]
fn sweeping_twice_is_idempotent() {
    let harness = harness();
    harness.seed(InteractionKind::Approval, Some(timestamp(9_000)));
    let router = harness
        .router
        .with_expiry_policy(Arc::new(RunPrincipalExpiry));

    let first = router.sweep(timestamp(9_000)).expect("first sweep");
    assert_eq!(first.expired, 1);

    let second = router.sweep(timestamp(9_500)).expect("second sweep");
    assert_eq!(second.expired, 0);
    assert_eq!(second.reconciled, 0);
    assert_eq!(
        harness.worker_store.load_all().expect("worker inbox").len(),
        1,
        "the second sweep delivered nothing"
    );
}

#[test]
fn a_custom_policy_can_expire_a_non_approval_kind() {
    struct AlwaysRefuse;
    impl ExpiryPolicy for AlwaysRefuse {
        fn expire(
            &self,
            row: &InteractionRow,
            _request: &InteractionRequest,
        ) -> Result<Option<ExpiryResolution>, HitlError> {
            Ok(Some(ExpiryResolution {
                principal: PrincipalRef::try_new(
                    "issuer",
                    "custom-expiry",
                    Some(row.tenant_scope.as_ref()),
                )
                .expect("principal"),
                evidence: AuthorizationEvidence::try_new("policy-v9", "decision-v9")
                    .expect("evidence"),
                payload: RawJson::parse(r#"{"answer":"none"}"#).expect("payload"),
            }))
        }
    }

    let harness = harness();
    let interaction_id = harness.seed(InteractionKind::FreeText, Some(timestamp(9_000)));
    let router = harness.router.with_expiry_policy(Arc::new(AlwaysRefuse));

    let report = router.sweep(timestamp(9_000)).expect("sweep");

    assert_eq!(report.expired, 1);
    let delivered = harness.worker_store.load_all().expect("worker inbox");
    assert_eq!(delivered.len(), 1);
    let command: InteractionResolutionCommand =
        serde_json::from_slice(delivered[0].payload.as_ref()).expect("command");
    assert_eq!(command.resolution.principal().subject(), "custom-expiry");
    let row = harness
        .inbox
        .load("tenant-a", &interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Expired);
    assert_eq!(row.resolved_by.as_deref(), Some("custom-expiry"));
}
