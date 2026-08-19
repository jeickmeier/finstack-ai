//! Process-local health, metadata, and protocol known-answer helpers.

use pyo3::exceptions::{PyException, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::ENGINE_VERSION;
use crate::callbacks::normalize_pydantic_schema;
use crate::json_bridge::{json_to_py, py_to_json};

pub(crate) const OPENAI_PROVIDER: &str = "openai";
pub(crate) const ANTHROPIC_PROVIDER: &str = "anthropic";
pub(crate) const OLLAMA_PROVIDER: &str = "ollama";

#[pyfunction]
#[pyo3(text_signature = "()")]
pub(crate) fn health() -> &'static str {
    "ok"
}

#[pyfunction]
#[pyo3(text_signature = "()")]
pub(crate) fn linked_providers() -> (&'static str, &'static str, &'static str) {
    let _ = finstack_ai_provider_anthropic::ANTHROPIC_MESSAGES_VERSION;
    (OPENAI_PROVIDER, ANTHROPIC_PROVIDER, OLLAMA_PROVIDER)
}

/// Compute journal known-answer hex through the one Rust engine.
#[pyfunction]
#[pyo3(text_signature = "(kind, value)")]
pub(crate) fn journal_known_answer(
    py: Python<'_>,
    kind: &str,
    value: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let encoded = serde_json::to_string(&py_to_json(value)?)
        .map_err(|_| PyTypeError::new_err("value is not JSON serializable"))?;
    let answer = finstack_ai_protocol::journal_known_answer(kind, &encoded)
        .map_err(|error| PyTypeError::new_err(error.to_string()))?;
    let value = serde_json::to_value(&answer)
        .map_err(|_| PyException::new_err("journal known-answer serialization failed"))?;
    json_to_py(py, &value)
}

/// Normalize a pre-beta lineage or authenticated external-command shape.
#[pyfunction]
pub(crate) fn normalize_prebeta_shape(
    py: Python<'_>,
    kind: &str,
    value: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let encoded = serde_json::to_string(&py_to_json(value)?)
        .map_err(|_| PyTypeError::new_err("value is not JSON serializable"))?;
    let normalized = match kind {
        "child_run_prepared" => normalize_shape::<finstack_ai_kernel::ChildRunPrepared>(&encoded),
        "interaction_resolution" => {
            normalize_shape::<finstack_ai_kernel::InteractionResolutionCommand>(&encoded)
        }
        "external_effect_completion" => {
            normalize_shape::<finstack_ai_kernel::ExternalEffectCompletionCommand>(&encoded)
        }
        _ => {
            return Err(PyTypeError::new_err(format!(
                "unsupported pre-beta shape: {kind}"
            )));
        }
    }?;
    let value: serde_json::Value = serde_json::from_str(&normalized)
        .map_err(|_| PyException::new_err("pre-beta shape serialization failed"))?;
    json_to_py(py, &value)
}

/// Normalize one generated Pydantic schema into the portable binding subset.
#[pyfunction]
pub(crate) fn _normalize_pydantic_schema(
    py: Python<'_>,
    schema: &Bound<'_, PyAny>,
    kind: &str,
) -> PyResult<Py<PyAny>> {
    let value = py_to_json(schema)?;
    let normalized = normalize_pydantic_schema(value, kind).map_err(PyTypeError::new_err)?;
    json_to_py(py, &normalized)
}

pub(crate) fn normalize_encoded_shape<T>(encoded: &str) -> PyResult<String>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    normalize_shape::<T>(encoded)
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
pub(crate) fn build_metadata(py: Python<'_>) -> PyResult<Py<PyDict>> {
    let metadata = PyDict::new(py);
    metadata.set_item("version", ENGINE_VERSION)?;
    metadata.set_item("engine_version", ENGINE_VERSION)?;
    metadata.set_item("implementation", "cpython")?;
    metadata.set_item("free_threaded", cfg!(Py_GIL_DISABLED))?;
    metadata.set_item("provider_quirks_version", 2)?;
    Ok(metadata.unbind())
}
