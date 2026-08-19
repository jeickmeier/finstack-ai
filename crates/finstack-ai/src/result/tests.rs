use std::sync::Arc;

use finstack_ai_kernel::{
    EffectId, JsonSchemaDraft, MessageId, ModelRequestId, OutputEndStrategy,
    StructuredResultSource, ToolCallId, TurnId,
};
use serde::Deserialize;

use super::*;

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct Answer {
    nested: Nested,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct Nested {
    count: u32,
}

fn schema(bytes: &[u8]) -> SchemaRef {
    SchemaRef {
        draft: JsonSchemaDraft::Draft202012,
        schema_version: 1,
        schema_digest: Digest::raw_json(bytes),
    }
}

fn committed(value: RawJson, schema: SchemaRef) -> FinalResultRecorded {
    FinalResultRecorded {
        cycle: 1,
        turn_id: TurnId::from_bytes([1; 16]),
        model_request_id: ModelRequestId::from_bytes([2; 16]),
        effect_id: EffectId::from_bytes([3; 16]),
        message_id: MessageId::from_bytes([4; 16]),
        schema,
        value_digest: value.digest(),
        value,
        source: StructuredResultSource::InternalTool {
            tool_call_id: ToolCallId::from_bytes([5; 16]),
        },
        end_strategy: OutputEndStrategy::Exhaustive,
        skipped_tool_call_ids: Arc::from([]),
    }
}

#[test]
fn decodes_exact_committed_bytes() {
    let schema = schema(b"answer-schema");
    let record = committed(
        RawJson::parse(br#"{"nested":{"count":3}}"#).expect("raw JSON"),
        schema.clone(),
    );
    let result = RunResult::try_from_committed(&schema, &record).expect("result");
    assert_eq!(
        result.decode::<Answer>().expect("typed result"),
        Answer {
            nested: Nested { count: 3 }
        }
    );
}

#[test]
fn reports_schema_and_nested_type_mismatches() {
    let expected_schema = schema(b"answer-schema");
    let record = committed(
        RawJson::parse(br#"{"nested":{"count":"three"}}"#).expect("raw JSON"),
        expected_schema.clone(),
    );
    let mismatch =
        RunResult::try_from_committed(&schema(b"different"), &record).expect_err("schema mismatch");
    assert!(matches!(mismatch, ResultDecodeError::SchemaMismatch { .. }));

    let result = RunResult::try_from_committed(&expected_schema, &record).expect("result");
    let error = result.decode::<Answer>().expect_err("type mismatch");
    match error {
        ResultDecodeError::InvalidValue { path, .. } => {
            assert_eq!(path, "nested.count");
        }
        other => panic!("unexpected error: {other}"),
    }
}
