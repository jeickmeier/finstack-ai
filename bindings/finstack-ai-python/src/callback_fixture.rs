//! Non-default port-conformance fixture for trusted Python callbacks.

use std::sync::Arc;

use finstack_ai::runtime::{
    AssembledToolStream, AuthorizationContext, CancellationSignal, ModelCallContext, ModelRequest,
    ModelRequestDraft, ModelRequestLimits, ModelResponse, ModelSettings, ModelStreamLimits,
    ModelTerminal, RunCallContext, ToolCallContext, ToolResult, ToolStreamLimits, ToolTerminal,
};
use finstack_ai_kernel::{
    ContentBlock, Digest, EffectId, EffectOutputContract, EffectOutputKind, LaneId, Metadata,
    ModelRequestId, OperationLocator, OutputSpec, PrincipalRef, ProviderIds, RawJson, RetrySafety,
    RunId, SessionId, TextBlock, ToolBatchId, ToolCallBlock, ToolCallId, ToolExecutionMode,
    ToolFailurePolicy, Usage, ValidatedToolCall,
};
use finstack_ai_test::{
    ModelConformanceCase, ToolsetConformanceCase, check_model_conformance,
    check_toolset_conformance,
};
use pyo3::prelude::*;

use crate::callbacks::{PyPythonModel, PyPythonToolset};

fn id_bytes(ordinal: u64) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    bytes
}

fn fixture_error(error: impl std::fmt::Display) -> PyErr {
    pyo3::exceptions::PyAssertionError::new_err(error.to_string())
}

fn run_context(effect_ordinal: u64) -> PyResult<RunCallContext> {
    Ok(RunCallContext {
        locator: OperationLocator::try_new(
            "tenant-python-conformance",
            SessionId::from_bytes(id_bytes(1)),
            LaneId::from_bytes(id_bytes(2)),
            RunId::from_bytes(id_bytes(3)),
        )
        .map_err(fixture_error)?,
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new(
                "finstack-ai-test",
                "python-conformance",
                Some("tenant-python-conformance"),
            )
            .map_err(fixture_error)?,
            authentication_method: Arc::from("fixture"),
            assurance_level: Arc::from("fixture"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([Arc::from("tenant-python-conformance")]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("fixture-v1"),
            decision_id: Arc::from("fixture-decision-v1"),
        },
        effect_id: EffectId::from_bytes(id_bytes(effect_ordinal)),
        attempt: 1,
        deadline: None,
        budget_scope_id: None,
        cancellation: CancellationSignal::new(),
    })
}

#[pyfunction(name = "_callback_port_conformance")]
fn callback_port_conformance<'py>(
    py: Python<'py>,
    model: &Bound<'py, PyPythonModel>,
    toolset: &Bound<'py, PyPythonToolset>,
) -> PyResult<Bound<'py, PyAny>> {
    let model = model.borrow().registration();
    let model_name = model.1.descriptor().models[0].clone();
    let toolset = toolset.borrow().registration();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let expected_model = ModelResponse {
            assistant_content: Arc::from([ContentBlock::Text(
                TextBlock::try_new("conformant").map_err(fixture_error)?,
            )]),
            tool_calls: Arc::from([]),
            usage: Usage::empty(),
            provider_ids: ProviderIds::empty(),
            completion_id: Arc::from("python-conformance-1"),
            continuation_state: None,
        };
        let model_case = ModelConformanceCase {
            model: model_name.clone(),
            request: ModelRequest {
                call: ModelCallContext {
                    run: run_context(4)?,
                    request_id: ModelRequestId::from_bytes(id_bytes(5)),
                },
                draft: ModelRequestDraft {
                    model: model_name,
                    messages: Arc::from([]),
                    tools: Arc::from([]),
                    output: OutputSpec::PlainText,
                    settings: ModelSettings {
                        values: RawJson::parse(b"{}").map_err(fixture_error)?,
                    },
                    limits: ModelRequestLimits {
                        max_input_bytes: 1_048_576,
                        max_input_tokens: 7_168,
                        max_output_tokens: 1_024,
                    },
                },
                continuation_state: None,
            },
            expected_terminal: ModelTerminal::Completed(expected_model),
            stream_limits: ModelStreamLimits::default(),
        };
        check_model_conformance(model.1.as_ref(), model_case)
            .await
            .map_err(|error| pyo3::exceptions::PyAssertionError::new_err(error.to_string()))?;

        let spec = toolset
            .1
            .tools()
            .first()
            .cloned()
            .ok_or_else(|| pyo3::exceptions::PyAssertionError::new_err("toolset is empty"))?;
        let arguments = RawJson::parse(br#"{"value":"ok"}"#).map_err(fixture_error)?;
        let call = ToolCallBlock::try_new(
            ToolCallId::from_bytes(id_bytes(7)),
            &spec.model_name,
            arguments,
        )
        .map_err(fixture_error)?;
        let result = ToolResult {
            output: RawJson::parse(br#"{"value":"ok"}"#).map_err(fixture_error)?,
            is_error: false,
        };
        let tool_case = ToolsetConformanceCase {
            context: ToolCallContext {
                run: run_context(6)?,
                tool_batch_id: ToolBatchId::from_bytes(id_bytes(8)),
                tool_call_id: ToolCallId::from_bytes(id_bytes(7)),
            },
            call: ValidatedToolCall {
                call,
                tool_id: spec.id,
                component: None,
                output_contract: EffectOutputContract {
                    kind: EffectOutputKind::ToolResult,
                    schema_version: 1,
                    schema_digest: Digest::raw_json(b"python-conformance-tool-result"),
                },
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
                execution: ToolExecutionMode::Sequential,
                failure_policy: ToolFailurePolicy::ReturnToModel,
            },
            expected: AssembledToolStream {
                progress: Arc::from([]),
                usage: None,
                terminal: ToolTerminal::Completed(result),
            },
            stream_limits: ToolStreamLimits::default(),
            max_result_bytes: spec.max_result_bytes,
        };
        check_toolset_conformance(toolset.1.as_ref(), tool_case)
            .await
            .map_err(|error| pyo3::exceptions::PyAssertionError::new_err(error.to_string()))?;
        Ok(("model", "toolset"))
    })
}

pub(super) fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(callback_port_conformance, module)?)?;
    Ok(())
}
