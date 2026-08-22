//! Failure isolation, index-is-a-hint, and cross-tenant negative proofs.
//!
//! The wake index is a hint, never authoritative: a poisoned row must never
//! stall the tick loop (isolated as one counted failure with backoff), a row
//! that lies about a session's true state must be corrected by the journal
//! the moment the worker actually claims and attaches it, and a delivered
//! response whose payload locator does not match the session it targets must
//! be rejected — and discarded, not left to wedge the loop forever.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    AuthorizationEvidence, InteractionResolution, InteractionResolutionCommand, OperationLocator,
    PrincipalRef, RawJson,
};
use finstack_ai_runtime::commit::CommitCoordinator;
use finstack_ai_runtime::ids::ExternalClock;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::model::Model;
use finstack_ai_runtime::workflow::WorkflowWait;
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use finstack_ai_workflow_local::MemoryCronStore;
use finstack_ai_workflow_worker::{
    FireStore, InboxKind, InboxRow, InboxStore, MemoryWorkerStore, PortsFactory, WakeIndexStore,
    WakeReason, WakeRow, WorkerBuilder, WorkerError, park,
};

use crate::helpers::{
    completed_plan, id, locator, memory_store, park_on_retry_timer, profile, retryable_failure,
    timestamp,
};

struct BindPorts {
    model: Arc<dyn Model>,
}

impl PortsFactory for BindPorts {
    fn bind(
        &self,
        session: finstack_ai_runtime::workflow::WorkflowSession,
    ) -> Result<finstack_ai_runtime::workflow::WorkflowSession, WorkerError> {
        Ok(session.with_ports(
            Arc::clone(&self.model),
            crate::helpers::locked_profile(),
            None,
        ))
    }
}

/// A wake row whose `workflow_kind` has no registered ports factory is
/// isolated as one counted failure, backed off with the same exponential
/// schedule as any other failed resume, and does not stop a healthy row in
/// the same tick from resuming.
#[tokio::test]
async fn a_poisoned_row_backs_off_and_does_not_stall_the_tick() {
    let journal = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
        }],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let mut session = Box::pin(park_on_retry_timer(&journal, &model, &clock, 810)).await;
    let store = Arc::new(MemoryWorkerStore::new());
    park(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    // Rewrite the healthy row's hint as already due before its committed
    // timer, exactly like `tick_reparks_a_row_that_is_due_before_its_committed_timer`
    // in `tests/worker/tick.rs`, so this tick's healthy resume takes the
    // cheap fast path instead of spending the drive budget.
    let mut healthy = store.load_tenant("tenant-a").expect("rows").remove(0);
    healthy.wake_at = Some(timestamp(2_100));
    store.upsert(&healthy).expect("healthy row");

    // A poisoned row for a workflow kind with no registered ports factory.
    // Its claim never touches the journal: `resume_row` fails in the ports
    // lookup itself, before any session attach, so no real run is needed.
    let poisoned = WakeRow {
        tenant_scope: Arc::from("tenant-a"),
        session_id: id(99),
        lane_id: id(2),
        run_id: id(3),
        workflow_kind: Arc::from("unregistered-kind"),
        reason: WakeReason::Timer,
        wake_at: Some(timestamp(2_050)),
        expires_at: None,
        pending_id: Arc::from("effect-poison"),
        leased_by: None,
        lease_expires_at: None,
        attempts: 0,
    };
    store.upsert(&poisoned).expect("poisoned row");

    clock.set(timestamp(2_200));
    let worker = WorkerBuilder::new(
        journal,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_secs(5))
    .register_ports("research", Arc::new(BindPorts { model }))
    .build()
    .expect("worker");

    let report = tokio::time::timeout(Duration::from_secs(5), Box::pin(worker.tick()))
        .await
        .expect("the poisoned row must not stall the tick")
        .expect("tick");

    assert_eq!(
        report.failures, 1,
        "the poisoned row is isolated as one failure"
    );
    assert_eq!(
        report.sessions_resumed, 1,
        "the healthy row still resumes in the same tick"
    );

    let rows = store.load_tenant("tenant-a").expect("rows");
    let poisoned_after = rows
        .iter()
        .find(|row| row.session_id == id(99))
        .expect("the poisoned row survives, backed off, not dropped");
    assert_eq!(poisoned_after.attempts, 1);
    assert_eq!(poisoned_after.leased_by, None);
    assert!(
        poisoned_after.wake_at.expect("wake_at") > timestamp(2_200),
        "backoff pushes the wake time into the future"
    );
    assert!(
        store
            .load_due(timestamp(2_200), 10)
            .expect("due")
            .iter()
            .all(|row| row.session_id != id(99)),
        "the backed-off row is excluded from load_due at `now`"
    );
}

/// A wake row is a hint that can lie: once a parked run is resolved entirely
/// out of band, the row indexing it is stale, but it still looks due. The
/// worker must not trust the row's own claim — claiming and attaching the
/// session must show the journal-authoritative `Terminal` state, and the
/// stale row is corrected (deleted) right then, not left to wedge forever.
#[tokio::test]
async fn a_stale_wake_row_is_corrected_by_the_journal() {
    let journal = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![
            ScriptedModelPlan {
                actions: vec![ScriptedModelAction::Emit(Err(retryable_failure()))],
            },
            completed_plan("done"),
        ],
    ));
    let clock = ExternalClock::new(timestamp(2_000));
    let mut session = Box::pin(park_on_retry_timer(&journal, &model, &clock, 820)).await;
    let store = Arc::new(MemoryWorkerStore::new());
    park(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    // Resolve out-of-band: jump the clock past the committed timer, attach a
    // separate session, and drive it — WITHOUT touching the wake store. The
    // wake row is left lying: still Timer, still due. Once the timer fires
    // and the retried request completes (both automatic), the run parks on
    // `RunPhase::BeforeFinalize` awaiting the finalize acceptance only a
    // facade normally supplies (see `finalize_out_of_band`'s doc comment),
    // so `drive_until_wait` times out there; a bare `CommitCoordinator`
    // stands in for that missing facade to reach Terminal.
    clock.jump(60_000).expect("past due");
    let mut oob = finstack_ai_runtime::workflow::WorkflowSession::trusted_seeded(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator(),
        clock.clone(),
        821,
    )
    .await
    .expect("attach")
    .with_ports(Arc::clone(&model), crate::helpers::locked_profile(), None);
    match crate::helpers::drive_past_current_wait(&mut oob, Duration::from_millis(500)).await {
        Ok(WorkflowWait::Terminal { .. }) => {}
        Ok(other) => panic!("unexpected wait resolving out of band: {other:?}"),
        Err(()) => {
            // The fired retry restarts the model cycle from context
            // preparation — a genuine facade decision — so a bare poll can
            // never carry it further on its own; stand in for that facade.
            oob.abort_owner();
            drop(oob);
            Box::pin(crate::helpers::continue_retry_cycle_out_of_band(
                &journal, &model, &clock, 822,
            ))
            .await;
            Box::pin(crate::helpers::finalize_out_of_band(&journal)).await;
        }
    }

    assert_eq!(
        store.load_tenant("tenant-a").expect("rows").len(),
        1,
        "the stale row is still indexed before the worker claims it"
    );

    let worker = WorkerBuilder::new(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_secs(5))
    .register_ports("research", Arc::new(BindPorts { model }))
    .build()
    .expect("worker");

    let report = tokio::time::timeout(Duration::from_secs(5), Box::pin(worker.tick()))
        .await
        .expect("claiming a stale row must not stall the tick")
        .expect("tick");

    assert_eq!(
        report.failures, 0,
        "the journal's Terminal state is not a failure, it is ground truth"
    );
    assert_eq!(
        report.sessions_resumed, 1,
        "resume_row's Ok(terminal) path unconditionally counts the claim as \
         a resumed session (worker.rs's tick_wake), even though this one \
         only ever observed Terminal and deleted the stale row"
    );
    assert!(
        store.load_tenant("tenant-a").expect("rows").is_empty(),
        "the stale row is deleted once the worker sees Terminal"
    );
}

/// Builds the interaction payload that would normally be delivered for the
/// fixture's parked interaction wait, but with a locator built for a
/// different tenant than the one the row (and the run) actually belong to.
fn cross_tenant_resolution_command(
    interaction_id: finstack_ai_kernel::InteractionId,
) -> InteractionResolutionCommand {
    let wrong_tenant_locator =
        OperationLocator::try_new("tenant-b", id(1), id(2), id(3)).expect("locator");
    InteractionResolutionCommand::try_new(
        wrong_tenant_locator,
        InteractionResolution::try_new(
            interaction_id,
            "resolution-1",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-b")).expect("principal"),
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
            RawJson::parse(r#"{"approved":true}"#).expect("response"),
            None::<&str>,
        )
        .expect("resolution"),
    )
    .expect("command")
}

/// A response delivered for the right session but built against the wrong
/// tenant's locator must be rejected at resolve time (`require_locator`,
/// TM-19) — never silently applied and never left in the inbox to wedge
/// every future tick on the same poisoned entry.
#[tokio::test]
async fn cross_tenant_delivery_is_rejected_at_resolve_time() {
    let journal = memory_store();
    let model: Arc<dyn Model> = Arc::new(ScriptedModel::from_plans(profile(), Vec::new()));
    let clock = ExternalClock::new(timestamp(2_000));

    // Park a session directly on an interaction wait by committing an
    // interaction request straight onto the journal (no tool-approval
    // fixture needed): the worker's `require_locator` rejection is about the
    // *resolve* path, not about how the interaction was requested.
    let interaction_id = crate::helpers::request_interaction(&journal, timestamp(2_000)).await;
    let mut session = finstack_ai_runtime::workflow::WorkflowSession::trusted_seeded(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator(),
        clock.clone(),
        830,
    )
    .await
    .expect("attach")
    .with_ports(Arc::clone(&model), crate::helpers::locked_profile(), None);
    let wait = session.drive_until_wait().await.expect("interaction wait");
    assert!(
        matches!(wait, WorkflowWait::Interaction { .. }),
        "expected an interaction wait, got {wait:?}"
    );
    let store = Arc::new(MemoryWorkerStore::new());
    park(&mut session, store.as_ref(), "research").expect("park");
    drop(session);

    let row = store.load_tenant("tenant-a").expect("rows").remove(0);
    assert_eq!(row.reason, WakeReason::Interaction);
    let command = cross_tenant_resolution_command(interaction_id);
    let payload = serde_json::to_vec(&command).expect("payload");
    store
        .insert(
            &InboxRow::try_new(
                Arc::clone(&row.tenant_scope),
                row.session_id,
                Arc::clone(&row.pending_id),
                InboxKind::Interaction,
                Arc::from(payload.into_boxed_slice()),
                timestamp(2_000),
            )
            .expect("row"),
        )
        .expect("poisoned inbox row");

    let worker = WorkerBuilder::new(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        Arc::new(MemoryCronStore::new()),
        Arc::clone(&store) as Arc<dyn WakeIndexStore>,
        Arc::clone(&store) as Arc<dyn FireStore>,
        Arc::clone(&store) as Arc<dyn InboxStore>,
    )
    .clock(clock.clone())
    .drive_timeout(Duration::from_secs(5))
    .register_ports("research", Arc::new(BindPorts { model }))
    .build()
    .expect("worker");

    let report = Box::pin(worker.tick()).await.expect("tick");

    assert_eq!(report.failures, 0, "report: {report:?}");
    assert_eq!(
        report.responses_rejected, 1,
        "the cross-tenant delivery is a durable ingress rejection"
    );
    assert_eq!(
        report.sessions_resumed, 0,
        "the run is not resolved by a payload for another tenant"
    );

    let rows = store.load_tenant("tenant-a").expect("rows");
    assert_eq!(rows.len(), 1, "the run's own wake row is untouched");
    assert_eq!(
        rows[0].reason,
        WakeReason::Interaction,
        "the run is still parked on its original interaction wait"
    );
    assert!(
        store.load_batch(10).expect("inbox").is_empty(),
        "the rejected response leaves the active inbox"
    );
    assert_eq!(
        store.load_dead_letters(10).expect("dead letters").len(),
        1,
        "operators retain the rejected command and reason"
    );

    let recovered = CommitCoordinator::recover(
        Arc::clone(&journal) as Arc<dyn JournalStore>,
        locator().session_id,
    )
    .await
    .expect("recover");
    assert!(
        recovered.state().pending_interaction.is_some(),
        "the interaction was never resolved"
    );
}

#[test]
fn wake_claim_defaults_fail_closed() {
    use finstack_ai_kernel::{SessionId, Timestamp};

    struct Naked;

    impl WakeIndexStore for Naked {
        fn upsert(&self, _: &WakeRow) -> Result<(), WorkerError> {
            Ok(())
        }
        fn delete(&self, _: &str, _: SessionId) -> Result<(), WorkerError> {
            Ok(())
        }
        fn load_due(&self, _: Timestamp, _: usize) -> Result<Vec<WakeRow>, WorkerError> {
            Ok(Vec::new())
        }
        fn load_tenant(&self, _: &str) -> Result<Vec<WakeRow>, WorkerError> {
            Ok(Vec::new())
        }
        fn contains_interaction(&self, _: &str, _: &str) -> Result<bool, WorkerError> {
            Ok(false)
        }
        fn release(&self, _: &str, _: SessionId, _: &str) -> Result<bool, WorkerError> {
            Ok(false)
        }
        fn record_failure(&self, _: &str, _: SessionId, _: Timestamp) -> Result<(), WorkerError> {
            Ok(())
        }
    }

    let err = Naked
        .try_claim("tenant-a", id(1), "w", timestamp(0), 1_000)
        .expect_err("fail closed");
    assert_eq!(err.code(), "wake_claim_unsupported");
}
