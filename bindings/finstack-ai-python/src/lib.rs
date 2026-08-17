//! `PyO3` control handles for the Rust-owned `finstack-ai` engine.

#![warn(missing_docs)]

mod agent;
#[cfg(feature = "benchmark-fixture")]
mod benchmark_fixture;
#[cfg(feature = "callback-fixture")]
mod callback_fixture;
mod callbacks;
mod capability;
mod errors;
mod events;
mod json_bridge;
mod locator;
mod protocol;
mod run;
mod session;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

use agent::PyAgent;
use callbacks::{
    PyCallbackContext, PyPythonContextProvider, PyPythonMiddleware, PyPythonModel,
    PyPythonObserver, PyPythonToolset,
};
use capability::PyCapability;
use events::{PyEvent, PyEventBatch, PyEventIterator};
use locator::PyLocator;
use protocol::{
    _normalize_pydantic_schema, build_metadata, health, journal_known_answer, linked_providers,
    normalize_prebeta_shape,
};
use run::{PyRun, PyRunResult};
use session::{PyLane, PyMemoryExternalIdentityMap, PySession};

pub(crate) const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

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
