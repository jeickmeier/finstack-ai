//! `PyO3` control handles for the Rust-owned `finstack-ai` engine.

#![warn(missing_docs)]

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use finstack_ai::runtime::{
    AgentId, BundleId, ComponentId, ComponentRef, JournalStore, Model, ModelName, OperationLocator,
    RunEvent, RunEventClass, Version,
};
use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_TIMEOUT, Agent, AgentRunError,
    AgentRunOutput, AgentRunRequest, PrincipalRef, RunSecurityContext,
};
use finstack_ai_provider_openai_compatible::{
    EndpointKind, OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiModelConfig,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyStopAsyncIteration};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

#[cfg(feature = "benchmark-fixture")]
mod benchmark_fixture;

const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
const OPENAI_COMPATIBLE_PROVIDER: &str = "openai-compatible";
const DEFAULT_TIMEOUT_SECONDS: f64 = 30.0;
const DEFAULT_MAX_CYCLES: u64 = 16;
const MAX_TIMEOUT_SECONDS: f64 = 86_400.0;
const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

create_exception!(
    _finstack_ai,
    FinstackError,
    PyException,
    "Base error raised by the Rust-owned finstack-ai engine."
);
create_exception!(
    _finstack_ai,
    ConfigurationError,
    FinstackError,
    "Invalid immutable agent or run configuration."
);
create_exception!(
    _finstack_ai,
    RuntimeError,
    FinstackError,
    "Rust runtime execution failure."
);
create_exception!(
    _finstack_ai,
    CancelledError,
    FinstackError,
    "Run reached its durable cancelled terminal state."
);
create_exception!(
    _finstack_ai,
    TimeoutError,
    FinstackError,
    "Run exceeded its configured operational deadline."
);

#[pyfunction]
#[pyo3(text_signature = "()")]
fn health() -> &'static str {
    "ok"
}

#[pyfunction]
#[pyo3(text_signature = "()")]
fn linked_providers() -> (&'static str,) {
    let _ = finstack_ai_provider_openai_compatible::ENDPOINT_QUIRKS_VERSION;
    (OPENAI_COMPATIBLE_PROVIDER,)
}

#[pyfunction]
#[pyo3(text_signature = "()")]
fn build_metadata(py: Python<'_>) -> PyResult<Py<PyDict>> {
    let metadata = PyDict::new(py);
    metadata.set_item("version", ENGINE_VERSION)?;
    metadata.set_item("engine_version", ENGINE_VERSION)?;
    metadata.set_item("implementation", "cpython")?;
    metadata.set_item("free_threaded", cfg!(Py_GIL_DISABLED))?;
    metadata.set_item(
        "provider_quirks_version",
        finstack_ai_provider_openai_compatible::ENDPOINT_QUIRKS_VERSION,
    )?;
    Ok(metadata.unbind())
}

/// Rust-owned resolved agent handle.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Agent", frozen)]
struct PyAgent {
    inner: Arc<Agent>,
    model: ModelName,
}

#[pymethods]
impl PyAgent {
    /// Construct a keyless Rust-backed OpenAI-compatible agent.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None))]
    fn openai_compatible(
        py: Python<'_>,
        base_url: String,
        model: String,
        instruction: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_openai_compatible_agent(base_url, model, instruction).await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Start a run and return its shared control handle immediately.
    #[pyo3(signature = (input, *, timeout_seconds = DEFAULT_TIMEOUT_SECONDS, max_cycles = DEFAULT_MAX_CYCLES))]
    fn start(
        &self,
        py: Python<'_>,
        input: String,
        timeout_seconds: f64,
        max_cycles: u64,
    ) -> PyResult<PyRun> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        py.detach(move || {
            let request = run_request(&model, input, timeout_seconds, max_cycles)?;
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            agent.start(request).map(|inner| PyRun { inner })
        })
        .map_err(|error| agent_error(py, &error, None))
    }

    /// Execute one run and await its committed result.
    #[pyo3(signature = (input, *, timeout_seconds = DEFAULT_TIMEOUT_SECONDS, max_cycles = DEFAULT_MAX_CYCLES))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        input: String,
        timeout_seconds: f64,
        max_cycles: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let request = match run_request(&model, input, timeout_seconds, max_cycles) {
                Ok(request) => request,
                Err(error) => return Python::attach(|py| Err(agent_error(py, &error, None))),
            };
            let run = match agent.start(request) {
                Ok(run) => run,
                Err(error) => return Python::attach(|py| Err(agent_error(py, &error, None))),
            };
            let locator = run.locator().clone();
            let result = run.result().await;
            Python::attach(|py| result_to_python_with_locator(py, result, Some(&locator)))
        })
    }
}

/// Shared control handle for one Rust-owned run.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Run", frozen)]
struct PyRun {
    inner: finstack_ai::AgentRun,
}

#[pymethods]
impl PyRun {
    /// Immutable session/lane/run snapshot.
    #[getter]
    fn session(&self) -> PySession {
        PySession {
            locator: Arc::new(self.inner.locator().clone()),
        }
    }

    /// Wait for the retained terminal result.
    fn result<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            let result = run.result().await;
            Python::attach(|py| result_to_python_with_locator(py, result, Some(&locator)))
        })
    }

    /// Submit idempotent durable cancellation.
    fn cancel<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.cancel().await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, Some(&locator)))),
            }
        })
    }

    /// Return the batch-first asynchronous event iterator.
    fn events(&self) -> PyEventIterator {
        PyEventIterator {
            run: self.inner.clone(),
        }
    }

    /// Close event observation without cancelling execution.
    fn close_events<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            run.close_events();
            Ok(())
        })
    }
}

/// Batch-first asynchronous event iterator.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "EventBatchIterator",
    frozen
)]
struct PyEventIterator {
    run: finstack_ai::AgentRun,
}

#[pymethods]
impl PyEventIterator {
    fn __aiter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __anext__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.run.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.next_event_batch().await {
                Ok(Some(batch)) => Python::attach(|py| {
                    Py::new(
                        py,
                        PyEventBatch {
                            inner: batch,
                            serialized: OnceLock::new(),
                        },
                    )
                }),
                Ok(None) => Err(PyStopAsyncIteration::new_err(())),
                Err(error) => Err(Python::attach(|py| agent_error(py, &error, Some(&locator)))),
            }
        })
    }
}

/// Immutable operation identity snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Session", frozen)]
struct PySession {
    locator: Arc<OperationLocator>,
}

#[pymethods]
impl PySession {
    #[getter]
    fn tenant_scope(&self) -> &str {
        &self.locator.tenant_scope
    }

    #[getter]
    fn session_id(&self) -> String {
        self.locator.session_id.to_string()
    }

    #[getter]
    fn lane_id(&self) -> String {
        self.locator.lane_id.to_string()
    }

    #[getter]
    fn run_id(&self) -> String {
        self.locator.run_id.to_string()
    }

    /// Serialize the immutable snapshot on explicit request.
    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        locator_dict(py, &self.locator)
    }
}

/// Immutable successful terminal result snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "RunResult", frozen)]
struct PyRunResult {
    inner: AgentRunOutput,
}

#[pymethods]
impl PyRunResult {
    #[getter]
    fn text(&self) -> String {
        self.inner.text()
    }

    #[getter]
    fn session(&self) -> PySession {
        PySession {
            locator: Arc::new(self.inner.locator.clone()),
        }
    }

    /// Serialize the immutable snapshot on explicit request.
    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let value = locator_dict(py, &self.inner.locator)?;
        value.bind(py).set_item("text", self.inner.text())?;
        Ok(value)
    }
}

/// Immutable runtime event snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Event", frozen)]
struct PyEvent {
    inner: RunEvent,
}

#[pymethods]
impl PyEvent {
    #[getter]
    fn kind(&self) -> PyResult<String> {
        event_json_value(&self.inner).and_then(|value| {
            value
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .ok_or_else(|| PyException::new_err("event kind is missing"))
        })
    }

    #[getter]
    fn event_class(&self) -> &'static str {
        match self.inner.class() {
            RunEventClass::DurableDerived => "durable_derived",
            RunEventClass::Transient => "transient",
        }
    }

    #[getter]
    fn transient_sequence(&self) -> u64 {
        self.inner.transient_sequence()
    }

    #[getter]
    fn durable_sequence(&self) -> Option<u64> {
        self.inner.durable_sequence()
    }

    /// Serialize the complete immutable event on explicit request.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner)
            .map_err(|_| PyException::new_err("event serialization failed"))
    }
}

/// Immutable bounded transport batch.
#[pyclass(module = "finstack_ai._finstack_ai", name = "EventBatch", frozen)]
struct PyEventBatch {
    inner: finstack_ai::runtime::EventBatch,
    serialized: OnceLock<Result<Arc<[u8]>, Arc<str>>>,
}

#[pymethods]
impl PyEventBatch {
    #[getter]
    fn first_sequence(&self) -> u64 {
        self.inner.first_sequence()
    }

    #[getter]
    fn last_sequence(&self) -> u64 {
        self.inner.last_sequence()
    }

    #[getter]
    fn dropped_progress(&self) -> u64 {
        self.inner.dropped_progress()
    }

    fn __len__(&self) -> usize {
        self.inner.events().len()
    }

    /// Expand this batch into immutable event snapshots on explicit request.
    fn events(&self) -> Vec<PyEvent> {
        self.inner
            .events()
            .iter()
            .cloned()
            .map(|inner| PyEvent { inner })
            .collect()
    }

    /// Serialize the complete batch on explicit request.
    fn to_json(&self) -> PyResult<String> {
        let bytes = self.serialized_bytes()?;
        std::str::from_utf8(bytes)
            .map(ToOwned::to_owned)
            .map_err(|_| PyException::new_err("event batch serialization produced invalid UTF-8"))
    }

    /// Serialize the complete batch once and copy it directly into Python bytes.
    fn to_json_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        self.serialized_bytes()
            .map(|bytes| PyBytes::new(py, bytes.as_ref()))
    }
}

impl PyEventBatch {
    fn serialized_bytes(&self) -> PyResult<&Arc<[u8]>> {
        self.serialized
            .get_or_init(|| {
                serde_json::to_vec(self.inner.events())
                    .map(Arc::from)
                    .map_err(|_| Arc::from("event batch serialization failed"))
            })
            .as_ref()
            .map_err(|message| PyException::new_err(message.to_string()))
    }
}

async fn build_openai_compatible_agent(
    base_url: String,
    model: String,
    instruction: Option<String>,
) -> Result<PyAgent, AgentRunError> {
    let config = OpenAiCompatibleConfig::try_new(base_url, EndpointKind::Gateway)
        .map_err(model_configuration_error)?;
    let model_config = OpenAiModelConfig::try_new(&model, 1_048_576, 1_048_576, 512, 512, 64)
        .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OpenAiCompatibleProvider::try_new(config, vec![model_config])
            .map_err(model_configuration_error)?,
    );
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 64,
            batches_per_session: 256,
            records_per_session: 4_096,
            snapshot_bytes: 64 * 1_024,
        })
        .map_err(|error| configuration_error(error.to_string()))?,
    );
    let mut builder = Agent::builder(
        AgentId::parse("python.agent.openai-compatible")
            .map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse("python.bundle.openai-compatible")
            .map_err(|error| configuration_error(error.to_string()))?,
        (
            component("python.model.openai-compatible")?,
            Arc::clone(&provider),
        ),
        (component("python.store.memory")?, store),
    );
    if let Some(instruction) = instruction {
        builder = builder.try_instruction(instruction)?;
    }
    let agent = builder.build().await?;
    Ok(PyAgent {
        inner: Arc::new(agent),
        model: model_name,
    })
}

fn run_request(
    model: &ModelName,
    input: String,
    timeout_seconds: f64,
    max_cycles: u64,
) -> Result<AgentRunRequest, AgentRunError> {
    if !timeout_seconds.is_finite()
        || timeout_seconds <= 0.0
        || timeout_seconds > MAX_TIMEOUT_SECONDS
    {
        return Err(configuration_error(
            "timeout_seconds must be finite and in (0, 86400]",
        ));
    }
    let security = RunSecurityContext::try_new(
        "python-local",
        PrincipalRef::try_new("finstack-ai-python", "local-user", Some("python-local"))
            .map_err(|error| configuration_error(error.to_string()))?,
        "local",
        "python-embedded",
        "python-policy-v1",
        "python-decision-v1",
        None,
    )
    .map_err(|error| configuration_error(error.to_string()))?;
    let mut request = AgentRunRequest::try_new(model.clone(), input, security)?;
    request.timeout = Duration::from_secs_f64(timeout_seconds);
    request.max_cycles = max_cycles;
    Ok(request)
}

fn component(id: &str) -> Result<ComponentRef, AgentRunError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(|error| configuration_error(error.to_string()))?,
        Some(PREVIEW_VERSION),
    ))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used directly as a Result::map_err adapter"
)]
fn model_configuration_error(error: finstack_ai::runtime::ModelError) -> AgentRunError {
    configuration_error(format!("{}: {}", error.code(), error.message()))
}

fn configuration_error(message: impl Into<String>) -> AgentRunError {
    AgentRunError::Configuration {
        code: AGENT_RUN_INVALID_CONFIGURATION,
        message: message.into(),
    }
}

fn result_to_python_with_locator(
    py: Python<'_>,
    result: Result<AgentRunOutput, AgentRunError>,
    locator: Option<&OperationLocator>,
) -> PyResult<Py<PyRunResult>> {
    match result {
        Ok(inner) => Py::new(py, PyRunResult { inner }),
        Err(error) => Err(agent_error(py, &error, locator)),
    }
}

fn agent_error(py: Python<'_>, error: &AgentRunError, locator: Option<&OperationLocator>) -> PyErr {
    let exception = match error.code() {
        AGENT_RUN_INVALID_CONFIGURATION => py.get_type::<ConfigurationError>(),
        AGENT_RUN_TIMEOUT => py.get_type::<TimeoutError>(),
        AGENT_RUN_CANCELLED => py.get_type::<CancelledError>(),
        _ => py.get_type::<RuntimeError>(),
    };
    let value = exception.call1((error.to_string(),));
    match value {
        Ok(value) => {
            let _ = value.setattr("code", error.code());
            let _ = value.setattr("retryable", error.retryable());
            if let Some(locator) = locator {
                if let Ok(context) = locator_dict(py, locator) {
                    let _ = value.setattr("context", context);
                }
            } else {
                let _ = value.setattr("context", py.None());
            }
            PyErr::from_value(value)
        }
        Err(construction_error) => construction_error,
    }
}

fn locator_dict(py: Python<'_>, locator: &OperationLocator) -> PyResult<Py<PyDict>> {
    let context = PyDict::new(py);
    context.set_item("tenant_scope", locator.tenant_scope.as_ref())?;
    context.set_item("session_id", locator.session_id.to_string())?;
    context.set_item("lane_id", locator.lane_id.to_string())?;
    context.set_item("run_id", locator.run_id.to_string())?;
    Ok(context.unbind())
}

fn event_json_value(event: &RunEvent) -> PyResult<serde_json::Value> {
    serde_json::to_value(event).map_err(|_| PyException::new_err("event serialization failed"))
}

#[pymodule(gil_used = false)]
fn _finstack_ai(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", ENGINE_VERSION)?;
    module.add("__engine_version__", ENGINE_VERSION)?;
    module.add("FinstackError", module.py().get_type::<FinstackError>())?;
    module.add(
        "ConfigurationError",
        module.py().get_type::<ConfigurationError>(),
    )?;
    module.add("RuntimeError", module.py().get_type::<RuntimeError>())?;
    module.add("CancelledError", module.py().get_type::<CancelledError>())?;
    module.add("TimeoutError", module.py().get_type::<TimeoutError>())?;
    module.add_class::<PyAgent>()?;
    module.add_class::<PyRun>()?;
    module.add_class::<PyEventIterator>()?;
    module.add_class::<PySession>()?;
    module.add_class::<PyRunResult>()?;
    module.add_class::<PyEvent>()?;
    module.add_class::<PyEventBatch>()?;
    module.add_function(wrap_pyfunction!(health, module)?)?;
    module.add_function(wrap_pyfunction!(build_metadata, module)?)?;
    module.add_function(wrap_pyfunction!(linked_providers, module)?)?;
    #[cfg(feature = "benchmark-fixture")]
    benchmark_fixture::register(module)?;
    Ok(())
}
