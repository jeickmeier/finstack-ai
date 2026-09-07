//! Hand-calculated task reductions, exact pairing, failure gates and safe exports.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::float_cmp
)]
use finstack_ai_eval::*;
use finstack_ai_kernel::{Digest, LaneId, OperationLocator, RawJson, RunId, SessionId};
use std::sync::Arc;

fn spec() -> EvalSpec {
    let mut spec: EvalSpec = serde_json::from_str(include_str!(
        "../../../fixtures/compatibility/eval/v1/valid--spec-minimal.json"
    ))
    .unwrap();
    let mut second = spec.tasks[0].clone();
    second.task_id = Arc::from("second");
    spec.tasks[0].input = Arc::from("PRIVATE_INPUT_SENTINEL");
    spec.tasks[0].target = Arc::from("PRIVATE_TARGET_SENTINEL");
    spec.tasks.push(second);
    spec.subjects.push(SubjectDecl {
        subject_id: Arc::from("candidate"),
        lock_digest: None,
    });
    spec.repetitions = 2;
    spec
}
fn grade(value: ScoreMicros, version: u32) -> ScoreSet {
    ScoreSet {
        scorer: Arc::from("exact_match"),
        scorer_version: version,
        failure_code: None,
        scores: vec![Score {
            scorer: Arc::from("exact_match"),
            scorer_version: version,
            name: Arc::from("match"),
            value,
            passed: Some(value == ScoreMicros::ONE),
            explanation: Some(Arc::from("PRIVATE_EXPLANATION_SENTINEL")),
            metadata: Some(
                RawJson::parse(br#"{"secret":"PRIVATE_METADATA_SENTINEL"}"#.as_slice()).unwrap(),
            ),
        }],
    }
}
fn fixture() -> (EvalSpec, MemoryEvalStore) {
    let spec = spec();
    let store = MemoryEvalStore::new();
    store.freeze(&spec).unwrap();
    for subject in &spec.subjects {
        store
            .bind_subject(
                &subject.subject_id,
                Digest::raw_json(subject.subject_id.as_bytes()),
            )
            .unwrap();
    }
    for (index, cell) in spec.cells().unwrap().iter().enumerate() {
        let id = [u8::try_from(index + 1).unwrap(); 16];
        let identity = ExecutionIdentity {
            tenant_scope: Arc::from("tenant"),
            session_id: SessionId::from_bytes(id),
            lane_id: LaneId::from_bytes(id),
        };
        store.reserve(cell, 1, 10).unwrap();
        store.bind_execution(&cell.id, 1, identity.clone()).unwrap();
        let candidate = cell.subject_id.as_ref() == "candidate";
        let micros = if candidate { 7 } else { 10 };
        let record = AttemptRecord {
            cell: Arc::clone(&cell.id),
            sequence: 1,
            status: AttemptStatus::Completed,
            reconciliation: Reconciliation::Terminal,
            failure_code: None,
            locator: Some(
                OperationLocator::try_new(
                    "tenant",
                    identity.session_id,
                    identity.lane_id,
                    RunId::from_bytes(id),
                )
                .unwrap(),
            ),
            usage: MeasuredUsage {
                input_tokens: Some(if candidate { 8 } else { 10 }),
                output_tokens: Some(4),
                total_tokens: Some(if candidate { 12 } else { 14 }),
                cost_by_unit: [(Arc::from("USD"), micros)].into(),
                cost: Some(MeasuredCost {
                    unit: Arc::from("USD"),
                    micros,
                }),
                effects: 1,
                model_effects: 1,
                uncosted_effects: 0,
                complete: true,
            },
            duration_ms: Some(1),
            record_kinds: vec![],
            scores: vec![],
            artifacts: vec![],
            started_at_ms: 10,
            completed_at_ms: 20,
        };
        store.settle(&record).unwrap();
        let value = if candidate || cell.task_id.as_ref() == "second" {
            ScoreMicros::ONE
        } else {
            ScoreMicros::ZERO
        };
        store.append_scores(&cell.id, 1, &grade(value, 1)).unwrap();
    }
    (spec, store)
}
fn gate() -> ThresholdGate {
    ThresholdGate {
        subject: Arc::from("candidate"),
        metric: MetricKey {
            scorer: Arc::from("exact_match"),
            scorer_version: 1,
            name: Arc::from("match"),
        },
        minimum_mean: Some(ScoreMicros::ONE),
        maximum_regression: Some(ScoreMicros::ZERO),
    }
}
#[test]
fn hand_calculated_reducers_and_sample_stderr() {
    let values = [
        ScoreMicros::ONE,
        ScoreMicros::ONE,
        ScoreMicros::ZERO,
        ScoreMicros::ZERO,
        ScoreMicros::ZERO,
    ];
    let pass = RepetitionReducer::PassAtK {
        k: 2,
        pass_threshold_micros: ScoreMicros::ONE,
    };
    assert!((reduce_repetitions(&values, &pass).unwrap().unwrap() - 0.7).abs() < 1e-12);
    assert_eq!(reduce_repetitions(&values[..1], &pass).unwrap(), None);
    assert_eq!(
        reduce_repetitions(
            &values,
            &RepetitionReducer::AtLeastK {
                k: 2,
                pass_threshold_micros: ScoreMicros::ONE
            }
        )
        .unwrap(),
        Some(1.0)
    );
    assert_eq!(
        reduce_repetitions(
            &values,
            &RepetitionReducer::AtLeastK {
                k: 3,
                pass_threshold_micros: ScoreMicros::ONE
            }
        )
        .unwrap(),
        Some(0.0)
    );
    assert!(Statistics::from_values([f64::NAN]).is_err());
    assert_eq!(Statistics::from_values([0.5]).unwrap().stderr, None);
    let statistics = Statistics::from_values([0.0, 1.0]).unwrap();
    assert_eq!(statistics.mean, Some(0.5));
    assert_eq!(statistics.stderr, Some(0.5));
}
#[test]
fn reports_reduce_tasks_then_pair_cells_and_preserve_signed_costs() {
    let (_, store) = fixture();
    let report = EvalReport::from_snapshot(&store.snapshot().unwrap()).unwrap();
    assert_eq!(report.counts.completed, 8);
    let baseline = report
        .aggregates
        .iter()
        .find(|a| a.subject.as_ref() == "baseline")
        .unwrap();
    assert_eq!(baseline.statistics.n, 2, "task count, not repetition count");
    assert_eq!(baseline.statistics.mean, Some(0.5));
    assert_eq!(baseline.statistics.stderr, Some(0.5));
    let paired = &report.comparisons[0];
    assert_eq!(paired.metrics[0].delta.n, 4);
    assert_eq!(paired.metrics[0].delta.mean, Some(0.5));
    assert_eq!(
        (
            paired.metrics[0].wins,
            paired.metrics[0].ties,
            paired.metrics[0].losses
        ),
        (2, 2, 0)
    );
    assert_eq!(paired.input_token_delta.mean, Some(-2.0));
    assert_eq!(paired.costs[0].total_delta_micros, "-12");
    assert_eq!(paired.costs[0].delta_micros.mean, Some(-3.0));
    assert_eq!(report.spending.total_micros["USD"], "68");
    assert!(gate().evaluate(&report).unwrap().passed);
    let mut buffer = Vec::new();
    report.write_json(&mut buffer).unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&buffer).unwrap();
    assert!(wire["aggregates"][0]["statistics"]["mean"].is_string());
    assert_eq!(
        serde_json::from_slice::<EvalReport>(&buffer).unwrap(),
        report
    );
}
#[test]
fn latest_versions_and_failure_gates_do_not_hide_missing_coverage() {
    let (spec, store) = fixture();
    let cell = spec
        .cells()
        .unwrap()
        .into_iter()
        .find(|c| c.subject_id.as_ref() == "candidate")
        .unwrap();
    store
        .append_scores(&cell.id, 1, &grade(ScoreMicros::ZERO, 2))
        .unwrap();
    let report = EvalReport::from_snapshot(&store.snapshot().unwrap()).unwrap();
    let result = gate().evaluate(&report).unwrap();
    assert!(result.incomplete && !result.passed);
    assert_eq!(
        report.comparisons[0]
            .metrics
            .iter()
            .find(|m| m.metric.scorer_version == 1)
            .unwrap()
            .missing_pairs,
        1
    );
    let failure = ScoreSet {
        scorer: Arc::from("exact_match"),
        scorer_version: 2,
        scores: vec![],
        failure_code: Some(Arc::from(EVAL_SCORER_FAILED)),
    };
    store.append_scores(&cell.id, 1, &failure).unwrap();
    let report = EvalReport::from_snapshot(&store.snapshot().unwrap()).unwrap();
    assert!(gate().evaluate(&report).unwrap().scoring_failed);
    assert_eq!(report.counts.completed, 8);
    store
        .append_scores(&cell.id, 1, &grade(ScoreMicros::ONE, 1))
        .unwrap();
    assert!(
        gate()
            .evaluate(&EvalReport::from_snapshot(&store.snapshot().unwrap()).unwrap())
            .unwrap()
            .passed,
        "old failed pass is historical"
    );
    for cell in spec
        .cells()
        .unwrap()
        .iter()
        .filter(|c| c.subject_id.as_ref() == "candidate")
    {
        store
            .append_scores(&cell.id, 1, &grade(ScoreMicros::ZERO, 1))
            .unwrap();
    }
    let result = gate()
        .evaluate(&EvalReport::from_snapshot(&store.snapshot().unwrap()).unwrap())
        .unwrap();
    assert!(
        result.quality_failed
            && !result.scoring_failed
            && !result.infrastructure_failed
            && !result.incomplete
    );
}
#[test]
fn exports_are_exact_and_omit_arbitrary_scorer_and_task_content() {
    let (_, store) = fixture();
    let mut snapshot = store.snapshot().unwrap();
    let usage = &mut snapshot.attempts.values_mut().next().unwrap()[0].usage;
    let exact = 9_007_199_254_740_993;
    usage.cost_by_unit.insert(Arc::from("USD"), exact);
    usage.cost.as_mut().unwrap().micros = exact;
    let mut bytes = Vec::new();
    export_jsonl(&snapshot, &mut bytes).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("PRIVATE_"));
    assert!(text.contains("\"9007199254740993\""));
    assert_eq!(text.lines().count(), 8);
    assert!(
        text.lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
    );
}
#[test]
fn mixed_unknown_cost_and_unsettled_admissions_remain_visible() {
    let (_, store) = fixture();
    let mut snapshot = store.snapshot().unwrap();
    let key = snapshot.attempts.keys().next().unwrap().clone();
    let usage = &mut snapshot.attempts.get_mut(&key).unwrap()[0].usage;
    usage.cost = None;
    usage.cost_by_unit.insert(Arc::from("EUR"), 5);
    let report = EvalReport::from_snapshot(&snapshot).unwrap();
    assert_eq!(report.spending.subject_unknown, 1);
    assert_eq!(report.spending.total_micros["EUR"], "5");
    assert_eq!(report.comparisons[0].missing_cost_pairs, 1);
    snapshot.attempts.remove(&key);
    let report = EvalReport::from_snapshot(&snapshot).unwrap();
    let result = gate().evaluate(&report).unwrap();
    assert_eq!(report.counts.pending, 1);
    assert!(result.infrastructure_failed && result.incomplete && !result.passed);
}
#[test]
fn usage_validator_rejects_fabricated_complete_cost() {
    let (_, store) = fixture();
    let mut usage = store.snapshot().unwrap().attempts.values().next().unwrap()[0]
        .usage
        .clone();
    usage.complete = false;
    assert!(usage.validate().is_err());
    usage.complete = true;
    usage.uncosted_effects = 1;
    assert!(usage.validate().is_err());
    usage.uncosted_effects = 0;
    usage.cost.as_mut().unwrap().micros += 1;
    assert!(usage.validate().is_err());
}
