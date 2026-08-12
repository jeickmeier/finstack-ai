//! Minimal `PyO3` package surface for `finstack-ai`.
//!
//! PR-027 establishes side-effect-free import, release metadata, and the
//! single-extension-module package. Agent and callback handles land in later
//! Phase 4 logical PRs.

#![warn(missing_docs)]

use pyo3::prelude::*;
use pyo3::types::PyDict;

const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
const OPENAI_COMPATIBLE_PROVIDER: &str = "openai-compatible";

#[pyfunction]
#[pyo3(text_signature = "()")]
fn health() -> &'static str {
    "ok"
}

#[pyfunction]
#[pyo3(text_signature = "()")]
fn linked_providers() -> (&'static str,) {
    // Referencing provider-owned metadata keeps the current curated provider
    // in the extension composition without constructing a client at import.
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

#[pymodule(gil_used = false)]
fn _finstack_ai(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", ENGINE_VERSION)?;
    module.add("__engine_version__", ENGINE_VERSION)?;
    module.add_function(wrap_pyfunction!(health, module)?)?;
    module.add_function(wrap_pyfunction!(build_metadata, module)?)?;
    module.add_function(wrap_pyfunction!(linked_providers, module)?)?;
    Ok(())
}
