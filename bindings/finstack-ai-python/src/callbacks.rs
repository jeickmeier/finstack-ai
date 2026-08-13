//! Trusted coarse Python callback adapters for Rust-owned runtime ports.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use finstack_ai::runtime::{
    CancellationSignal, ComponentId, ComponentInvocation, ComponentRef, ContextCallContext,
    ContextContribution, ContextError, ContextProvider, ContextProviderDescriptor, ContextRequest,
    Digest, ErrorCategory, InputCapabilities, InvocationRecovery, Metadata, Middleware,
    MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder, MiddlewareRole,
    Model, ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError, ModelEventStream,
    ModelName, ModelRequest, ModelResponse, ModelStreamItem, ModelTokenEstimate, ModelToolCall,
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode, OrderTier, PortFuture,
    ProviderIds, RawJson, RunEvent, Sensitivity, Stage, StageInput, StageMask, StageOutcome,
    StructuredOutputCapability, TextDelta, TokenEstimatorRef, TokenEstimatorSource,
    ToolCallContext, ToolCallDelta, ToolError, ToolEventStream, ToolResult, ToolSpec,
    ToolStreamItem, Toolset, ToolsetDescriptor, Usage, Version,
};
use futures_util::future::{Either, select};
use futures_util::{FutureExt, pin_mut, stream};
use pyo3::exceptions::{PyRuntimeError as PyBuiltinRuntimeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

const CALLBACK_TIMEOUT_MAX_SECONDS: f64 = 86_400.0;
const CALLBACK_CANCELLATION_GRACE: Duration = Duration::from_millis(100);
const PYTHON_CALLBACK_CANCELLED: &str = "python_callback_cancelled";
const PYTHON_CALLBACK_FAILED: &str = "python_callback_failed";
const PYTHON_CALLBACK_RESULT_INVALID: &str = "python_callback_result_invalid";
const PYTHON_CALLBACK_TIMEOUT: &str = "python_callback_timeout";
const PYTHON_CALLBACK_CONTEXT_SETTLED: &str = "python_callback_context_settled";
const PYTHON_ESTIMATOR_ID: &str = "finstack.python.bytes-upper-bound";
const CALLBACK_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

static CALLBACK_TASK_LOCALS: OnceLock<Result<pyo3_async_runtimes::TaskLocals, Arc<str>>> =
    OnceLock::new();

fn callback_task_locals(py: Python<'_>) -> PyResult<pyo3_async_runtimes::TaskLocals> {
    CALLBACK_TASK_LOCALS
        .get_or_init(|| start_callback_loop(py))
        .clone()
        .map_err(|message| PyBuiltinRuntimeError::new_err(message.to_string()))
}

fn start_callback_loop(py: Python<'_>) -> Result<pyo3_async_runtimes::TaskLocals, Arc<str>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("finstack-python-callbacks".to_owned())
        .spawn(move || {
            Python::attach(|py| {
                let started = (|| {
                    let event_loop = py.import("asyncio")?.call_method0("new_event_loop")?;
                    let locals = pyo3_async_runtimes::TaskLocals::new(event_loop.clone());
                    sender.send(Ok(locals)).map_err(|_| {
                        PyBuiltinRuntimeError::new_err("callback loop receiver closed")
                    })?;
                    event_loop.call_method0("run_forever")?;
                    Ok::<(), PyErr>(())
                })();
                if let Err(error) = started {
                    let _ = sender.send(Err(Arc::from("failed to start Python callback loop")));
                    error.print(py);
                }
            });
        })
        .map_err(|_| Arc::from("failed to spawn Python callback loop"))?;
    py.detach(move || {
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| Arc::from("Python callback loop did not start"))?
    })
}

#[derive(Debug, Clone, Copy)]
enum CallbackFailure {
    Cancelled,
    Exception,
    InvalidResult,
    Timeout,
}

impl CallbackFailure {
    const fn code(self) -> &'static str {
        match self {
            Self::Cancelled => PYTHON_CALLBACK_CANCELLED,
            Self::Exception => PYTHON_CALLBACK_FAILED,
            Self::InvalidResult => PYTHON_CALLBACK_RESULT_INVALID,
            Self::Timeout => PYTHON_CALLBACK_TIMEOUT,
        }
    }

    const fn category(self, component: ErrorCategory) -> ErrorCategory {
        match self {
            Self::Cancelled => ErrorCategory::Cancellation,
            Self::Timeout => ErrorCategory::Deadline,
            Self::Exception => component,
            Self::InvalidResult => ErrorCategory::Validation,
        }
    }

    const fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Python callback was cancelled",
            Self::Exception => "Python callback failed",
            Self::InvalidResult => "Python callback returned an invalid normalized result",
            Self::Timeout => "Python callback exceeded its configured timeout",
        }
    }
}

#[derive(Clone)]
struct CallbackState {
    active: Arc<AtomicBool>,
    settled: finstack_ai::runtime::native_driver::Signal,
}

impl CallbackState {
    fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(true)),
            settled: finstack_ai::runtime::native_driver::Signal::new(),
        }
    }

    fn settle(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.settled.notify_waiters();
        }
    }
}

struct CallbackLease(CallbackState);

impl Drop for CallbackLease {
    fn drop(&mut self) {
        self.0.settle();
    }
}

/// One invocation-scoped immutable context passed to trusted Python callbacks.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "CallbackContext",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyCallbackContext {
    state: CallbackState,
    cancellation: CancellationSignal,
    kind: Arc<str>,
    tenant_scope: Arc<str>,
    session_id: Arc<str>,
    lane_id: Arc<str>,
    run_id: Arc<str>,
    effect_id: Arc<str>,
}

#[pymethods]
impl PyCallbackContext {
    /// Stable coarse callback kind.
    #[getter]
    fn kind(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.kind)
    }

    /// Authenticated tenant scope for this committed effect.
    #[getter]
    fn tenant_scope(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.tenant_scope)
    }

    /// Session identity for this committed effect.
    #[getter]
    fn session_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.session_id)
    }

    /// Lane identity for this committed effect.
    #[getter]
    fn lane_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.lane_id)
    }

    /// Run identity for this committed effect.
    #[getter]
    fn run_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.run_id)
    }

    /// Committed effect identity for this callback.
    #[getter]
    fn effect_id(&self, py: Python<'_>) -> PyResult<&str> {
        self.ensure_active(py)?;
        Ok(&self.effect_id)
    }

    /// Whether cooperative cancellation has reached this callback.
    #[getter]
    fn cancelled(&self, py: Python<'_>) -> PyResult<bool> {
        self.ensure_active(py)?;
        Ok(self.cancellation.is_cancelled())
    }

    /// Wait until cancellation reaches this still-active callback.
    fn wait_cancelled<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.ensure_active(py)?;
        let cancellation = self.cancellation.clone();
        let state = self.state.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let cancelled = cancellation.cancelled().fuse();
            let settled = state.settled.notified().fuse();
            pin_mut!(cancelled, settled);
            match select(cancelled, settled).await {
                Either::Left(((), _)) if state.active.load(Ordering::Acquire) => Ok(()),
                _ => Python::attach(|py| Err(context_settled_error(py))),
            }
        })
    }

    /// Snapshot this active context into ordinary Python strings.
    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        self.ensure_active(py)?;
        let value = PyDict::new(py);
        value.set_item("kind", self.kind.as_ref())?;
        value.set_item("tenant_scope", self.tenant_scope.as_ref())?;
        value.set_item("session_id", self.session_id.as_ref())?;
        value.set_item("lane_id", self.lane_id.as_ref())?;
        value.set_item("run_id", self.run_id.as_ref())?;
        value.set_item("effect_id", self.effect_id.as_ref())?;
        value.set_item("cancelled", self.cancellation.is_cancelled())?;
        Ok(value.unbind())
    }
}

impl PyCallbackContext {
    fn new(kind: &'static str, run: &finstack_ai::runtime::RunCallContext) -> Self {
        Self {
            state: CallbackState::new(),
            cancellation: run.cancellation.clone(),
            kind: Arc::from(kind),
            tenant_scope: Arc::clone(&run.locator.tenant_scope),
            session_id: Arc::from(run.locator.session_id.to_string()),
            lane_id: Arc::from(run.locator.lane_id.to_string()),
            run_id: Arc::from(run.locator.run_id.to_string()),
            effect_id: Arc::from(run.effect_id.to_string()),
        }
    }

    fn ensure_active(&self, py: Python<'_>) -> PyResult<()> {
        if self.state.active.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(context_settled_error(py))
        }
    }
}

fn context_settled_error(py: Python<'_>) -> PyErr {
    let exception = py.get_type::<crate::RuntimeError>();
    match exception.call1(("callback context is no longer active",)) {
        Ok(value) => {
            let _ = value.setattr("code", PYTHON_CALLBACK_CONTEXT_SETTLED);
            let _ = value.setattr("retryable", false);
            let _ = value.setattr("context", py.None());
            PyErr::from_value(value)
        }
        Err(error) => error,
    }
}

struct PythonCallback {
    callable: Py<PyAny>,
    json_loads: Py<PyAny>,
    json_dumps: Py<PyAny>,
    is_async: bool,
    task_locals: Option<pyo3_async_runtimes::TaskLocals>,
    timeout: Duration,
}

impl PythonCallback {
    fn try_new(py: Python<'_>, callable: Py<PyAny>, timeout_seconds: f64) -> PyResult<Self> {
        if !callable.bind(py).is_callable() {
            return Err(PyTypeError::new_err("callback must be callable"));
        }
        if !timeout_seconds.is_finite()
            || timeout_seconds <= 0.0
            || timeout_seconds > CALLBACK_TIMEOUT_MAX_SECONDS
        {
            return Err(PyTypeError::new_err(
                "callback_timeout_seconds must be finite and in (0, 86400]",
            ));
        }
        let inspect = py.import("inspect")?;
        let classifier = inspect.getattr("iscoroutinefunction")?;
        let mut is_async = classifier.call1((callable.bind(py),))?.is_truthy()?;
        if !is_async && let Ok(call_method) = callable.bind(py).getattr("__call__") {
            is_async = classifier.call1((call_method,))?.is_truthy()?;
        }
        let json = py.import("json")?;
        Ok(Self {
            callable,
            json_loads: json.getattr("loads")?.unbind(),
            json_dumps: json.getattr("dumps")?.unbind(),
            is_async,
            task_locals: is_async.then(|| callback_task_locals(py)).transpose()?,
            timeout: Duration::from_secs_f64(timeout_seconds),
        })
    }

    async fn invoke<Request, Response>(
        &self,
        context: PyCallbackContext,
        request: &Request,
    ) -> Result<Response, CallbackFailure>
    where
        Request: Serialize,
        Response: DeserializeOwned,
    {
        let request_bytes =
            serde_json::to_vec(request).map_err(|_| CallbackFailure::InvalidResult)?;
        let cancellation = context.cancellation.clone();
        let lease = CallbackLease(context.state.clone());
        let callback: PortFuture<Result<Py<PyAny>, CallbackFailure>> = if self.is_async {
            let callback = Python::attach(|py| {
                let request_bytes = PyBytes::new(py, &request_bytes);
                let request = self.json_loads.call1(py, (request_bytes,))?;
                let context = Py::new(py, context)?;
                let awaitable = self.callable.call1(py, (context, request))?;
                pyo3_async_runtimes::into_future_with_locals(
                    self.task_locals
                        .as_ref()
                        .expect("async callbacks retain task locals"),
                    awaitable.into_bound(py),
                )
            })
            .map_err(|_| CallbackFailure::Exception)?;
            Box::pin(async move { callback.await.map_err(|_| CallbackFailure::Exception) })
        } else {
            let (callable, json_loads) =
                Python::attach(|py| (self.callable.clone_ref(py), self.json_loads.clone_ref(py)));
            Box::pin(async move {
                finstack_ai::runtime::native_driver::run_blocking(move || {
                    Python::attach(|py| {
                        let request_bytes = PyBytes::new(py, &request_bytes);
                        let request = json_loads.call1(py, (request_bytes,))?;
                        let context = Py::new(py, context)?;
                        callable.call1(py, (context, request))
                    })
                })
                .await
                .map_err(|_| CallbackFailure::Exception)?
                .map_err(|_| CallbackFailure::Exception)
            })
        };

        let bounded = finstack_ai::runtime::native_driver::timeout(self.timeout, callback).fuse();
        let cancelled = cancellation.cancelled().fuse();
        pin_mut!(bounded, cancelled);
        let result = match select(bounded, cancelled).await {
            Either::Left((Ok(Ok(value)), _)) => value,
            Either::Left((Ok(Err(failure)), _)) => return Err(failure),
            Either::Left((Err(_), _)) => return Err(CallbackFailure::Timeout),
            Either::Right(((), callback)) => {
                // Give a cooperative callback one bounded turn to observe the
                // signal before invalidating its invocation context. A callback
                // that ignores cancellation cannot delay the Rust run beyond
                // this fixed grace period.
                let _ = finstack_ai::runtime::native_driver::timeout(
                    CALLBACK_CANCELLATION_GRACE,
                    callback,
                )
                .await;
                return Err(CallbackFailure::Cancelled);
            }
        };
        drop(lease);
        Python::attach(|py| {
            let encoded = self
                .json_dumps
                .call1(py, (result,))?
                .extract::<String>(py)?;
            serde_json::from_str(&encoded).map_err(|error| PyTypeError::new_err(error.to_string()))
        })
        .map_err(|_| CallbackFailure::InvalidResult)
    }

    async fn invoke_batch<Request>(&self, request: &Request) -> Result<(), CallbackFailure>
    where
        Request: Serialize,
    {
        let request_bytes =
            serde_json::to_vec(request).map_err(|_| CallbackFailure::InvalidResult)?;
        let callback: PortFuture<Result<Py<PyAny>, CallbackFailure>> = if self.is_async {
            let callback = Python::attach(|py| {
                let request_bytes = PyBytes::new(py, &request_bytes);
                let request = self.json_loads.call1(py, (request_bytes,))?;
                let awaitable = self.callable.call1(py, (request,))?;
                pyo3_async_runtimes::into_future_with_locals(
                    self.task_locals
                        .as_ref()
                        .expect("async callbacks retain task locals"),
                    awaitable.into_bound(py),
                )
            })
            .map_err(|_| CallbackFailure::Exception)?;
            Box::pin(async move { callback.await.map_err(|_| CallbackFailure::Exception) })
        } else {
            let (callable, json_loads) =
                Python::attach(|py| (self.callable.clone_ref(py), self.json_loads.clone_ref(py)));
            Box::pin(async move {
                finstack_ai::runtime::native_driver::run_blocking(move || {
                    Python::attach(|py| {
                        let request_bytes = PyBytes::new(py, &request_bytes);
                        let request = json_loads.call1(py, (request_bytes,))?;
                        callable.call1(py, (request,))
                    })
                })
                .await
                .map_err(|_| CallbackFailure::Exception)?
                .map_err(|_| CallbackFailure::Exception)
            })
        };
        match finstack_ai::runtime::native_driver::timeout(self.timeout, callback).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(failure)) => Err(failure),
            Err(_) => Err(CallbackFailure::Timeout),
        }
    }
}

/// Normalize the deliberately small provider/native/WASM Pydantic schema subset.
pub(crate) fn normalize_pydantic_schema(
    mut schema: serde_json::Value,
    kind: &str,
) -> Result<serde_json::Value, String> {
    let require_object_root = match kind {
        "tool_input" | "structured_output" => true,
        "tool_output" => false,
        _ => return Err(format!("unsupported Pydantic schema kind: {kind}")),
    };
    normalize_schema_node(&mut schema, "", 0)?;
    if require_object_root
        && schema
            .as_object()
            .and_then(|object| object.get("type"))
            .and_then(serde_json::Value::as_str)
            != Some("object")
    {
        return Err(format!(
            "{kind} schema root must be an object in the portable subset"
        ));
    }
    Ok(schema)
}

#[expect(
    clippy::too_many_lines,
    reason = "portable schema validation is one bounded recursive policy traversal"
)]
fn normalize_schema_node(
    schema: &mut serde_json::Value,
    path: &str,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err(format!(
            "Pydantic schema nesting exceeds 64 levels at {path}"
        ));
    }
    let object = schema
        .as_object_mut()
        .ok_or_else(|| format!("schema at {path} must be an object"))?;
    for keyword in object.keys() {
        if !matches!(
            keyword.as_str(),
            "$defs"
                | "$ref"
                | "$schema"
                | "additionalProperties"
                | "anyOf"
                | "const"
                | "description"
                | "enum"
                | "items"
                | "properties"
                | "required"
                | "title"
                | "type"
        ) {
            return Err(format!(
                "unsupported JSON Schema keyword '{keyword}' at {}",
                pointer(path, keyword)
            ));
        }
    }

    if let Some(draft) = object.get("$schema")
        && draft.as_str() != Some("https://json-schema.org/draft/2020-12/schema")
    {
        return Err(format!(
            "unsupported JSON Schema draft at {}",
            pointer(path, "$schema")
        ));
    }
    if let Some(reference) = object.get("$ref")
        && !reference
            .as_str()
            .is_some_and(|value| value.starts_with("#/$defs/"))
    {
        return Err(format!(
            "unsupported non-local JSON Schema reference at {}",
            pointer(path, "$ref")
        ));
    }
    if let Some(kind) = object.get("type") {
        let Some(kind) = kind.as_str() else {
            return Err(format!(
                "JSON Schema type must be a single string at {}",
                pointer(path, "type")
            ));
        };
        if !matches!(
            kind,
            "array" | "boolean" | "integer" | "null" | "number" | "object" | "string"
        ) {
            return Err(format!(
                "unsupported JSON Schema type '{kind}' at {}",
                pointer(path, "type")
            ));
        }
    }
    for label in ["title", "description"] {
        if object.get(label).is_some_and(|value| !value.is_string()) {
            return Err(format!(
                "JSON Schema {label} must be a string at {}",
                pointer(path, label)
            ));
        }
    }
    if object
        .get("enum")
        .is_some_and(|value| value.as_array().is_none_or(Vec::is_empty))
    {
        return Err(format!(
            "JSON Schema enum must be a non-empty array at {}",
            pointer(path, "enum")
        ));
    }

    let object_keywords = object.contains_key("properties")
        || object.contains_key("required")
        || object.contains_key("additionalProperties");
    if object_keywords && object.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err(format!(
            "object schema at {path} must declare type 'object'"
        ));
    }
    if object.get("type").and_then(serde_json::Value::as_str) == Some("object") {
        let property_names = object
            .get("properties")
            .map(|value| {
                value
                    .as_object()
                    .map(|properties| properties.keys().cloned().collect::<BTreeSet<_>>())
                    .ok_or_else(|| {
                        format!(
                            "JSON Schema properties must be an object at {}",
                            pointer(path, "properties")
                        )
                    })
            })
            .transpose()?
            .unwrap_or_default();
        let required = object
            .get("required")
            .map(|value| {
                let items = value.as_array().ok_or_else(|| {
                    format!(
                        "JSON Schema required must be an array at {}",
                        pointer(path, "required")
                    )
                })?;
                let mut names = BTreeSet::new();
                for item in items {
                    let name = item.as_str().ok_or_else(|| {
                        format!(
                            "JSON Schema required entries must be strings at {}",
                            pointer(path, "required")
                        )
                    })?;
                    if !names.insert(name.to_owned()) {
                        return Err(format!(
                            "duplicate required property '{name}' at {}",
                            pointer(path, "required")
                        ));
                    }
                }
                Ok(names)
            })
            .transpose()?
            .unwrap_or_default();
        if let Some(unknown) = required.difference(&property_names).next() {
            return Err(format!(
                "required property '{unknown}' is not declared at {}",
                pointer(path, "required")
            ));
        }
        if let Some(optional) = property_names.difference(&required).next() {
            return Err(format!(
                "unsupported optional property '{optional}' at {}",
                pointer(&pointer(path, "properties"), optional)
            ));
        }
        if !required.is_empty() {
            object.insert(
                "required".to_owned(),
                serde_json::Value::Array(
                    required
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        match object.get("additionalProperties") {
            None => {
                object.insert(
                    "additionalProperties".to_owned(),
                    serde_json::Value::Bool(false),
                );
            }
            Some(serde_json::Value::Bool(false)) => {}
            Some(_) => {
                return Err(format!(
                    "additionalProperties must be false at {}",
                    pointer(path, "additionalProperties")
                ));
            }
        }
    }

    if let Some(definitions) = object.get_mut("$defs") {
        let definitions = definitions.as_object_mut().ok_or_else(|| {
            format!(
                "JSON Schema $defs must be an object at {}",
                pointer(path, "$defs")
            )
        })?;
        for (name, definition) in definitions {
            normalize_schema_node(
                definition,
                &pointer(&pointer(path, "$defs"), name),
                depth + 1,
            )?;
        }
    }
    if let Some(properties) = object.get_mut("properties") {
        let properties = properties
            .as_object_mut()
            .expect("properties shape was checked above");
        for (name, property) in properties {
            normalize_schema_node(
                property,
                &pointer(&pointer(path, "properties"), name),
                depth + 1,
            )?;
        }
    }
    if let Some(items) = object.get_mut("items") {
        normalize_schema_node(items, &pointer(path, "items"), depth + 1)?;
    }
    if let Some(branches) = object.get_mut("anyOf") {
        let branches = branches.as_array_mut().ok_or_else(|| {
            format!(
                "JSON Schema anyOf must be an array at {}",
                pointer(path, "anyOf")
            )
        })?;
        if branches.is_empty() || branches.len() > 64 {
            return Err(format!(
                "JSON Schema anyOf must contain 1 through 64 branches at {}",
                pointer(path, "anyOf")
            ));
        }
        for (index, branch) in branches.iter_mut().enumerate() {
            normalize_schema_node(
                branch,
                &pointer(&pointer(path, "anyOf"), &index.to_string()),
                depth + 1,
            )?;
        }
    }
    Ok(())
}

fn pointer(path: &str, token: &str) -> String {
    let escaped = token.replace('~', "~0").replace('/', "~1");
    format!("{path}/{escaped}")
}

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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PythonModelToolCall {
    name: String,
    arguments: serde_json::Value,
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
        Box::pin(async move {
            let context = PyCallbackContext::new("model", &request.call.run);
            let output: PythonModelOutput = callback
                .invoke(context, &request.draft)
                .await
                .map_err(model_failure)?;
            let response = model_response(output)
                .map_err(|()| model_failure(CallbackFailure::InvalidResult))?;
            let mut items = Vec::with_capacity(2);
            if let Some(text) = response
                .assistant_content
                .first()
                .and_then(|block| match block {
                    finstack_ai::runtime::ContentBlock::Text(text) => Some(text.text()),
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
        Arc::from([finstack_ai::runtime::ContentBlock::Json(
            finstack_ai::runtime::JsonBlock::new(RawJson::parse(bytes).map_err(|_| ())?),
        )])
    } else if !output.text.is_empty() {
        Arc::from([finstack_ai::runtime::ContentBlock::Text(
            finstack_ai::runtime::TextBlock::try_new(&output.text).map_err(|_| ())?,
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
    .expect("frozen Python model callback error is valid")
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PythonToolOutput {
    output: serde_json::Value,
    #[serde(default)]
    is_error: bool,
}

struct PythonToolsetAdapter {
    callback: Arc<PythonCallback>,
    component: ComponentRef,
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
}

impl Toolset for PythonToolsetAdapter {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: finstack_ai::runtime::ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let callback = Arc::clone(&self.callback);
        Box::pin(async move {
            let context = PyCallbackContext::new("toolset", &ctx.run);
            let output: PythonToolOutput = callback
                .invoke(context, &call)
                .await
                .map_err(tool_failure)?;
            let bytes = serde_json::to_vec(&output.output)
                .map_err(|_| tool_failure(CallbackFailure::InvalidResult))?;
            let result = ToolResult {
                output: RawJson::parse(bytes)
                    .map_err(|_| tool_failure(CallbackFailure::InvalidResult))?,
                is_error: output.is_error,
            };
            Ok(Box::pin(stream::iter([Ok(ToolStreamItem::Completed(result))])) as ToolEventStream)
        })
    }
}

fn tool_failure(failure: CallbackFailure) -> ToolError {
    ToolError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Tool),
        false,
        failure.message(),
        Metadata::empty(),
    )
    .expect("frozen Python tool callback error is valid")
}

/// Trusted Python implementation of the Toolset port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonToolset",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonToolset {
    inner: Arc<PythonToolsetAdapter>,
}

#[pymethods]
impl PyPythonToolset {
    /// Register one trusted coarse Python tool callback and cached tool schemas.
    #[new]
    #[pyo3(signature = (callback, *, component, name, tools, callback_timeout_seconds = 30.0))]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        name: String,
        tools: Py<PyAny>,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        let callback = Arc::new(PythonCallback::try_new(
            py,
            callback,
            callback_timeout_seconds,
        )?);
        let tools = python_value::<Vec<ToolSpec>>(py, &callback, tools)?;
        if tools.is_empty() {
            return Err(PyTypeError::new_err("tools must not be empty"));
        }
        for tool in &tools {
            tool.validate().map_err(configuration_error)?;
        }
        Ok(Self {
            inner: Arc::new(PythonToolsetAdapter {
                callback,
                component,
                descriptor: ToolsetDescriptor {
                    name: Arc::from(name),
                    metadata: Metadata::empty(),
                },
                tools: tools.into(),
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.component.id().to_string()
    }

    /// Return the number of schemas cached at registration.
    #[getter]
    fn tool_count(&self) -> usize {
        self.inner.tools.len()
    }
}

struct PythonContextProviderAdapter {
    callback: Arc<PythonCallback>,
    descriptor: ContextProviderDescriptor,
}

impl ContextProvider for PythonContextProviderAdapter {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let callback = Arc::clone(&self.callback);
        Box::pin(async move {
            let context = PyCallbackContext::new("context", &ctx.run);
            let contribution: ContextContribution = callback
                .invoke(context, &request)
                .await
                .map_err(context_failure)?;
            contribution.to_raw_json()?;
            Ok(contribution)
        })
    }
}

fn context_failure(failure: CallbackFailure) -> ContextError {
    ContextError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Context),
        failure.message(),
        Metadata::empty(),
    )
    .expect("frozen Python context callback error is valid")
}

/// Trusted Python implementation of the `ContextProvider` port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonContextProvider",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonContextProvider {
    inner: Arc<PythonContextProviderAdapter>,
}

#[pymethods]
impl PyPythonContextProvider {
    /// Register one trusted coarse Python context-provider callback.
    #[new]
    #[pyo3(signature = (callback, *, component, callback_timeout_seconds = 30.0, trusted_application_instructions = false))]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        callback_timeout_seconds: f64,
        trusted_application_instructions: bool,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        Ok(Self {
            inner: Arc::new(PythonContextProviderAdapter {
                callback: Arc::new(PythonCallback::try_new(
                    py,
                    callback,
                    callback_timeout_seconds,
                )?),
                descriptor: ContextProviderDescriptor {
                    invocation: callback_invocation(&component),
                    trusted_application_instructions,
                    metadata: Metadata::empty(),
                },
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor.invocation.component.to_string()
    }
}

struct PythonMiddlewareAdapter {
    callback: Arc<PythonCallback>,
    descriptor: MiddlewareDescriptor,
}

impl Middleware for PythonMiddlewareAdapter {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let callback = Arc::clone(&self.callback);
        Box::pin(async move {
            let context = PyCallbackContext::new("middleware", &ctx.run);
            let outcome: StageOutcome = callback
                .invoke(context, &input)
                .await
                .map_err(middleware_failure)?;
            outcome.to_raw_json()?;
            Ok(outcome)
        })
    }
}

fn middleware_failure(failure: CallbackFailure) -> MiddlewareError {
    MiddlewareError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Middleware),
        failure.message(),
        Metadata::empty(),
    )
    .expect("frozen Python middleware callback error is valid")
}

/// Trusted Python implementation of the Middleware port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonMiddleware",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonMiddleware {
    inner: Arc<PythonMiddlewareAdapter>,
}

#[pymethods]
impl PyPythonMiddleware {
    /// Register one trusted coarse Python stage callback.
    #[new]
    #[pyo3(signature = (callback, *, component, stages, priority = 0, callback_timeout_seconds = 30.0))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "PyO3 owns the extracted Python list at the constructor boundary"
    )]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        stages: Vec<String>,
        priority: i32,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        let stages = stages
            .iter()
            .map(|stage| parse_stage(stage))
            .collect::<PyResult<Vec<_>>>()?;
        if stages.is_empty() {
            return Err(PyTypeError::new_err("stages must not be empty"));
        }
        Ok(Self {
            inner: Arc::new(PythonMiddlewareAdapter {
                callback: Arc::new(PythonCallback::try_new(
                    py,
                    callback,
                    callback_timeout_seconds,
                )?),
                descriptor: MiddlewareDescriptor {
                    invocation: callback_invocation(&component),
                    stages: StageMask::from_stages(stages),
                    order: MiddlewareOrder {
                        tier: OrderTier::Standard,
                        priority,
                        before: Arc::from([]),
                        after: Arc::from([]),
                    },
                    role: MiddlewareRole::Standard,
                    metadata: Metadata::empty(),
                },
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor.invocation.component.to_string()
    }
}

struct PythonObserverAdapter {
    callback: Arc<PythonCallback>,
    descriptor: ObserverDescriptor,
}

impl Observer for PythonObserverAdapter {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let callback = Arc::clone(&self.callback);
        let mode = self.descriptor.payload_mode;
        Box::pin(async move {
            let projected = batch
                .iter()
                .map(|event| observer_event(event, mode))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|()| ObserverError::Unavailable)?;
            callback
                .invoke_batch(&projected)
                .await
                .map_err(|_| ObserverError::Unavailable)
        })
    }
}

/// Trusted batched Python implementation of the Observer port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonObserver",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonObserver {
    inner: Arc<PythonObserverAdapter>,
}

#[pymethods]
impl PyPythonObserver {
    /// Register one trusted batched Python observer callback.
    #[new]
    #[pyo3(signature = (callback, *, component, payload_mode = "metadata_only", callback_timeout_seconds = 30.0))]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        payload_mode: &str,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        Ok(Self {
            inner: Arc::new(PythonObserverAdapter {
                callback: Arc::new(PythonCallback::try_new(
                    py,
                    callback,
                    callback_timeout_seconds,
                )?),
                descriptor: ObserverDescriptor {
                    component,
                    payload_mode: parse_payload_mode(payload_mode)?,
                    metadata: Metadata::empty(),
                },
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor.component.id().to_string()
    }
}

fn callback_invocation(component: &ComponentRef) -> ComponentInvocation {
    ComponentInvocation {
        component: component.id().clone(),
        version: component
            .version()
            .expect("Python callback components always have exact versions"),
        configuration_digest: Digest::raw_json(b"{}"),
        recovery: InvocationRecovery::NonRepeatable,
    }
}

fn parse_stage(value: &str) -> PyResult<Stage> {
    match value {
        "before_run" => Ok(Stage::BeforeRun),
        "prepare_context" => Ok(Stage::PrepareContext),
        "before_model" => Ok(Stage::BeforeModel),
        "after_model" => Ok(Stage::AfterModel),
        "before_tool_batch" => Ok(Stage::BeforeToolBatch),
        "after_tool_batch" => Ok(Stage::AfterToolBatch),
        "before_finalize" => Ok(Stage::BeforeFinalize),
        _ => Err(PyTypeError::new_err(format!(
            "unsupported middleware stage: {value}"
        ))),
    }
}

fn parse_payload_mode(value: &str) -> PyResult<ObserverPayloadMode> {
    match value {
        "metadata_only" => Ok(ObserverPayloadMode::MetadataOnly),
        "redacted" => Ok(ObserverPayloadMode::Redacted),
        "full" => Ok(ObserverPayloadMode::Full),
        _ => Err(PyTypeError::new_err(format!(
            "unsupported observer payload_mode: {value}"
        ))),
    }
}

fn observer_event(event: &RunEvent, mode: ObserverPayloadMode) -> Result<serde_json::Value, ()> {
    let mut value = serde_json::to_value(event).map_err(|_| ())?;
    let include_body = match mode {
        ObserverPayloadMode::MetadataOnly => false,
        ObserverPayloadMode::Redacted => event.sensitivity() == Sensitivity::Public,
        ObserverPayloadMode::Full => event.sensitivity() != Sensitivity::Credential,
    };
    if !include_body {
        value.as_object_mut().ok_or(())?.remove("body");
    }
    Ok(value)
}

impl PyPythonToolset {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Toolset>) {
        (self.inner.component.clone(), self.inner.clone())
    }
}

fn python_value<T: DeserializeOwned>(
    py: Python<'_>,
    callback: &PythonCallback,
    value: Py<PyAny>,
) -> PyResult<T> {
    let encoded = callback
        .json_dumps
        .call1(py, (value,))?
        .extract::<String>(py)?;
    serde_json::from_str(&encoded).map_err(|error| PyTypeError::new_err(error.to_string()))
}

fn exact_component(value: &str) -> PyResult<ComponentRef> {
    let id = ComponentId::parse(value).map_err(configuration_error)?;
    Ok(ComponentRef::new(id, Some(CALLBACK_VERSION)))
}

fn configuration_error(error: impl std::fmt::Display) -> PyErr {
    PyTypeError::new_err(error.to_string())
}
