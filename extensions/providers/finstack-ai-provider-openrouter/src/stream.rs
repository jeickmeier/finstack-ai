//! `OpenRouter` Responses SSE event assembly (shared OpenAI-Responses normalization).

use finstack_ai_provider_wire::{OpenAiResponsesAssembly, StreamNormError, StreamNormKind};
use finstack_ai_runtime::ports::model::{ModelError, ModelStreamItem};

use crate::error::{incomplete_error, response_error, stream_error, stream_limit_error};

pub(crate) struct CompletionAssembly {
    inner: OpenAiResponsesAssembly,
}

impl CompletionAssembly {
    pub(crate) fn new(request_id: String, structured: bool) -> Self {
        Self {
            inner: OpenAiResponsesAssembly::new(request_id, structured),
        }
    }

    pub(crate) fn consume(&mut self, data: &str) -> Result<Vec<ModelStreamItem>, ModelError> {
        self.inner.consume(data).map_err(map_norm)
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "map_err passes the normalization error by value"
)]
fn map_norm(error: StreamNormError) -> ModelError {
    match error.kind {
        StreamNormKind::Limit => stream_limit_error(),
        StreamNormKind::Stream => stream_error(error.message),
        StreamNormKind::Response => response_error(error.message),
        StreamNormKind::Incomplete => incomplete_error(),
    }
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::ContentBlock;
    use finstack_ai_runtime::ports::model::ModelStreamItem;
    use serde_json::Value;

    use super::*;

    #[test]
    fn assembles_text_and_function_call_id_on_completed() {
        let mut assembly = CompletionAssembly::new("request-1".to_owned(), false);
        let text_items = assembly
            .consume(r#"{"type":"response.output_text.delta","sequence_number":1,"delta":"hello"}"#)
            .unwrap();
        assert!(matches!(text_items[0], ModelStreamItem::TextDelta(_)));
        assembly
            .consume(
                r#"{"type":"response.output_item.added","sequence_number":2,"output_index":1,"item":{"type":"function_call","call_id":"call_abc","name":"lookup","arguments":""}}"#,
            )
            .unwrap();
        assembly
            .consume(
                r#"{"type":"response.function_call_arguments.delta","sequence_number":3,"output_index":1,"delta":"{\"x\":1}"}"#,
            )
            .unwrap();
        let items = assembly
            .consume(
                r#"{"type":"response.completed","sequence_number":4,"response":{"id":"resp-1","output":[{"type":"message"},{"arguments":"{\"x\":1}","call_id":"call_abc","name":"lookup","type":"function_call"}],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}}"#,
            )
            .unwrap();
        let ModelStreamItem::Completed(response) = &items[1] else {
            panic!("expected completed item");
        };
        assert_eq!(response.completion_id.as_ref(), "resp-1");
        assert!(matches!(
            response.assistant_content[0],
            ContentBlock::Text(_)
        ));
        assert_eq!(response.tool_calls[0].name.as_ref(), "lookup");
        assert_eq!(response.tool_calls[0].arguments.as_str(), r#"{"x":1}"#);
        assert_eq!(
            response.tool_calls[0].provider_call_id.as_deref(),
            Some("call_abc")
        );
        assert_eq!(response.usage.total_tokens(), Some(5));
        let continuation = response.continuation_state.as_ref().expect("continuation");
        let value: Value = serde_json::from_slice(continuation.as_bytes()).unwrap();
        assert_eq!(value["provider"], "openai.responses");
        assert_eq!(value["version"], 1);
        assert_eq!(value["replay_items"].as_array().expect("items").len(), 2);
    }

    #[test]
    fn incomplete_and_failed_are_errors() {
        let mut assembly = CompletionAssembly::new("request-1".to_owned(), false);
        assert_eq!(
            assembly
                .consume(r#"{"type":"response.incomplete","sequence_number":1,"response":{"id":"resp-1","incomplete_details":{"reason":"max_output_tokens"}}}"#)
                .expect_err("incomplete")
                .code(),
            crate::error::RESPONSE_INVALID
        );
        let mut assembly = CompletionAssembly::new("request-2".to_owned(), false);
        assert_eq!(
            assembly
                .consume(
                    r#"{"type":"response.failed","sequence_number":1,"response":{"id":"resp-2"}}"#
                )
                .expect_err("failed")
                .code(),
            crate::error::RESPONSE_INVALID
        );
    }

    #[test]
    fn done_sentinel_is_not_success() {
        let mut assembly = CompletionAssembly::new("request-1".to_owned(), false);
        assert!(assembly.consume("[DONE]").is_err());
    }
}
