//! Actual Rust SDK execution against the same normalized fixtures as Python.
#![cfg(feature = "native-tokio")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod common;
use common::*;
use finstack_ai::runtime::ports::model::{ModelStreamItem, TextDelta};
use finstack_ai_eval::*;
use finstack_ai_kernel::{ContentBlock, TextBlock, Usage};
use finstack_ai_test::{ScriptedModelAction, ScriptedModelPlan};
use std::sync::Arc;

fn response(candidate: bool) -> ScriptedModelPlan {
    let mut response = plan(Some(("USD", if candidate { 7 } else { 10 })));
    let text = if candidate { "answer" } else { "wrong" };
    response.actions[0] = ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
        text: Arc::from(text),
    })));
    let ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed))) =
        &mut response.actions[1]
    else {
        panic!("fixture");
    };
    completed.assistant_content =
        Arc::from([ContentBlock::Text(TextBlock::try_new(text).unwrap())]);
    let tokens = if candidate { 8 } else { 10 };
    completed.usage = Usage::try_new(
        Some(tokens),
        Some(4),
        Some(tokens + 4),
        completed.usage.cost().cloned(),
        std::collections::BTreeMap::default(),
    )
    .unwrap();
    response
}
fn normalized(report: &EvalReport) -> serde_json::Value {
    let mut value = serde_json::to_value(report).unwrap();
    let object = value.as_object_mut().unwrap();
    for field in ["spec_digest", "engine_version", "subject_locks"] {
        object.remove(field);
    }
    value
}
#[tokio::test]
async fn rust_execution_and_rescoring_match_shared_python_parity_fixture() {
    for failing in [false, true] {
        parity(failing).await;
    }
}
async fn parity(failing: bool) {
    let spec: EvalSpec = serde_json::from_str(include_str!(
        "../../../fixtures/compatibility/eval/parity/v1/spec.json"
    ))
    .unwrap();
    let expected: serde_json::Value = serde_json::from_str(if failing {
        include_str!("../../../fixtures/compatibility/eval/parity/v1/failed.json")
    } else {
        include_str!("../../../fixtures/compatibility/eval/parity/v1/completed.json")
    })
    .unwrap();
    let (baseline, baseline_model, _) = setup((0..4).map(|_| response(false)).collect()).await;
    let (candidate, candidate_model, _) = setup(
        (0..4)
            .map(|index| {
                if !failing || index < 2 {
                    response(true)
                } else {
                    ScriptedModelPlan {
                        actions: vec![ScriptedModelAction::Emit(Err(
                            finstack_ai::runtime::ports::model::ModelError::try_new(
                                "fixture_failed",
                                finstack_ai_kernel::ErrorCategory::Model,
                                false,
                                "fixture failure",
                                finstack_ai_kernel::Metadata::empty(),
                            )
                            .unwrap(),
                        ))],
                    }
                }
            })
            .collect(),
    )
    .await;
    let bindings = vec![
        SharedSubject::new("baseline", baseline, request(), &spec.tasks)
            .unwrap()
            .bind(),
        SharedSubject::new("candidate", candidate, request(), &spec.tasks)
            .unwrap()
            .bind(),
    ];
    let store: Arc<dyn EvalStore> = Arc::new(MemoryEvalStore::new());
    let eval = EvalRunner::new(
        spec.clone(),
        Arc::clone(&store),
        bindings.clone(),
        vec![Arc::new(
            ExactMatchScorer::new("exact_match", 1, true, true).unwrap(),
        )],
    )
    .unwrap();
    let run = eval.run().await.unwrap();
    assert_eq!(
        normalized(&EvalReport::from_snapshot(&run.snapshot).unwrap()),
        expected
    );
    let eval = EvalRunner::new(
        spec,
        store,
        bindings,
        vec![Arc::new(
            ExactMatchScorer::new("exact_match", 2, true, true).unwrap(),
        )],
    )
    .unwrap();
    let rescored = eval.rescore().await.unwrap();
    let mut report = EvalReport::from_snapshot(&rescored.snapshot).unwrap();
    assert!(
        report
            .aggregates
            .iter()
            .all(|a| a.metric.scorer_version == 2)
    );
    // Normalizing only the deliberate scorer-version change leaves identical economics.
    for aggregate in &mut report.aggregates {
        aggregate.metric.scorer_version = 1;
    }
    for comparison in &mut report.comparisons {
        for metric in &mut comparison.metrics {
            metric.metric.scorer_version = 1;
        }
    }
    assert_eq!(normalized(&report), expected);
    assert_eq!(baseline_model.request_count(), 4);
    assert_eq!(candidate_model.request_count(), 4);
}
