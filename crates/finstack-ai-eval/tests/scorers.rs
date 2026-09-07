//! Built-in grades from deterministic optional output and exact JSON fixtures.
#![cfg(feature = "native-tokio")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use finstack_ai::AgentRunOutput;
use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai_eval::*;
use finstack_ai_kernel::*;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use std::sync::Arc;
fn record() -> AttemptRecord {
    AttemptRecord {
        cell: Arc::from("task::0::baseline"),
        sequence: 1,
        status: AttemptStatus::Completed,
        reconciliation: Reconciliation::Terminal,
        failure_code: None,
        locator: Some(OperationLocator {
            tenant_scope: Arc::from("tenant"),
            session_id: SessionId::from_bytes([1; 16]),
            lane_id: LaneId::from_bytes([2; 16]),
            run_id: RunId::from_bytes([3; 16]),
        }),
        usage: MeasuredUsage {
            complete: true,
            ..MeasuredUsage::default()
        },
        duration_ms: None,
        record_kinds: vec![Arc::from("run_completed")],
        scores: vec![],
        artifacts: vec![],
        started_at_ms: 0,
        completed_at_ms: 1,
    }
}
fn output(text: &str, json: bool) -> AgentRunOutput {
    let content = if json {
        ContentBlock::Json(JsonBlock::new(RawJson::parse(text.as_bytes()).unwrap()))
    } else {
        ContentBlock::Text(TextBlock::try_new(text).unwrap())
    };
    AgentRunOutput {
        locator: record().locator.unwrap(),
        message: Message::try_new(
            MessageId::from_bytes([4; 16]),
            MessageRole::Assistant,
            vec![content],
            Timestamp::from_unix_ms(0).unwrap(),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .unwrap(),
        retry_attempts: 0,
        active_capabilities: Arc::from([]),
        record_kinds: Arc::from([]),
    }
}
async fn grade(
    scorer: &dyn Scorer,
    target: &str,
    answer: Option<AgentRunOutput>,
    complete: bool,
) -> Result<Vec<Score>, EvalError> {
    let sample = TaskSample {
        task_id: Arc::from("task"),
        input: Arc::from("input"),
        target: Arc::from(target),
        metadata: None,
        attachments: Vec::new(),
    };
    let journal: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 1,
            batches_per_session: 1,
            records_per_session: 1,
            snapshot_bytes: 4096,
        })
        .unwrap(),
    );
    let mut record = record();
    record.usage.complete = complete;
    scorer
        .score(&ScoreContext {
            sample: &sample,
            output: answer.as_ref(),
            record: &record,
            journal: &journal,
            grader: None,
        })
        .await
}
#[tokio::test]
async fn text_and_optional_failed_output_cases() {
    let exact = ExactMatchScorer::new("exact", 1, true, false).unwrap();
    assert_eq!(
        grade(&exact, "answer", Some(output(" ANSWER ", false)), true)
            .await
            .unwrap()[0]
            .value,
        ScoreMicros::ONE
    );
    assert_eq!(
        grade(&exact, "answer", None, true).await.unwrap()[0].value,
        ScoreMicros::ZERO
    );
    let includes = IncludesScorer::new("includes", 1, true).unwrap();
    assert_eq!(
        grade(
            &includes,
            "[a]",
            Some(output("value [a] here", false)),
            true
        )
        .await
        .unwrap()[0]
            .value,
        ScoreMicros::ONE
    );
    assert_eq!(
        grade(&includes, "", None, true).await.unwrap_err().code(),
        EVAL_TARGET_INVALID
    );
    let regex = RegexScorer::new("regex", 1, r"^policy: [0-9]+ years$").unwrap();
    assert_eq!(
        grade(
            &regex,
            "unused",
            Some(output("policy: 7 years", false)),
            true
        )
        .await
        .unwrap()[0]
            .value,
        ScoreMicros::ONE
    );
    assert!(RegexScorer::new("regex", 1, "[").is_err());
    let numeric = NumericToleranceScorer::new("numeric", 2, ToleranceBands::relative(0)).unwrap();
    assert_eq!(
        grade(&numeric, "119.7bps", Some(output("1.197%", false)), true)
            .await
            .unwrap()[0]
            .value,
        ScoreMicros::ONE
    );
    assert_eq!(
        grade(&numeric, "not a number", None, true)
            .await
            .unwrap_err()
            .code(),
        EVAL_TARGET_INVALID
    );
}
#[tokio::test]
async fn structured_fields_preserve_decimal_precision_and_weighting() {
    let fields = StructuredFieldScorer::new(
        "fields",
        1,
        vec![
            FieldSpec {
                pointer: Arc::from("/precise"),
                tolerance: FieldTolerance::NumericPpm(0),
                weight: 3,
            },
            FieldSpec {
                pointer: Arc::from("/a~1b/0"),
                tolerance: FieldTolerance::Exact,
                weight: 1,
            },
        ],
    )
    .unwrap();
    let target = r#"{"precise":1.0000000000000001,"a/b":["correct"]}"#;
    let scores = grade(
        &fields,
        target,
        Some(output(r#"{"precise":1.0,"a/b":["correct"]}"#, true)),
        true,
    )
    .await
    .unwrap();
    assert_eq!(scores[0].value, ScoreMicros::ZERO);
    assert_eq!(scores[1].value, ScoreMicros::ONE);
    assert_eq!(scores[2].name.as_ref(), "aggregate");
    assert_eq!(scores[2].value.get(), 250_000);
    assert_eq!(
        grade(&fields, "{}", None, true).await.unwrap_err().code(),
        EVAL_TARGET_INVALID
    );
    assert_eq!(
        grade(&fields, target, None, true).await.unwrap()[2].value,
        ScoreMicros::ZERO
    );
    assert!(
        StructuredFieldScorer::new(
            "fields",
            1,
            vec![FieldSpec {
                pointer: Arc::from("/a~2b"),
                tolerance: FieldTolerance::Exact,
                weight: 1
            }]
        )
        .is_err()
    );
}
#[tokio::test]
async fn record_kinds_require_complete_retained_evidence() {
    let scorer = RecordKindsScorer::new(
        "records",
        1,
        vec![Arc::from("run_completed")],
        vec![Arc::from("run_failed")],
    )
    .unwrap();
    assert_eq!(
        grade(&scorer, "unused", None, true).await.unwrap()[0].value,
        ScoreMicros::ONE
    );
    assert_eq!(
        grade(&scorer, "unused", None, false)
            .await
            .unwrap_err()
            .code(),
        EVAL_HISTORY_INCOMPLETE
    );
}
