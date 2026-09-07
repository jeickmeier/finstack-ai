//! Configuration and append-only memory/SQLite conformance.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
use finstack_ai_eval::*;
use finstack_ai_kernel::{Digest, LaneId, OperationLocator, RunId, SessionId};
use std::sync::Arc;

fn spec() -> EvalSpec {
    serde_json::from_str(include_str!(
        "../../../fixtures/compatibility/eval/v1/valid--spec-minimal.json"
    ))
    .expect("spec")
}
fn identity(byte: u8) -> ExecutionIdentity {
    ExecutionIdentity {
        tenant_scope: Arc::from("tenant"),
        session_id: SessionId::from_bytes([byte; 16]),
        lane_id: LaneId::from_bytes([byte; 16]),
    }
}
fn record(
    cell: &Cell,
    sequence: u32,
    status: AttemptStatus,
    reconciliation: Reconciliation,
) -> AttemptRecord {
    AttemptRecord {
        cell: Arc::clone(&cell.id),
        sequence,
        status,
        reconciliation,
        failure_code: None,
        locator: None,
        usage: MeasuredUsage::default(),
        duration_ms: None,
        record_kinds: Vec::new(),
        scores: Vec::new(),
        artifacts: Vec::new(),
        started_at_ms: 10,
        completed_at_ms: 20,
    }
}

fn contract(store: &dyn EvalStore) -> StoreSnapshot {
    let spec = spec();
    let frozen = store.freeze(&spec).expect("freeze");
    assert_eq!(store.freeze(&spec).expect("idempotent"), frozen);
    let mut changed = spec.clone();
    changed.repetitions = 2;
    assert_eq!(
        store.freeze(&changed).expect_err("divergence").code(),
        EVAL_SPEC_DIVERGED
    );
    let digest = Digest::raw_json(b"lock");
    store.bind_subject("baseline", digest).expect("bind");
    store.bind_subject("baseline", digest).expect("equal bind");
    assert_eq!(
        store
            .bind_subject("baseline", Digest::raw_json(b"changed"))
            .expect_err("drift")
            .code(),
        EVAL_SUBJECT_LOCK_MISMATCH
    );
    let cell = &spec.cells().expect("cells")[0];
    store.reserve(cell, 1, 10).expect("reserve");
    assert_eq!(
        store.reserve(cell, 1, 10).expect_err("sequence").code(),
        EVAL_ATTEMPT_SEQUENCE_CONFLICT
    );
    assert_eq!(
        store.reserve(cell, 2, 10).expect_err("unfinished").code(),
        EVAL_ATTEMPT_UNRESOLVED
    );
    store
        .bind_execution(&cell.id, 1, identity(1))
        .expect("bind execution");
    store
        .bind_execution(&cell.id, 1, identity(1))
        .expect("equal bind");
    assert!(store.bind_execution(&cell.id, 1, identity(2)).is_err());
    let unknown = record(
        cell,
        1,
        AttemptStatus::Indeterminate,
        Reconciliation::Unresolved,
    );
    store.settle(&unknown).expect("classify unknown");
    assert_eq!(
        store
            .reserve(cell, 2, 10)
            .expect_err("zero usage is not proof")
            .code(),
        EVAL_ATTEMPT_UNRESOLVED
    );
    let safe = record(
        cell,
        1,
        AttemptStatus::InfraFailed,
        Reconciliation::NoAdmission,
    );
    store.settle(&safe).expect("journal reconciliation");
    store.reserve(cell, 2, 10).expect("proven replacement");
    assert!(
        store.bind_execution(&cell.id, 2, identity(1)).is_err(),
        "replacement cannot reuse prior execution"
    );
    store
        .bind_execution(&cell.id, 2, identity(2))
        .expect("new session");
    let mut completed = record(cell, 2, AttemptStatus::Completed, Reconciliation::Terminal);
    completed.locator = Some(
        OperationLocator::try_new(
            "tenant",
            identity(2).session_id,
            identity(2).lane_id,
            RunId::from_bytes([3; 16]),
        )
        .expect("locator"),
    );
    store.settle(&completed).expect("complete");
    assert_eq!(
        store.reserve(cell, 3, 10).expect_err("finalized").code(),
        EVAL_CELL_FINALIZED
    );
    let scores = ScoreSet {
        scorer: Arc::from("exact_match"),
        scorer_version: 1,
        scores: Vec::new(),
        failure_code: Some(Arc::from("eval_target_invalid")),
    };
    store
        .append_scores(&cell.id, 2, &scores)
        .expect("scoring error separate");
    let mut next = scores.clone();
    next.scorer_version = 2;
    store
        .append_scores(&cell.id, 2, &next)
        .expect("append rescoring");
    let snapshot = store.snapshot().expect("snapshot");
    let attempts = &snapshot.attempts[&cell.id];
    assert_eq!(attempts[1].status, AttemptStatus::Completed);
    assert_eq!(attempts[1].scores, [scores, next]);
    snapshot
}

#[test]
fn memory_store_conformance_and_exclusive_runner() {
    let store = MemoryEvalStore::new();
    let lease = store.acquire_runner().expect("lease");
    assert!(store.acquire_runner().is_err());
    drop(lease);
    let _lease = store.acquire_runner().expect("released");
    contract(&store);
}

#[test]
fn score_and_expansion_validation_are_wire_safe() {
    assert!(serde_json::from_str::<ScoreMicros>("1000001").is_err());
    assert_eq!(
        serde_json::to_string(&ScoreMicros::ONE).expect("json"),
        "1000000"
    );
    let mut spec = spec();
    spec.subjects.push(SubjectDecl {
        subject_id: Arc::from("candidate"),
        lock_digest: None,
    });
    spec.repetitions = 2;
    assert_eq!(
        spec.cells()
            .expect("cells")
            .iter()
            .map(|cell| cell.id.as_ref())
            .collect::<Vec<_>>(),
        [
            "retention::0::baseline",
            "retention::0::candidate",
            "retention::1::baseline",
            "retention::1::candidate"
        ]
    );
    let reordered: EvalSpec =
        serde_json::from_value(serde_json::to_value(&spec).expect("serialize"))
            .expect("deserialize");
    assert_eq!(
        spec.digest().expect("digest"),
        reordered.digest().expect("digest")
    );
    spec.limits.budget_micros = Some(10);
    assert!(spec.validate().is_err());
    spec.limits.budget_unit = Some(Arc::from("USD"));
    spec.validate().expect("finite unit");
    spec.tasks.push(spec.tasks[0].clone());
    assert!(spec.validate().is_err());
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_reopens_identically_and_rejects_unknown_schema() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("eval.sqlite");
    let store = SqliteEvalStore::try_open(&path).expect("open");
    let first = store.acquire_runner().expect("lease");
    let other = SqliteEvalStore::try_open(&path).expect("other handle");
    assert!(other.acquire_runner().is_err());
    let snapshot = contract(&store);
    assert_eq!(snapshot, other.snapshot().expect("tail refresh"));
    drop(first);
    let second = other.acquire_runner().expect("lease released");
    drop(second);
    drop(store);
    drop(other);
    assert_eq!(
        snapshot,
        SqliteEvalStore::try_open(&path)
            .expect("reopen")
            .snapshot()
            .expect("replayed")
    );
    assert_eq!(snapshot, contract(&MemoryEvalStore::new()));
    let connection = rusqlite::Connection::open(&path).expect("database");
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .expect("wal"),
        "wal"
    );
    let rows: u64 = connection
        .query_row("SELECT count(*) FROM eval_events", [], |row| row.get(0))
        .expect("rows");
    assert_eq!(
        rows, 11,
        "equal mutation retries must not append duplicates"
    );
    connection
        .pragma_update(None, "user_version", 999)
        .expect("future version");
    assert!(SqliteEvalStore::try_open(&path).is_err());
}

#[cfg(feature = "sqlite")]
#[test]
fn sqlite_does_not_claim_an_unversioned_foreign_database() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("foreign.sqlite");
    rusqlite::Connection::open(&path)
        .expect("open")
        .execute("CREATE TABLE foreign_data(value TEXT)", [])
        .expect("foreign table");
    assert!(SqliteEvalStore::try_open(&path).is_err());
}

#[test]
fn spec_compatibility_fixtures_enforce_bounds() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/compatibility/eval/v1");
    for path in std::fs::read_dir(directory).expect("fixtures") {
        let path = path.expect("entry").path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let value: EvalSpec =
            serde_json::from_slice(&std::fs::read(&path).expect("bytes")).expect("wire");
        let valid = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .starts_with("valid--");
        assert_eq!(value.validate().is_ok(), valid, "{}", path.display());
    }
}

#[test]
fn concurrent_reservations_have_one_winner() {
    let store = Arc::new(MemoryEvalStore::new());
    let spec = spec();
    store.freeze(&spec).expect("freeze");
    store
        .bind_subject("baseline", Digest::raw_json(b"lock"))
        .expect("bind");
    let cell = spec.cells().expect("cells")[0].clone();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let tasks = (0..2)
        .map(|_| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            let cell = cell.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.reserve(&cell, 1, 10).is_ok()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tasks
            .into_iter()
            .map(|task| u32::from(task.join().expect("thread")))
            .sum::<u32>(),
        1
    );
}

fn grader_contract(store: &dyn EvalStore) -> StoreSnapshot {
    let snapshot = contract(store);
    let cell = snapshot.reservations.keys().next().unwrap();
    let reservation = GraderReservation {
        cell: Arc::clone(cell),
        attempt_sequence: 2,
        scorer: Arc::from("exact_match"),
        scorer_version: 2,
        pass: 3,
        lock_digest: Digest::raw_json(b"grader-lock"),
        request_digest: Digest::raw_json(b"grader-request"),
        started_at_ms: 10,
    };
    let key = reservation.key();
    store.reserve_grader(&reservation).unwrap();
    store.reserve_grader(&reservation).unwrap();
    assert!(store.bind_grader_execution(&key, identity(2)).is_err());
    store.bind_grader_execution(&key, identity(9)).unwrap();
    store.bind_grader_execution(&key, identity(9)).unwrap();
    let mut outcome = GraderOutcome {
        status: AttemptStatus::Indeterminate,
        reconciliation: Reconciliation::Unresolved,
        failure_code: Some(Arc::from(EVAL_ATTEMPT_UNRESOLVED)),
        locator: Some(
            OperationLocator::try_new(
                "tenant",
                identity(9).session_id,
                identity(9).lane_id,
                RunId::from_bytes([10; 16]),
            )
            .unwrap(),
        ),
        usage: MeasuredUsage::default(),
        completed_at_ms: 20,
    };
    store.settle_grader(&key, &outcome).unwrap();
    let failure = ScoreSet {
        scorer: Arc::from("exact_match"),
        scorer_version: 2,
        scores: Vec::new(),
        failure_code: Some(Arc::from(EVAL_JUDGE_OUTPUT_INVALID)),
    };
    assert_eq!(
        store.append_scores(cell, 2, &failure).unwrap_err().code(),
        EVAL_ATTEMPT_UNRESOLVED
    );
    outcome.status = AttemptStatus::SubjectFailed;
    outcome.reconciliation = Reconciliation::Terminal;
    outcome.usage = MeasuredUsage {
        cost: Some(MeasuredCost {
            unit: Arc::from("USD"),
            micros: 5,
        }),
        cost_by_unit: [(Arc::from("USD"), 5)].into(),
        complete: true,
        ..MeasuredUsage::default()
    };
    store.settle_grader(&key, &outcome).unwrap();
    store.settle_grader(&key, &outcome).unwrap();
    store.append_scores(cell, 2, &failure).unwrap();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.attempts[cell][1].status, AttemptStatus::Completed);
    assert_eq!(
        snapshot.graders[&key]
            .outcome
            .as_ref()
            .unwrap()
            .usage
            .cost
            .as_ref()
            .unwrap()
            .micros,
        5
    );
    let mut changed = outcome;
    changed.usage.cost.as_mut().unwrap().micros = 6;
    assert!(store.settle_grader(&key, &changed).is_err());
    snapshot
}
#[test]
fn grader_admissions_preserve_failed_spend_and_block_unresolved_passes() {
    grader_contract(&MemoryEvalStore::new());
}
#[cfg(feature = "sqlite")]
#[test]
fn grader_memory_sqlite_replay_parity() {
    let expected = grader_contract(&MemoryEvalStore::new());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("grader.sqlite");
    {
        let store = SqliteEvalStore::try_open(&path).unwrap();
        assert_eq!(grader_contract(&store), expected);
    }
    assert_eq!(
        SqliteEvalStore::try_open(path).unwrap().snapshot().unwrap(),
        expected
    );
}

#[test]
fn no_admission_cannot_hide_chargeable_usage_from_budget_accounting() {
    let spec = spec();
    let store = MemoryEvalStore::new();
    store.freeze(&spec).unwrap();
    store
        .bind_subject("baseline", Digest::raw_json(b"lock"))
        .unwrap();
    let cell = &spec.cells().unwrap()[0];
    store.reserve(cell, 1, 10).unwrap();
    let mut record = record(
        cell,
        1,
        AttemptStatus::InfraFailed,
        Reconciliation::NoAdmission,
    );
    record.usage.total_tokens = Some(1);
    assert!(store.settle(&record).is_err());
    record.usage.total_tokens = Some(0);
    record.usage.cost_by_unit.insert(Arc::from("USD"), 1);
    assert!(store.settle(&record).is_err());
}
