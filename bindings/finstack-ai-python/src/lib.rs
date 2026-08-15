//! `PyO3` control handles for the Rust-owned `finstack-ai` engine.

#![warn(missing_docs)]

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use finstack_ai::runtime::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentRef, JournalStore, Model, ModelName,
    OperationLocator, RawJson, RunEvent, RunEventClass, Version,
};
use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_TIMEOUT, Agent, AgentRunError,
    AgentRunOutput, AgentRunRequest, CapabilityActivation, CapabilitySpec, ExternalIdentityKey,
    ExternalIdentityMap, InstructionSpec, InteractionResolution, Lane, MemoryExternalIdentityMap,
    PrincipalRef, RunSecurityContext, Session, SessionError,
};
use finstack_ai_provider_anthropic::{
    AnthropicConfig, AnthropicModelConfig, AnthropicProvider,
    Authentication as AnthropicAuthentication, SecretString as AnthropicSecret,
};
use finstack_ai_provider_openai_compatible::{
    EndpointKind, OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiModelConfig,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyStopAsyncIteration, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

#[cfg(feature = "benchmark-fixture")]
mod benchmark_fixture;
#[cfg(feature = "callback-fixture")]
mod callback_fixture;
mod callbacks;

use callbacks::{
    PyCallbackContext, PyPythonContextProvider, PyPythonMiddleware, PyPythonModel,
    PyPythonObserver, PyPythonToolset, normalize_pydantic_schema,
};

const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
const OPENAI_COMPATIBLE_PROVIDER: &str = "openai-compatible";
const ANTHROPIC_PROVIDER: &str = "anthropic";
const OLLAMA_PROVIDER: &str = "ollama";
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
fn linked_providers() -> (&'static str, &'static str, &'static str) {
    let _ = finstack_ai_provider_openai_compatible::ENDPOINT_QUIRKS_VERSION;
    let _ = finstack_ai_provider_anthropic::ANTHROPIC_MESSAGES_VERSION;
    (
        OPENAI_COMPATIBLE_PROVIDER,
        ANTHROPIC_PROVIDER,
        OLLAMA_PROVIDER,
    )
}

/// Compute journal known-answer hex through the one Rust engine.
#[pyfunction]
#[pyo3(text_signature = "(kind, value)")]
fn journal_known_answer(py: Python<'_>, kind: &str, value: Py<PyAny>) -> PyResult<Py<PyAny>> {
    let json = py.import("json")?;
    let encoded = json
        .getattr("dumps")?
        .call1((value,))?
        .extract::<String>()?;
    let answer = finstack_ai_protocol::journal_known_answer(kind, &encoded)
        .map_err(|error| PyTypeError::new_err(error.to_string()))?;
    let encoded = serde_json::to_string(&answer)
        .map_err(|_| PyException::new_err("journal known-answer serialization failed"))?;
    Ok(json.getattr("loads")?.call1((encoded,))?.unbind())
}

/// Normalize a pre-beta lineage or authenticated external-command shape.
#[pyfunction]
fn normalize_prebeta_shape(py: Python<'_>, kind: &str, value: Py<PyAny>) -> PyResult<Py<PyAny>> {
    let json = py.import("json")?;
    let encoded = json
        .getattr("dumps")?
        .call1((value,))?
        .extract::<String>()?;
    let normalized = match kind {
        "child_run_prepared" => normalize_shape::<finstack_ai::runtime::ChildRunPrepared>(&encoded),
        "interaction_resolution" => {
            normalize_shape::<finstack_ai::runtime::InteractionResolutionCommand>(&encoded)
        }
        "external_effect_completion" => {
            normalize_shape::<finstack_ai::runtime::ExternalEffectCompletionCommand>(&encoded)
        }
        _ => {
            return Err(PyTypeError::new_err(format!(
                "unsupported pre-beta shape: {kind}"
            )));
        }
    }?;
    Ok(json.getattr("loads")?.call1((normalized,))?.unbind())
}

/// Normalize one generated Pydantic schema into the portable binding subset.
#[pyfunction]
fn _normalize_pydantic_schema(
    py: Python<'_>,
    schema: Py<PyAny>,
    kind: &str,
) -> PyResult<Py<PyAny>> {
    let json = py.import("json")?;
    let encoded = json
        .getattr("dumps")?
        .call1((schema,))?
        .extract::<String>()?;
    let value = serde_json::from_str(&encoded)
        .map_err(|_| PyTypeError::new_err("Pydantic schema is not JSON serializable"))?;
    let normalized = normalize_pydantic_schema(value, kind).map_err(PyTypeError::new_err)?;
    let encoded = serde_json::to_string(&normalized)
        .map_err(|_| PyException::new_err("Pydantic schema normalization failed"))?;
    Ok(json.getattr("loads")?.call1((encoded,))?.unbind())
}

fn normalize_shape<T>(encoded: &str) -> PyResult<String>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let value: T = serde_json::from_str(encoded)
        .map_err(|error| PyTypeError::new_err(format!("invalid pre-beta shape: {error}")))?;
    serde_json::to_string(&value)
        .map_err(|_| PyException::new_err("pre-beta shape serialization failed"))
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

/// Data-only declarative capability accepted by both Python agent factories.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "Capability",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
struct PyCapability {
    inner: CapabilitySpec,
}

#[pymethods]
impl PyCapability {
    /// Construct one bounded declarative capability.
    #[new]
    #[pyo3(signature = (id, description, instructions, *, activation = "application"))]
    fn new(
        id: String,
        description: String,
        instructions: Vec<String>,
        activation: &str,
    ) -> PyResult<Self> {
        let activation = match activation {
            "always" => CapabilityActivation::Always,
            "application" => CapabilityActivation::Application,
            "model" => CapabilityActivation::Model,
            "disabled" => CapabilityActivation::Disabled,
            _ => {
                return Err(PyTypeError::new_err(
                    "activation must be always, application, model, or disabled",
                ));
            }
        };
        let instructions = instructions
            .into_iter()
            .map(InstructionSpec::try_new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| PyTypeError::new_err(error.to_string()))?;
        let inner = CapabilitySpec {
            id: CapabilityId::parse(id).map_err(|error| PyTypeError::new_err(error.to_string()))?,
            description: Arc::from(description),
            instructions: instructions.into(),
            toolsets: Arc::from([]),
            context_providers: Arc::from([]),
            middleware: Arc::from([]),
            activation,
        };
        inner
            .validate()
            .map_err(|error| PyTypeError::new_err(error.to_string()))?;
        Ok(Self { inner })
    }

    #[getter]
    fn id(&self) -> &str {
        self.inner.id.as_str()
    }

    #[getter]
    fn description(&self) -> &str {
        &self.inner.description
    }

    #[getter]
    fn activation(&self) -> &'static str {
        match self.inner.activation {
            CapabilityActivation::Always => "always",
            CapabilityActivation::Application => "application",
            CapabilityActivation::Model => "model",
            CapabilityActivation::Disabled => "disabled",
        }
    }
}

/// Rust-owned resolved agent handle.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Agent", frozen)]
struct PyAgent {
    inner: Arc<Agent>,
    model: ModelName,
    output_adapter: Option<Py<PyAny>>,
}

#[pymethods]
impl PyAgent {
    /// Construct a keyless Rust-backed OpenAI-compatible agent.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None))]
    fn openai_compatible(
        py: Python<'_>,
        base_url: String,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_openai_compatible_agent(
                base_url,
                model,
                instruction,
                capabilities,
                active_capabilities,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Construct a Rust-backed Anthropic Messages agent.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None))]
    fn anthropic(
        py: Python<'_>,
        base_url: String,
        model: String,
        api_key: Option<String>,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_anthropic_agent(
                base_url,
                model,
                api_key,
                instruction,
                capabilities,
                active_capabilities,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Construct a keyless Rust-backed Ollama/local agent.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None))]
    fn ollama(
        py: Python<'_>,
        base_url: String,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_ollama_agent(
                base_url,
                model,
                instruction,
                capabilities,
                active_capabilities,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Construct an agent from trusted coarse Python model and Toolset callbacks.
    #[staticmethod]
    #[pyo3(signature = (model, toolsets = None, instruction = None, output_type = None, capabilities = None, active_capabilities = None))]
    fn from_python<'py>(
        py: Python<'py>,
        model: &Bound<'py, PyPythonModel>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        instruction: Option<String>,
        output_type: Option<Py<PyAny>>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = model.borrow();
        let model_name = model.model_name();
        let model = model.registration();
        let toolsets = toolsets
            .unwrap_or_default()
            .into_iter()
            .map(|toolset| toolset.bind(py).borrow().registration())
            .collect::<Vec<_>>();
        let output = output_type
            .map(|target| prepare_pydantic_output(py, target))
            .transpose()?;
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_python_agent(
                model_name,
                model,
                toolsets,
                instruction,
                output,
                capabilities,
                active_capabilities,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Return the bounded model-activated catalog in stable identity order.
    fn capability_catalog(&self, py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
        self.inner
            .capability_catalog()
            .into_iter()
            .map(|entry| {
                let value = PyDict::new(py);
                value.set_item("id", entry.id().as_str())?;
                value.set_item("description", entry.description())?;
                Ok(value.unbind())
            })
            .collect()
    }

    /// Render the compact model-facing catalog without activating a capability.
    fn compact_capability_catalog(&self) -> String {
        self.inner.compact_capability_catalog()
    }

    /// Create a live session on this agent's journal store.
    #[pyo3(signature = (tenant_scope = "default"))]
    fn create_session<'py>(
        &self,
        py: Python<'py>,
        tenant_scope: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let store = self.inner.journal_store();
        let tenant_scope = tenant_scope.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match Session::create(store, tenant_scope).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PySession { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Open an existing session without respawning parked runs.
    fn open_session<'py>(
        &self,
        py: Python<'py>,
        session_id: String,
        tenant_scope: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let store = self.inner.journal_store();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let session_id = finstack_ai::runtime::SessionId::parse(&session_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match Session::open(store, session_id, tenant_scope).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PySession { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Start a run and return its shared control handle immediately.
    #[pyo3(signature = (input, *, timeout_seconds = DEFAULT_TIMEOUT_SECONDS, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None))]
    fn start(
        &self,
        py: Python<'_>,
        input: String,
        timeout_seconds: f64,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
    ) -> PyResult<PyRun> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        py.detach(move || {
            let request = run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
            )?;
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            agent.start(request).map(|inner| PyRun {
                inner,
                output_adapter,
            })
        })
        .map_err(|error| agent_error(py, &error, None))
    }

    /// Execute one run and await its committed result.
    #[pyo3(signature = (input, *, timeout_seconds = DEFAULT_TIMEOUT_SECONDS, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        input: String,
        timeout_seconds: f64,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let request = match run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
            ) {
                Ok(request) => request,
                Err(error) => return Python::attach(|py| Err(agent_error(py, &error, None))),
            };
            let run = match agent.start(request) {
                Ok(run) => run,
                Err(error) => return Python::attach(|py| Err(agent_error(py, &error, None))),
            };
            let locator = run.locator().clone();
            let result = run.result().await;
            Python::attach(|py| {
                result_to_python_with_locator(py, result, Some(&locator), output_adapter)
            })
        })
    }
}

/// Shared control handle for one Rust-owned run.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Run", frozen)]
struct PyRun {
    inner: finstack_ai::AgentRun,
    output_adapter: Option<Py<PyAny>>,
}

#[pymethods]
impl PyRun {
    /// Live session handle for this run.
    #[getter]
    fn session(&self) -> PySession {
        PySession {
            inner: self.inner.session(),
        }
    }

    /// Immutable operation locator snapshot.
    #[getter]
    fn locator(&self) -> PyLocator {
        PyLocator {
            locator: Arc::new(self.inner.locator().clone()),
        }
    }

    /// Wait for the retained terminal result.
    fn result<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            let result = run.result().await;
            Python::attach(|py| {
                result_to_python_with_locator(py, result, Some(&locator), output_adapter)
            })
        })
    }

    /// List the outstanding typed interaction for this run (0 or 1).
    #[pyo3(text_signature = "($self)")]
    fn list_interactions<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.list_interactions().await {
                Ok(requests) => Python::attach(|py| {
                    let encoded = serde_json::to_string(&requests).map_err(|_| {
                        PyException::new_err("interaction list serialization failed")
                    })?;
                    py.import("json")?
                        .getattr("loads")?
                        .call1((encoded,))
                        .map(pyo3::Bound::unbind)
                }),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, Some(&locator)))),
            }
        })
    }

    /// Resolve the outstanding interaction through the live Rust-owned run.
    #[pyo3(text_signature = "($self, resolution)")]
    fn resolve_interaction<'py>(
        &self,
        py: Python<'py>,
        resolution: Py<PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let encoded = py
            .import("json")?
            .getattr("dumps")?
            .call1((resolution,))?
            .extract::<String>()?;
        let resolution = serde_json::from_str::<InteractionResolution>(&encoded)
            .map_err(|error| PyTypeError::new_err(error.to_string()))?;
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.resolve_interaction(resolution).await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, Some(&locator)))),
            }
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
#[pyclass(module = "finstack_ai._finstack_ai", name = "Locator", frozen)]
struct PyLocator {
    locator: Arc<OperationLocator>,
}

#[pymethods]
impl PyLocator {
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

/// Live session handle over one journaled session.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Session", frozen)]
struct PySession {
    inner: Session,
}

#[pymethods]
impl PySession {
    #[getter]
    fn tenant_scope(&self) -> &str {
        self.inner.tenant_scope()
    }

    #[getter]
    fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Create a named lane, optionally forking from an existing entry.
    #[pyo3(signature = (name, fork = None))]
    fn create_lane<'py>(
        &self,
        py: Python<'py>,
        name: String,
        fork: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let fork = fork
                .map(|value| finstack_ai::runtime::EntryId::parse(&value))
                .transpose()
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match session.create_lane(name, fork).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PyLane { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// List restored lanes.
    fn list_lanes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match session.list_lanes().await {
                Ok(lanes) => Python::attach(|py| {
                    lanes
                        .into_iter()
                        .map(|inner| Py::new(py, PyLane { inner }))
                        .collect::<PyResult<Vec<_>>>()
                }),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Look up one lane by application name.
    fn lane<'py>(&self, py: Python<'py>, name: String) -> PyResult<Bound<'py, PyAny>> {
        let session = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match session.lane(&name).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PyLane { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Bind a host-owned external identity to one lane.
    fn bind_external_identity(
        &self,
        map: &Bound<'_, PyMemoryExternalIdentityMap>,
        channel: String,
        account: String,
        thread: String,
        lane_id: &str,
    ) -> PyResult<()> {
        let key = ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        let lane_id = finstack_ai::runtime::LaneId::parse(lane_id)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        self.inner
            .bind_external_identity(&map.borrow().inner, key, lane_id)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))
    }

    /// Resolve a host-owned external identity key.
    #[staticmethod]
    fn resolve_external_identity(
        map: &Bound<'_, PyMemoryExternalIdentityMap>,
        channel: String,
        account: String,
        thread: String,
    ) -> PyResult<Option<(String, String)>> {
        let key = ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        Ok(
            Session::resolve_external_identity(&map.borrow().inner, &key)
                .map(|(session_id, lane_id)| (session_id.to_string(), lane_id.to_string())),
        )
    }
}

/// Live lane handle.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Lane", frozen)]
struct PyLane {
    inner: Lane,
}

#[pymethods]
impl PyLane {
    #[getter]
    fn lane_id(&self) -> String {
        self.inner.lane_id().to_string()
    }

    #[getter]
    fn session(&self) -> PySession {
        PySession {
            inner: self.inner.session().clone(),
        }
    }

    /// Point this idle lane at an existing entry without copying.
    fn navigate<'py>(&self, py: Python<'py>, entry_id: String) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let entry_id = finstack_ai::runtime::EntryId::parse(&entry_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match lane.navigate(entry_id).await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Inspect name, leaf, active run, and history length.
    fn inspect<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let lane = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match lane.inspect().await {
                Ok(inspect) => Python::attach(|py| {
                    let value = PyDict::new(py);
                    value.set_item("lane_id", inspect.lane_id.to_string())?;
                    value.set_item("name", inspect.name.as_ref())?;
                    value.set_item("leaf_id", inspect.leaf_id.map(|id| id.to_string()))?;
                    value.set_item(
                        "active_run_id",
                        inspect.active_run_id.map(|id| id.to_string()),
                    )?;
                    value.set_item("history_len", inspect.history.len())?;
                    Ok(value.unbind())
                }),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }
}

/// In-process external identity map.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "MemoryExternalIdentityMap",
    frozen
)]
struct PyMemoryExternalIdentityMap {
    inner: MemoryExternalIdentityMap,
}

#[pymethods]
impl PyMemoryExternalIdentityMap {
    #[new]
    fn new() -> Self {
        Self {
            inner: MemoryExternalIdentityMap::new(),
        }
    }

    /// Resolve one previously bound key.
    fn resolve(
        &self,
        channel: String,
        account: String,
        thread: String,
    ) -> PyResult<Option<(String, String)>> {
        let key = ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
        Ok(self
            .inner
            .resolve(&key)
            .map(|(session_id, lane_id)| (session_id.to_string(), lane_id.to_string())))
    }
}

/// Immutable successful terminal result snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "RunResult", frozen)]
struct PyRunResult {
    inner: AgentRunOutput,
    output: Option<Py<PyAny>>,
}

struct PreparedPydanticOutput {
    adapter: Py<PyAny>,
    schema: RawJson,
}

fn prepare_pydantic_output(py: Python<'_>, target: Py<PyAny>) -> PyResult<PreparedPydanticOutput> {
    let pydantic = py.import("pydantic").map_err(|_| {
        PyTypeError::new_err(
            "output_type requires the optional Pydantic extra: install finstack-ai[pydantic]",
        )
    })?;
    let adapter_type = pydantic.getattr("TypeAdapter")?;
    let adapter = if target.bind(py).is_instance(&adapter_type)? {
        target
    } else {
        adapter_type.call1((target,))?.unbind()
    };
    let kwargs = PyDict::new(py);
    kwargs.set_item("mode", "validation")?;
    let schema = adapter
        .bind(py)
        .call_method("json_schema", (), Some(&kwargs))?
        .unbind();
    let schema = raw_pydantic_schema(py, schema, "structured_output")?;
    Ok(PreparedPydanticOutput { adapter, schema })
}

fn raw_pydantic_schema(py: Python<'_>, schema: Py<PyAny>, kind: &str) -> PyResult<RawJson> {
    let encoded = py
        .import("json")?
        .getattr("dumps")?
        .call1((schema,))?
        .extract::<String>()?;
    let value = serde_json::from_str(&encoded)
        .map_err(|_| PyTypeError::new_err("Pydantic schema is not JSON serializable"))?;
    let normalized = normalize_pydantic_schema(value, kind).map_err(PyTypeError::new_err)?;
    let bytes = serde_json::to_vec(&normalized)
        .map_err(|_| PyException::new_err("Pydantic schema normalization failed"))?;
    RawJson::parse(bytes).map_err(|_| PyException::new_err("Pydantic schema is invalid JSON"))
}

#[pymethods]
impl PyRunResult {
    #[getter]
    fn text(&self) -> String {
        self.inner.text()
    }

    /// Typed structured output, when this run configured `output_type`.
    #[getter]
    fn output(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.output.as_ref().map(|output| output.clone_ref(py))
    }

    /// Durable retry attempts consumed by this run.
    #[getter]
    fn retry_attempts(&self) -> u32 {
        self.inner.retry_attempts()
    }

    /// Complete Rust-owned capability activation set for this run.
    #[getter]
    fn active_capabilities(&self, py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
        self.inner
            .active_capabilities()
            .iter()
            .map(|active| {
                let value = PyDict::new(py);
                value.set_item("id", active.capability_id.as_str())?;
                value.set_item(
                    "source",
                    match active.source {
                        finstack_ai::CapabilityActivationSource::Always => "always",
                        finstack_ai::CapabilityActivationSource::Application => "application",
                        finstack_ai::CapabilityActivationSource::Model => "model",
                    },
                )?;
                Ok(value.unbind())
            })
            .collect()
    }

    /// Stable Rust-owned committed record-kind trace in journal order.
    #[getter]
    fn trace(&self) -> Vec<&str> {
        self.inner
            .record_kinds()
            .iter()
            .map(AsRef::as_ref)
            .collect()
    }

    #[getter]
    fn locator(&self) -> PyLocator {
        PyLocator {
            locator: Arc::new(self.inner.locator.clone()),
        }
    }

    #[getter]
    fn session(&self) -> PyLocator {
        PyLocator {
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
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
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
    for capability in capabilities {
        builder = builder.capability(capability);
    }
    for capability in active_capabilities {
        builder = builder.activate_application(capability);
    }
    let agent = builder.build().await?;
    Ok(PyAgent {
        inner: Arc::new(agent),
        model: model_name,
        output_adapter: None,
    })
}

async fn build_anthropic_agent(
    base_url: String,
    model: String,
    api_key: Option<String>,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<PyAgent, AgentRunError> {
    let mut config = AnthropicConfig::try_new(base_url).map_err(model_configuration_error)?;
    if let Some(api_key) = api_key {
        config = config.with_authentication(AnthropicAuthentication::ApiKey(
            AnthropicSecret::try_new(api_key).map_err(model_configuration_error)?,
        ));
    }
    let model_config = AnthropicModelConfig::try_new(&model, 1_048_576, 1_048_576, 512, 512, 64)
        .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        AnthropicProvider::try_new(config, vec![model_config])
            .map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.anthropic",
        bundle_id: "python.bundle.anthropic",
        model_id: "python.model.anthropic",
        model_name,
        provider,
        instruction,
        capabilities,
        active_capabilities,
    })
    .await
}

async fn build_ollama_agent(
    base_url: String,
    model: String,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<PyAgent, AgentRunError> {
    let config =
        OpenAiCompatibleConfig::ollama_local(base_url).map_err(model_configuration_error)?;
    let model_config = OpenAiModelConfig::try_new(&model, 1_048_576, 1_048_576, 512, 512, 64)
        .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OpenAiCompatibleProvider::try_new(config, vec![model_config])
            .map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.ollama",
        bundle_id: "python.bundle.ollama",
        model_id: "python.model.ollama",
        model_name,
        provider,
        instruction,
        capabilities,
        active_capabilities,
    })
    .await
}

struct LinkedAgentSpec {
    agent_id: &'static str,
    bundle_id: &'static str,
    model_id: &'static str,
    model_name: ModelName,
    provider: Arc<dyn Model>,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
}

async fn finish_linked_agent(spec: LinkedAgentSpec) -> Result<PyAgent, AgentRunError> {
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
        AgentId::parse(spec.agent_id).map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse(spec.bundle_id).map_err(|error| configuration_error(error.to_string()))?,
        (component(spec.model_id)?, spec.provider),
        (component("python.store.memory")?, store),
    );
    if let Some(instruction) = spec.instruction {
        builder = builder.try_instruction(instruction)?;
    }
    for capability in spec.capabilities {
        builder = builder.capability(capability);
    }
    for capability in spec.active_capabilities {
        builder = builder.activate_application(capability);
    }
    let agent = builder.build().await?;
    Ok(PyAgent {
        inner: Arc::new(agent),
        model: spec.model_name,
        output_adapter: None,
    })
}

async fn build_python_agent(
    model_name: ModelName,
    model: (ComponentRef, Arc<dyn Model>),
    toolsets: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Toolset>)>,
    instruction: Option<String>,
    output: Option<PreparedPydanticOutput>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<PyAgent, AgentRunError> {
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
        AgentId::parse("python.agent.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse("python.bundle.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        model,
        (component("python.store.memory")?, store),
    );
    for (component, toolset) in toolsets {
        builder = builder.toolset(component, toolset);
    }
    if let Some(instruction) = instruction {
        builder = builder.try_instruction(instruction)?;
    }
    for capability in capabilities {
        builder = builder.capability(capability);
    }
    for capability in active_capabilities {
        builder = builder.activate_application(capability);
    }
    let agent = builder.build().await?;
    let (agent, output_adapter) = if let Some(output) = output {
        (
            agent.try_with_output_schema(&output.schema)?,
            Some(output.adapter),
        )
    } else {
        (agent, None)
    };
    Ok(PyAgent {
        inner: Arc::new(agent),
        model: model_name,
        output_adapter,
    })
}

fn capability_configuration(
    py: Python<'_>,
    capabilities: Option<Vec<Py<PyCapability>>>,
    active_capabilities: Option<Vec<String>>,
) -> PyResult<(Vec<CapabilitySpec>, Vec<CapabilityId>)> {
    let capabilities = capabilities
        .unwrap_or_default()
        .into_iter()
        .map(|capability| capability.bind(py).borrow().inner.clone())
        .collect();
    let active_capabilities = active_capabilities
        .unwrap_or_default()
        .into_iter()
        .map(CapabilityId::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| PyTypeError::new_err(error.to_string()))?;
    Ok((capabilities, active_capabilities))
}

fn run_request(
    model: &ModelName,
    input: String,
    timeout_seconds: f64,
    max_cycles: u64,
    max_output_retries: u32,
    capability: Option<String>,
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
    request.max_output_retries = max_output_retries;
    if let Some(capability) = capability {
        request.capability = Some(
            CapabilityId::parse(&capability)
                .map_err(|error| configuration_error(error.to_string()))?,
        );
    }
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
    output_adapter: Option<Py<PyAny>>,
) -> PyResult<Py<PyRunResult>> {
    match result {
        Ok(inner) => {
            let output = output_adapter
                .map(|adapter| {
                    let raw = inner.structured_json().ok_or_else(|| {
                        PyException::new_err("structured result is missing canonical JSON")
                    })?;
                    adapter.call_method1(py, "validate_json", (PyBytes::new(py, raw.as_bytes()),))
                })
                .transpose()?;
            Py::new(py, PyRunResult { inner, output })
        }
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

fn session_py_error(py: Python<'_>, error: &SessionError) -> PyErr {
    let value = py
        .get_type::<ConfigurationError>()
        .call1((error.to_string(),));
    match value {
        Ok(value) => {
            let _ = value.setattr("code", error.code());
            let _ = value.setattr("retryable", false);
            let _ = value.setattr("context", py.None());
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
    module.add_class::<PyCapability>()?;
    module.add_class::<PyRun>()?;
    module.add_class::<PyEventIterator>()?;
    module.add_class::<PyLocator>()?;
    module.add_class::<PySession>()?;
    module.add_class::<PyLane>()?;
    module.add_class::<PyMemoryExternalIdentityMap>()?;
    module.add_class::<PyRunResult>()?;
    module.add_class::<PyEvent>()?;
    module.add_class::<PyEventBatch>()?;
    module.add_class::<PyCallbackContext>()?;
    module.add_class::<PyPythonModel>()?;
    module.add_class::<PyPythonToolset>()?;
    module.add_class::<PyPythonContextProvider>()?;
    module.add_class::<PyPythonMiddleware>()?;
    module.add_class::<PyPythonObserver>()?;
    module.add_function(wrap_pyfunction!(health, module)?)?;
    module.add_function(wrap_pyfunction!(build_metadata, module)?)?;
    module.add_function(wrap_pyfunction!(linked_providers, module)?)?;
    module.add_function(wrap_pyfunction!(journal_known_answer, module)?)?;
    module.add_function(wrap_pyfunction!(normalize_prebeta_shape, module)?)?;
    module.add_function(wrap_pyfunction!(_normalize_pydantic_schema, module)?)?;
    #[cfg(feature = "benchmark-fixture")]
    benchmark_fixture::register(module)?;
    #[cfg(feature = "callback-fixture")]
    callback_fixture::register(module)?;
    Ok(())
}
