//! Executable spec for the HITL router's `pending` and authorized `resolve`.
//!
//! Rows are seeded through [`finstack_ai_workflow_hitl::capture`] with the
//! same approval fixture the capture spec uses, and delivery lands in a real
//! [`WorkflowWorker`]'s inbox — no tick is required, because
//! `deliver_interaction` only inserts an adapter row.

use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, Id, InteractionRequest, InteractionTag, PrincipalRef, RawJson,
};
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::workflow::WorkflowWait;
use finstack_ai_workflow_hitl::{
    HitlError, HitlInboxStore, HitlRouter, InteractionRow, InteractionStatus, MemoryHitlStore,
    ResolutionInput, ResolveAuthorizer, capture,
};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxKind, InboxStore, MemoryWorkerStore, WakeIndexStore, WorkerBuilder,
    WorkflowWorker,
};

use crate::capture::{accepted, approval_request, checkpoint, id, memory_store, timestamp};

struct Harness {
    inbox: Arc<MemoryHitlStore>,
    worker_store: Arc<MemoryWorkerStore>,
    router: HitlRouter,
    interaction_id: Arc<str>,
}

fn principal(tenant: &str) -> PrincipalRef {
    PrincipalRef::try_new("issuer", "subject", Some(tenant)).expect("principal")
}

fn evidence() -> AuthorizationEvidence {
    AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("evidence")
}

fn payload() -> RawJson {
    RawJson::parse(r#"{"approved":true}"#).expect("payload")
}

/// One `Open` approval row for `tenant-a`, plus a live worker over memory
/// adapter tables.
fn harness() -> Harness {
    let inbox = Arc::new(MemoryHitlStore::new());
    let interaction_id: Id<InteractionTag> = id(50);
    let wait = WorkflowWait::Interaction {
        interaction_id,
        request: approval_request(interaction_id, id(51), None),
    };
    assert!(
        capture(
            inbox.as_ref(),
            &checkpoint(),
            &wait,
            accepted().security(),
            timestamp(2_000),
        )
        .expect("capture")
    );

    let worker_store = Arc::new(MemoryWorkerStore::new());
    let worker = Arc::new(
        WorkerBuilder::new(
            memory_store() as Arc<dyn JournalStore>,
            Arc::new(MemoryCronStore::new()),
            Arc::clone(&worker_store) as Arc<dyn WakeIndexStore>,
            Arc::clone(&worker_store) as Arc<dyn FireStore>,
            Arc::clone(&worker_store) as Arc<dyn InboxStore>,
        )
        .build()
        .expect("worker"),
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
        interaction_id: Arc::from(interaction_id.to_canonical_string()),
    }
}

#[test]
fn resolve_delivers_to_the_worker_and_marks_the_row_delivered() {
    let harness = harness();

    let listed = harness.router.pending("tenant-a").expect("pending before");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].interaction_id, harness.interaction_id);
    assert_eq!(listed[0].status, InteractionStatus::Open);

    harness
        .router
        .resolve(
            "tenant-a",
            &harness.interaction_id,
            ResolutionInput {
                resolution_id: Arc::from("resolution-1"),
                principal: principal("tenant-a"),
                evidence: evidence(),
                payload: payload(),
                note: None,
            },
            timestamp(3_000),
        )
        .expect("resolve");

    let delivered = harness.worker_store.load_batch(10).expect("worker inbox");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].kind, InboxKind::Interaction);
    assert_eq!(delivered[0].pending_id, harness.interaction_id);
    assert_eq!(delivered[0].tenant_scope.as_ref(), "tenant-a");
    assert_eq!(delivered[0].received_at, timestamp(3_000));

    let row = harness
        .inbox
        .load("tenant-a", &harness.interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Buffered);
    assert_eq!(row.resolved_by.as_deref(), Some("subject"));
    assert_eq!(row.updated_at, timestamp(3_000));

    assert!(
        harness
            .router
            .pending("tenant-a")
            .expect("pending after")
            .is_empty(),
        "a delivered interaction leaves the pending view"
    );
}

#[test]
fn resolve_refuses_a_principal_from_another_tenant() {
    let harness = harness();

    let error = harness
        .router
        .resolve(
            "tenant-a",
            &harness.interaction_id,
            ResolutionInput {
                resolution_id: Arc::from("resolution-1"),
                principal: principal("tenant-b"),
                evidence: evidence(),
                payload: payload(),
                note: None,
            },
            timestamp(3_000),
        )
        .expect_err("tenant mismatch");

    assert!(matches!(error, HitlError::Unauthorized { .. }));
    assert_eq!(error.code(), "accepted_context_mismatch");
    assert!(
        harness
            .worker_store
            .load_batch(10)
            .expect("worker inbox")
            .is_empty(),
        "a refused resolution delivers nothing"
    );
    let row = harness
        .inbox
        .load("tenant-a", &harness.interaction_id)
        .expect("load")
        .expect("row");
    assert_eq!(row.status, InteractionStatus::Open);
    assert_eq!(row.resolved_by, None);
    assert_eq!(
        harness.router.pending("tenant-a").expect("pending").len(),
        1
    );
}

#[test]
fn resolve_refuses_evidence_that_differs_from_the_accepted_run() {
    let harness = harness();
    let error = harness
        .router
        .resolve(
            "tenant-a",
            &harness.interaction_id,
            ResolutionInput {
                resolution_id: Arc::from("resolution-1"),
                principal: principal("tenant-a"),
                evidence: AuthorizationEvidence::try_new("policy-v1", "different-decision")
                    .expect("evidence"),
                payload: payload(),
                note: None,
            },
            timestamp(3_000),
        )
        .expect_err("accepted evidence mismatch");

    assert_eq!(error.code(), "accepted_context_mismatch");
    assert!(
        harness
            .worker_store
            .load_batch(10)
            .expect("worker inbox")
            .is_empty()
    );
}

#[test]
fn a_custom_authorizer_replaces_the_tenant_default() {
    struct DenyAll;
    impl ResolveAuthorizer for DenyAll {
        fn authorize(
            &self,
            _row: &InteractionRow,
            _request: &InteractionRequest,
            _principal: &PrincipalRef,
        ) -> Result<(), HitlError> {
            Err(HitlError::Unauthorized { code: "deny_all" })
        }
    }

    let harness = harness();
    let router = harness.router.with_authorizer(Arc::new(DenyAll));

    let error = router
        .resolve(
            "tenant-a",
            &harness.interaction_id,
            ResolutionInput {
                resolution_id: Arc::from("resolution-1"),
                principal: principal("tenant-a"),
                evidence: evidence(),
                payload: payload(),
                note: None,
            },
            timestamp(3_000),
        )
        .expect_err("denied");

    assert_eq!(error.code(), "deny_all");
    assert!(
        harness
            .worker_store
            .load_batch(10)
            .expect("worker inbox")
            .is_empty()
    );
}

#[test]
fn resolve_rejects_an_unknown_interaction() {
    let harness = harness();

    let error = harness
        .router
        .resolve(
            "tenant-a",
            &id::<InteractionTag>(99).to_canonical_string(),
            ResolutionInput {
                resolution_id: Arc::from("resolution-1"),
                principal: principal("tenant-a"),
                evidence: evidence(),
                payload: payload(),
                note: None,
            },
            timestamp(3_000),
        )
        .expect_err("unknown");

    assert_eq!(error.code(), "unknown_interaction");
    assert!(
        harness
            .worker_store
            .load_batch(10)
            .expect("worker inbox")
            .is_empty()
    );
}

#[test]
fn resolve_is_not_repeatable_once_delivered() {
    let harness = harness();
    harness
        .router
        .resolve(
            "tenant-a",
            &harness.interaction_id,
            ResolutionInput {
                resolution_id: Arc::from("resolution-1"),
                principal: principal("tenant-a"),
                evidence: evidence(),
                payload: payload(),
                note: None,
            },
            timestamp(3_000),
        )
        .expect("first resolve");

    let error = harness
        .router
        .resolve(
            "tenant-a",
            &harness.interaction_id,
            ResolutionInput {
                resolution_id: Arc::from("resolution-2"),
                principal: principal("tenant-a"),
                evidence: evidence(),
                payload: payload(),
                note: None,
            },
            timestamp(4_000),
        )
        .expect_err("second resolve");

    assert_eq!(error.code(), "not_open");
    assert_eq!(
        harness
            .worker_store
            .load_batch(10)
            .expect("worker inbox")
            .len(),
        1,
        "the second attempt delivered nothing"
    );
}
