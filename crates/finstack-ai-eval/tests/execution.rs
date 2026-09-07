//! Actual SDK execution, journal reconciliation and bounded admission.
#![cfg(feature = "native-tokio")]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
use finstack_ai::runtime::ports::journal::LoadRequest;
use finstack_ai::{Agent, Session};
use finstack_ai_eval::*;
use finstack_ai_kernel::*;
use finstack_ai_test::ScriptedModelAction;
use std::{sync::Arc, time::Duration};

mod common;
use common::*;

fn runner(spec: &EvalSpec, store: Arc<dyn EvalStore>, agent: Agent) -> EvalRunner {
    let binding = SharedSubject::new("baseline", agent, request(), &spec.tasks)
        .unwrap()
        .bind();
    EvalRunner::new(
        spec.clone(),
        store,
        vec![binding],
        vec![Arc::new(
            ExactMatchScorer::new("exact_match", 1, true, true).unwrap(),
        )],
    )
    .unwrap()
}

#[tokio::test]
async fn actual_session_usage_reconstruction_and_resume_skip() {
    let (agent, model, journal) = setup(vec![plan(Some(("USD", 37)))]).await;
    let store: Arc<dyn EvalStore> = Arc::new(MemoryEvalStore::new());
    let runner = runner(&spec(), Arc::clone(&store), agent);
    let result = runner.run().await.unwrap();
    assert!(result.stop_reason.is_none(), "{result:?}");
    let record = result
        .snapshot
        .attempts
        .values()
        .next()
        .unwrap()
        .first()
        .unwrap();
    let reservation = &result.snapshot.reservations[&record.cell][0];
    assert_eq!(record.status, AttemptStatus::Completed);
    assert_eq!(
        record.locator.as_ref().unwrap().session_id,
        reservation.execution.as_ref().unwrap().session_id
    );
    assert_eq!(record.usage.input_tokens, Some(10));
    assert_eq!(record.usage.total_tokens, Some(14));
    assert_eq!(record.usage.cost.as_ref().unwrap().micros, 37);
    assert!(record.usage.complete);
    let loaded = journal
        .load(LoadRequest {
            session_id: record.locator.as_ref().unwrap().session_id,
        })
        .await
        .unwrap();
    assert_eq!(
        loaded
            .committed_batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .filter(|record| matches!(record.body(), RecordBody::RunAccepted(_)))
            .count(),
        1
    );
    let replay = reconcile_attempt(journal, reservation, record.completed_at_ms)
        .await
        .unwrap();
    let mut measured_record = record.clone();
    measured_record.scores.clear();
    assert_eq!(replay.record, measured_record);
    assert_eq!(replay.output.unwrap().text(), "answer");
    let resumed = runner.resume().await.unwrap();
    assert_eq!(resumed.snapshot, result.snapshot);
    assert_eq!(model.request_count(), 1);
}

#[tokio::test]
async fn unknown_cost_stops_next_admission_and_known_zero_is_covered() {
    for (cost, expected_count, reason) in [
        (None, 1, Some(EVAL_COST_UNKNOWN)),
        (Some(("USD", 0)), 2, None),
        (Some(("USD", 3)), 1, Some(EVAL_BUDGET_EXHAUSTED)),
    ] {
        let (agent, model, _) = setup(vec![plan(cost), plan(cost)]).await;
        let mut spec = spec();
        spec.repetitions = 2;
        spec.limits.max_concurrency = 1;
        spec.limits.budget_micros = Some(3);
        spec.limits.budget_unit = Some(Arc::from("USD"));
        let run = runner(&spec, Arc::new(MemoryEvalStore::new()), agent);
        let report = run.run().await.unwrap();
        assert_eq!(report.stop_reason.as_deref(), reason);
        assert_eq!(model.request_count(), expected_count);
    }
}

#[tokio::test]
async fn admitted_unfinished_cannot_repeat_even_with_zero_usage() {
    let mut blocked = plan(None);
    blocked
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::from("pending")));
    let (agent, model, journal) = setup(vec![blocked]).await;
    let control = model.control();
    let store = Arc::new(MemoryEvalStore::new());
    let spec = spec();
    let cell = spec.cells().unwrap().remove(0);
    store.freeze(&spec).unwrap();
    let binding = SharedSubject::new("baseline", agent.clone(), request(), &spec.tasks)
        .unwrap()
        .bind();
    store
        .bind_subject("baseline", binding.lock_digest().unwrap())
        .unwrap();
    store.reserve(&cell, 1, 0).unwrap();
    let session = Session::create(Arc::clone(&journal), "eval-tenant")
        .await
        .unwrap();
    let lane = session.lane("main").await.unwrap();
    store
        .bind_execution(
            &cell.id,
            1,
            ExecutionIdentity {
                tenant_scope: Arc::from("eval-tenant"),
                session_id: session.session_id(),
                lane_id: lane.lane_id(),
            },
        )
        .unwrap();
    let run = lane.run(&agent, request()).unwrap();
    run.close_events();
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries("pending") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let eval = EvalRunner::new(
        spec,
        store.clone(),
        vec![binding],
        vec![Arc::new(
            ExactMatchScorer::new("exact_match", 1, true, true).unwrap(),
        )],
    )
    .unwrap();
    let first = eval.resume().await.unwrap();
    assert_eq!(first.stop_reason.as_deref(), Some(EVAL_ATTEMPT_UNRESOLVED));
    assert_eq!(
        first.snapshot.attempts[&cell.id][0].status,
        AttemptStatus::Indeterminate
    );
    let second = eval.resume().await.unwrap();
    assert_eq!(second.snapshot, first.snapshot);
    assert_eq!(model.request_count(), 1);
    control.release("pending");
    run.result().await.unwrap();
    let final_report = eval.resume().await.unwrap();
    assert_eq!(
        final_report.snapshot.attempts[&cell.id][0].status,
        AttemptStatus::Completed
    );
    assert_eq!(model.request_count(), 1);
}

#[tokio::test]
async fn explicit_cancel_settles_and_dropped_await_keeps_owner() {
    let mut blocked = plan(None);
    blocked
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::from("cancel")));
    let (agent, model, _) = setup(vec![blocked]).await;
    let control = model.control();
    let store: Arc<dyn EvalStore> = Arc::new(MemoryEvalStore::new());
    let eval = runner(&spec(), Arc::clone(&store), agent);
    let caller = eval.clone();
    let waiting = tokio::spawn(async move { caller.run().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries("cancel") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiting.abort();
    assert_eq!(
        store.acquire_runner().err().unwrap().code(),
        EVAL_RUNNER_BUSY
    );
    eval.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store.acquire_runner().is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let snapshot = store.snapshot().unwrap();
    let record = &snapshot.attempts.values().next().unwrap()[0];
    assert_eq!(record.status, AttemptStatus::SubjectFailed);
    assert_eq!(record.reconciliation, Reconciliation::Terminal);
    assert_eq!(model.request_count(), 1);
}

#[tokio::test]
async fn incompatible_finite_budget_rejects_before_dispatch() {
    let (agent, model, _) = setup(vec![plan(Some(("EUR", 3)))]).await;
    let mut spec = spec();
    spec.limits.budget_micros = Some(10);
    spec.limits.budget_unit = Some(Arc::from("USD"));
    let eval = runner(&spec, Arc::new(MemoryEvalStore::new()), agent);
    assert_eq!(eval.run().await.unwrap_err().code(), EVAL_COST_UNKNOWN);
    assert_eq!(model.request_count(), 0);
}

#[tokio::test]
async fn child_lineage_receipts_are_counted_once_and_unfinished_children_block() {
    let mut parent_plan = plan(Some(("USD", 37)));
    parent_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::from("parent")));
    let (parent_agent, parent_model, journal) = setup(vec![parent_plan]).await;
    let mut child_plan = plan(Some(("USD", 11)));
    child_plan
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::from("child")));
    let (child_agent, child_model, _) =
        setup_on_store(vec![child_plan], Arc::clone(&journal)).await;
    let session = Session::create(Arc::clone(&journal), "eval-tenant")
        .await
        .unwrap();
    let lane = session.lane("main").await.unwrap();
    let reservation = AttemptReservation {
        cell: spec().cells().unwrap().remove(0),
        sequence: 1,
        started_at_ms: 0,
        execution: Some(ExecutionIdentity {
            tenant_scope: Arc::from("eval-tenant"),
            session_id: session.session_id(),
            lane_id: lane.lane_id(),
        }),
    };
    let parent = lane.run(&parent_agent, request()).unwrap();
    parent.close_events();
    let parent_control = parent_model.control();
    let child_control = child_model.control();
    tokio::time::timeout(Duration::from_secs(3), async {
        while parent_control.entries("parent") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let child = parent
        .start_child(
            &child_agent,
            request(),
            ChildPlacement::IsolatedChildSession,
            None,
        )
        .await
        .unwrap();
    child.close_events();
    tokio::time::timeout(Duration::from_secs(3), async {
        while child_control.entries("child") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let pending = reconcile_attempt(Arc::clone(&journal), &reservation, 10)
        .await
        .unwrap();
    assert_eq!(pending.record.status, AttemptStatus::Indeterminate);
    child_control.release("child");
    child.result().await.unwrap();
    parent_control.release("parent");
    parent.result().await.unwrap();
    let settled = reconcile_attempt(Arc::clone(&journal), &reservation, 10)
        .await
        .unwrap();
    assert_eq!(settled.record.status, AttemptStatus::Completed);
    assert_eq!(settled.record.usage.cost.as_ref().unwrap().micros, 48);
    assert_eq!(settled.record.usage.model_effects, 2);
    assert_eq!(settled.record.usage.total_tokens, Some(28));
    assert_eq!(
        settled.record,
        reconcile_attempt(journal, &reservation, 10)
            .await
            .unwrap()
            .record
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn eval_restart_child() {
    let Some(path) = std::env::var_os("FINSTACK_EVAL_RESTART_FIXTURE") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let journal = disk_journal(&path);
        let (agent, _, _) =
            setup_on_store(vec![plan(Some(("USD", 37)))], Arc::clone(&journal)).await;
        let store = SqliteEvalStore::try_open(path.join("eval.sqlite")).unwrap();
        let spec = spec();
        let cell = spec.cells().unwrap().remove(0);
        store.freeze(&spec).unwrap();
        let binding = SharedSubject::new("baseline", agent.clone(), request(), &spec.tasks)
            .unwrap()
            .bind();
        store
            .bind_subject("baseline", binding.lock_digest().unwrap())
            .unwrap();
        store.reserve(&cell, 1, 0).unwrap();
        let session = Session::create(journal, "eval-tenant").await.unwrap();
        let lane = session.lane("main").await.unwrap();
        store
            .bind_execution(
                &cell.id,
                1,
                ExecutionIdentity {
                    tenant_scope: Arc::from("eval-tenant"),
                    session_id: session.session_id(),
                    lane_id: lane.lane_id(),
                },
            )
            .unwrap();
        let run = lane.run(&agent, request()).unwrap();
        run.close_events();
        tokio::time::timeout(Duration::from_secs(5), run.result())
            .await
            .unwrap()
            .unwrap();
        // Simulate process loss after the journal commits and before eval settlement.
        std::process::exit(0);
    });
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn independent_process_restart_and_pruned_receipt_coverage() {
    use finstack_ai::runtime::{
        commit::CommitCoordinator,
        ports::journal::{IdempotencyHorizon, PruneRequest, StateSnapshotRequest},
    };
    let directory = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "eval_restart_child", "--nocapture"])
        .env("FINSTACK_EVAL_RESTART_FIXTURE", directory.path())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("restart child timed out");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let journal = disk_journal(directory.path());
    // Same immutable app definition with no available subject responses.
    let (agent, model, _) = setup_on_store(Vec::new(), Arc::clone(&journal)).await;
    let store: Arc<dyn EvalStore> =
        Arc::new(SqliteEvalStore::try_open(directory.path().join("eval.sqlite")).unwrap());
    let eval = runner(&spec(), store, agent);
    let report = eval.resume().await.unwrap();
    let record = &report.snapshot.attempts.values().next().unwrap()[0];
    assert_eq!(record.status, AttemptStatus::Completed);
    assert_eq!(record.usage.cost.as_ref().unwrap().micros, 37);
    assert_eq!(model.request_count(), 0);
    let reservation = &report.snapshot.reservations[&record.cell][0];
    let locator = record.locator.as_ref().unwrap();
    let replay = CommitCoordinator::recover_run(
        Arc::clone(&journal),
        locator.session_id,
        Some(locator.run_id),
    )
    .await
    .unwrap();
    let loaded = journal
        .load(LoadRequest {
            session_id: locator.session_id,
        })
        .await
        .unwrap();
    journal
        .write_state_snapshot(StateSnapshotRequest {
            session_id: locator.session_id,
            state: replay.state().clone(),
            head_checksum: loaded.head_checksum.unwrap(),
            pending_timer_scheduled_at: None,
            last_model_continuation: None,
        })
        .await
        .unwrap();
    journal
        .prune(PruneRequest {
            session_id: locator.session_id,
            horizon: IdempotencyHorizon {
                expire_at: Timestamp::from_unix_ms(1).unwrap(),
            },
        })
        .await
        .unwrap();
    let after = reconcile_attempt(journal, reservation, record.completed_at_ms)
        .await
        .unwrap();
    assert_eq!(after.record.status, AttemptStatus::Completed);
    assert_eq!(after.output.unwrap().text(), "answer");
    assert!(!after.record.usage.complete);
    assert!(after.record.usage.cost.is_none());
}

#[tokio::test]
async fn bounded_concurrency_and_timeout_classify_admitted_work() {
    let mut blocked = plan(None);
    blocked
        .actions
        .insert(0, ScriptedModelAction::Block(Arc::from("bounded")));
    let (agent, model, _) = setup(vec![blocked.clone(), blocked.clone(), plan(None)]).await;
    let mut config = spec();
    config.repetitions = 3;
    config.limits.max_concurrency = 2;
    let store: Arc<dyn EvalStore> = Arc::new(MemoryEvalStore::new());
    let eval = runner(&config, Arc::clone(&store), agent);
    let owner = eval.clone();
    let task = tokio::spawn(async move { owner.run().await.unwrap() });
    let control = model.control();
    tokio::time::timeout(Duration::from_secs(3), async {
        while control.entries("bounded") < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(model.request_count(), 2);
    assert_eq!(store.snapshot().unwrap().reservations.len(), 2);
    eval.cancel();
    let report = task.await.unwrap();
    assert_eq!(report.stop_reason.as_deref(), Some("eval_cancelled"));
    assert_eq!(
        report
            .snapshot
            .attempts
            .values()
            .flatten()
            .filter(|record| record.status == AttemptStatus::SubjectFailed)
            .count(),
        2
    );
    assert_eq!(model.request_count(), 2);

    let (agent, model, _) = setup(vec![blocked]).await;
    let mut config = spec();
    config.limits.attempt_timeout_ms = 50;
    let eval = runner(&config, Arc::new(MemoryEvalStore::new()), agent);
    let report = tokio::time::timeout(Duration::from_secs(3), eval.run())
        .await
        .unwrap()
        .unwrap();
    let record = &report.snapshot.attempts.values().next().unwrap()[0];
    assert_eq!(record.status, AttemptStatus::SubjectFailed);
    assert_eq!(record.reconciliation, Reconciliation::Terminal);
    assert_eq!(model.request_count(), 1);
}

#[tokio::test]
async fn recovered_execution_must_match_the_bound_subject_lock() {
    let (bound, bound_model, journal) = setup(Vec::new()).await;
    let (drifted, drifted_model, _) =
        setup_on_store(vec![plan(Some(("EUR", 3)))], Arc::clone(&journal)).await;
    let spec = spec();
    let cell = spec.cells().unwrap().remove(0);
    let store: Arc<dyn EvalStore> = Arc::new(MemoryEvalStore::new());
    store.freeze(&spec).unwrap();
    let binding = SharedSubject::new("baseline", bound.clone(), request(), &spec.tasks)
        .unwrap()
        .bind();
    store
        .bind_subject("baseline", binding.lock_digest().unwrap())
        .unwrap();
    store.reserve(&cell, 1, 0).unwrap();
    let session = Session::create(journal, "eval-tenant").await.unwrap();
    let lane = session.lane("main").await.unwrap();
    store
        .bind_execution(
            &cell.id,
            1,
            ExecutionIdentity {
                tenant_scope: Arc::from("eval-tenant"),
                session_id: session.session_id(),
                lane_id: lane.lane_id(),
            },
        )
        .unwrap();
    let run = lane.run(&drifted, request()).unwrap();
    run.close_events();
    run.result().await.unwrap();
    let eval = runner(&spec, store, bound);
    let report = eval.resume().await.unwrap();
    assert_eq!(report.stop_reason.as_deref(), Some(EVAL_ATTEMPT_UNRESOLVED));
    assert_eq!(
        report.snapshot.attempts[&cell.id][0].status,
        AttemptStatus::Indeterminate
    );
    assert_eq!(bound_model.request_count(), 0);
    assert_eq!(drifted_model.request_count(), 1);
}
