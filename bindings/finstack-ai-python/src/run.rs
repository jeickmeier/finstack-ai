//! Shared run handle, terminal result snapshot, and run-request construction.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai::runtime::{CapabilityId, ModelName, OperationLocator, RawJson};
use finstack_ai::{
    AgentRunError, AgentRunOutput, AgentRunRequest, PrincipalRef, RunSecurityContext,
};
use pyo3::exceptions::{PyException, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::callbacks::normalize_pydantic_schema;
use crate::errors::{agent_error, configuration_error};
use crate::events::PyEventIterator;
use crate::json_bridge::{json_to_py, py_to_json};
use crate::locator::{PyLocator, locator_dict};
use crate::session::PySession;

pub(crate) const MAX_TIMEOUT_SECONDS: f64 = 86_400.0;

/// Shared control handle for one Rust-owned run.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Run", frozen)]
pub(crate) struct PyRun {
    pub(crate) inner: finstack_ai::AgentRun,
    pub(crate) output_adapter: Option<Py<PyAny>>,
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
                    let value = serde_json::to_value(&requests).map_err(|_| {
                        PyException::new_err("interaction list serialization failed")
                    })?;
                    json_to_py(py, &value)
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
        resolution: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let resolution =
            serde_json::from_value::<finstack_ai::InteractionResolution>(py_to_json(resolution)?)
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

/// Immutable successful terminal result snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "RunResult", frozen)]
pub(crate) struct PyRunResult {
    inner: AgentRunOutput,
    output: Option<Py<PyAny>>,
}

pub(crate) struct PreparedPydanticOutput {
    pub(crate) adapter: Py<PyAny>,
    pub(crate) schema: RawJson,
}

pub(crate) fn prepare_pydantic_output(
    py: Python<'_>,
    target: Py<PyAny>,
) -> PyResult<PreparedPydanticOutput> {
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
        .call_method("json_schema", (), Some(&kwargs))?;
    let raw = raw_pydantic_schema(&schema, "structured_output")?;
    Ok(PreparedPydanticOutput {
        adapter,
        schema: raw,
    })
}

fn raw_pydantic_schema(schema: &Bound<'_, PyAny>, kind: &str) -> PyResult<RawJson> {
    let value = py_to_json(schema)?;
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

pub(crate) fn run_request(
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

pub(crate) fn result_to_python_with_locator(
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
