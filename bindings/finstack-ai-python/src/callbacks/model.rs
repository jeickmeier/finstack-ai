use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::model::{
    InputCapabilities, Model, ModelCapabilities, ModelContextProfile, ModelDeferral,
    ModelDescriptor, ModelError, ModelEventStream, ModelName, ModelRequest, ModelResponse,
    ModelStreamItem, ModelTokenEstimate, ModelToolCall, StructuredOutputCapability, TextDelta,
    TokenEstimatorRef, TokenEstimatorSource, ToolCallDelta,
};
use finstack_ai_kernel::{
    ComponentRef, ErrorCategory, ExternalHandleRef, Metadata, ProviderIds, RawJson,
    ReconciliationPolicy, Usage,
};
use futures_util::stream;
use pyo3::prelude::*;
use serde::Deserialize;

use super::context::PyCallbackContext;
use super::engine::{CallbackFailure, PythonCallback};
use super::shared::{configuration_error, exact_component};

const PYTHON_ESTIMATOR_ID: &str = "finstack.python.bytes-upper-bound";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PythonModelOutput {
    #[serde(default)]
    text: String,
    #[serde(default)]
    json: Option<serde_json::Value>,
    completion_id: String,
    #[serde(default)]
    tool_calls: Vec<PythonModelToolCall>,
    #[serde(default)]
    deferred: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PythonModelToolCall {
    name: String,
    arguments: serde_json::Value,
    #[serde(default)]
    provider_call_id: Option<String>,
}

struct PythonModelAdapter {
    callback: Arc<PythonCallback>,
    component: ComponentRef,
    descriptor: ModelDescriptor,
    capabilities: ModelCapabilities,
}

impl Model for PythonModelAdapter {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor.clone()
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        self.capabilities.clone()
    }

    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        if !self.descriptor.models.contains(model) {
            return Err(model_failure(CallbackFailure::InvalidResult));
        }
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len())
                .map_err(|_| model_failure(CallbackFailure::InvalidResult))?,
            estimator: self.capabilities.context_profile.estimator.clone(),
        })
    }

    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        let callback = Arc::clone(&self.callback);
        let component = self.component.clone();
        Box::pin(async move {
            let context = PyCallbackContext::new("model", &request.call.run);
            let output: PythonModelOutput = callback
                .invoke(context, &request.draft)
                .await
                .map_err(model_failure)?;
            if let Some(job) = output.deferred.as_deref() {
                let handle = ExternalHandleRef::try_new(
                    component.id().clone(),
                    job,
                    RawJson::parse(b"{}")
                        .map_err(|_| model_failure(CallbackFailure::InvalidResult))?,
                )
                .map_err(|_| model_failure(CallbackFailure::InvalidResult))?;
                let items = vec![ModelStreamItem::Deferred(ModelDeferral {
                    handle,
                    reconciliation: ReconciliationPolicy::CallbackOnly,
                    next_poll_at: None,
                    expires_at: None,
                })];
                return Ok(Box::pin(stream::iter(items.into_iter().map(Ok))) as ModelEventStream);
            }
            let response = model_response(output)
                .map_err(|()| model_failure(CallbackFailure::InvalidResult))?;
            let mut items = Vec::with_capacity(2);
            if let Some(text) = response
                .assistant_content
                .first()
                .and_then(|block| match block {
                    finstack_ai_kernel::ContentBlock::Text(text) => Some(text.text()),
                    _ => None,
                })
                && !text.is_empty()
            {
                items.push(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from(text),
                }));
            }
            for (index, call) in response.tool_calls.iter().enumerate() {
                items.push(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                    index: u32::try_from(index)
                        .map_err(|_| model_failure(CallbackFailure::InvalidResult))?,
                    name: Some(Arc::clone(&call.name)),
                    arguments_delta: Arc::from(call.arguments.as_str()),
                    provider_call_id: None,
                }));
            }
            items.push(ModelStreamItem::Completed(response));
            Ok(Box::pin(stream::iter(items.into_iter().map(Ok))) as ModelEventStream)
        })
    }
}

fn model_response(output: PythonModelOutput) -> Result<ModelResponse, ()> {
    if output.completion_id.is_empty() || output.completion_id.as_bytes().contains(&0) {
        return Err(());
    }
    if !output.text.is_empty() && output.json.is_some() {
        return Err(());
    }
    let assistant_content = if let Some(value) = output.json {
        let bytes = serde_json::to_vec(&value).map_err(|_| ())?;
        Arc::from([finstack_ai_kernel::ContentBlock::Json(
            finstack_ai_kernel::JsonBlock::new(RawJson::parse(bytes).map_err(|_| ())?),
        )])
    } else if !output.text.is_empty() {
        Arc::from([finstack_ai_kernel::ContentBlock::Text(
            finstack_ai_kernel::TextBlock::try_new(&output.text).map_err(|_| ())?,
        )])
    } else {
        Arc::from([])
    };
    let tool_calls = output
        .tool_calls
        .into_iter()
        .map(|call| {
            let arguments = serde_json::to_vec(&call.arguments).map_err(|_| ())?;
            Ok(ModelToolCall {
                name: Arc::from(call.name),
                arguments: RawJson::parse(arguments).map_err(|_| ())?,
                provider_call_id: call.provider_call_id.map(Arc::from),
            })
        })
        .collect::<Result<Vec<_>, ()>>()?;
    Ok(ModelResponse {
        assistant_content,
        tool_calls: tool_calls.into(),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from(output.completion_id),
        continuation_state: None,
    })
}

fn model_failure(failure: CallbackFailure) -> ModelError {
    ModelError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Model),
        false,
        failure.message(),
        Metadata::empty(),
    )
    .unwrap_or_else(ModelError::from)
}

/// Trusted Python implementation of the Model port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonModel",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonModel {
    inner: Arc<PythonModelAdapter>,
}

#[pymethods]
impl PyPythonModel {
    /// Register one trusted coarse Python model callback.
    #[new]
    #[pyo3(signature = (callback, *, component, provider, model, callback_timeout_seconds = 30.0, hard_input_bytes = 1_048_576, context_window_tokens = 8_192, max_output_tokens = 1_024))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        provider: String,
        model: String,
        callback_timeout_seconds: f64,
        hard_input_bytes: u64,
        context_window_tokens: u64,
        max_output_tokens: u64,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        let model = ModelName::try_new(model).map_err(configuration_error)?;
        let estimator = TokenEstimatorRef {
            id: Arc::from(PYTHON_ESTIMATOR_ID),
            version: Arc::from("1"),
            source: TokenEstimatorSource::ConservativeUpperBound,
        };
        let profile = ModelContextProfile {
            provider: Arc::from(provider.as_str()),
            model: model.clone(),
            hard_input_bytes,
            context_window_tokens,
            max_output_tokens,
            reserved_output_tokens: max_output_tokens,
            provider_overhead_tokens: 0,
            estimator,
        };
        let capabilities = ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: profile,
            native_tool_calls: true,
            parallel_tool_calls: true,
            structured_output: StructuredOutputCapability::Native,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities: BTreeSet::new(),
        };
        let descriptor = ModelDescriptor {
            provider: Arc::from(provider),
            models: Arc::from([model]),
            metadata: Metadata::empty(),
        };
        descriptor.validate().map_err(configuration_error)?;
        Ok(Self {
            inner: Arc::new(PythonModelAdapter {
                callback: Arc::new(PythonCallback::try_new(
                    py,
                    callback,
                    callback_timeout_seconds,
                )?),
                component,
                descriptor,
                capabilities,
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.component.id().to_string()
    }

    /// Exact registered model name.
    #[getter]
    fn model(&self) -> &str {
        self.inner.descriptor.models[0].as_str()
    }
}

impl PyPythonModel {
    pub(crate) fn model_name(&self) -> ModelName {
        self.inner.descriptor.models[0].clone()
    }

    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Model>) {
        (self.inner.component.clone(), self.inner.clone())
    }
}
