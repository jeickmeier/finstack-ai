//! `PyO3` control handles for the Rust-owned `finstack-ai` engine.

#![warn(missing_docs)]
// PyO3 generates FFI glue that requires `unsafe`.
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

mod agent;
mod approval_grant;
#[cfg(feature = "benchmark-fixture")]
mod benchmark_fixture;
#[cfg(feature = "callback-fixture")]
mod callback_fixture;
mod callbacks;
mod capability;
mod child_policy;
mod document;
mod e2b;
mod elicitation;
mod errors;
mod events;
mod fetch;
mod json_bridge;
mod locator;
mod memory;
mod middleware;
mod observers;
mod protocol;
mod repository;
mod run;
mod session;
mod skills;
mod store;
mod toolsets;

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

use agent::{PyAgent, PyHistoryCachePolicy};
use approval_grant::PyApprovalGrantMode;
use callbacks::{
    PyCallbackContext, PyPythonContextProvider, PyPythonMiddleware, PyPythonModel,
    PyPythonObserver, PyPythonToolset,
};
use capability::PyCapability;
use child_policy::PyChildRunPolicy;
use document::{parse_document, parse_document_markdown};
use events::{PyEvent, PyEventBatch, PyEventIterator};
use locator::PyLocator;
use memory::{PyMemoryContextProvider, PyMemoryExtension, PyMemoryObserver, PyMemoryToolset};
use middleware::{
    PyCompactionMiddleware, PyInstructionsMiddleware, PyRedactionMiddleware,
    PyToolPolicyMiddleware, PyVerifyMiddleware,
};
use observers::{
    PyBillingObserver, PyLogObserver, PyMetricsObserver, PyNotifyObserver, PyOtelObserver,
};
use protocol::{
    _normalize_pydantic_schema, build_metadata, health, journal_known_answer, linked_providers,
    normalize_prebeta_shape,
};
use repository::PyRepositoryContextProvider;
use run::{PyAttachment, PyRun, PyRunResult};
use session::{PyLane, PyMemoryExternalIdentityMap, PySession};
use skills::PySkillsToolset;
use store::PySqliteDurability;
use toolsets::{PyCalculatorToolset, PyFileSystemToolset, PyShellToolset};

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
    module.add_class::<PyHistoryCachePolicy>()?;
    module.add_class::<PyCapability>()?;
    module.add_class::<PyChildRunPolicy>()?;
    module.add_class::<PyApprovalGrantMode>()?;
    module.add_class::<PyRun>()?;
    module.add_class::<PyAttachment>()?;
    module.add_class::<PyEventIterator>()?;
    module.add_class::<PyLocator>()?;
    module.add_class::<PySession>()?;
    module.add_class::<PyLane>()?;
    module.add_class::<PyMemoryExternalIdentityMap>()?;
    module.add_class::<PySqliteDurability>()?;
    module.add_class::<PyRunResult>()?;
    module.add_class::<PyEvent>()?;
    module.add_class::<PyEventBatch>()?;
    module.add_class::<PyCallbackContext>()?;
    module.add_class::<PyPythonModel>()?;
    module.add_class::<PyPythonToolset>()?;
    module.add_class::<elicitation::PyElicitationToolset>()?;
    module.add_class::<PyMemoryExtension>()?;
    module.add_class::<PyMemoryContextProvider>()?;
    module.add_class::<PyMemoryToolset>()?;
    module.add_class::<PyMemoryObserver>()?;
    module.add_class::<fetch::PyHttpFetchToolset>()?;
    module.add_class::<e2b::PyE2bSandboxToolset>()?;
    module.add_class::<PyPythonContextProvider>()?;
    module.add_class::<PyPythonMiddleware>()?;
    module.add_class::<PyInstructionsMiddleware>()?;
    module.add_class::<PyCompactionMiddleware>()?;
    module.add_class::<PyVerifyMiddleware>()?;
    module.add_class::<PyRedactionMiddleware>()?;
    module.add_class::<PyToolPolicyMiddleware>()?;
    module.add_class::<PyRepositoryContextProvider>()?;
    module.add_class::<PyLogObserver>()?;
    module.add_class::<PyMetricsObserver>()?;
    module.add_class::<PyOtelObserver>()?;
    module.add_class::<PyBillingObserver>()?;
    module.add_class::<PyNotifyObserver>()?;
    module.add_class::<PySkillsToolset>()?;
    module.add_class::<PyCalculatorToolset>()?;
    module.add_class::<PyFileSystemToolset>()?;
    module.add_class::<PyShellToolset>()?;
    module.add_class::<PyPythonObserver>()?;
    module.add_function(wrap_pyfunction!(health, module)?)?;
    module.add_function(wrap_pyfunction!(build_metadata, module)?)?;
    module.add_function(wrap_pyfunction!(linked_providers, module)?)?;
    module.add_function(wrap_pyfunction!(journal_known_answer, module)?)?;
    module.add_function(wrap_pyfunction!(normalize_prebeta_shape, module)?)?;
    module.add_function(wrap_pyfunction!(_normalize_pydantic_schema, module)?)?;
    module.add_function(wrap_pyfunction!(parse_document_markdown, module)?)?;
    module.add_function(wrap_pyfunction!(parse_document, module)?)?;
    module.add_function(wrap_pyfunction!(skills::import_skill_markdown, module)?)?;
    #[cfg(feature = "benchmark-fixture")]
    benchmark_fixture::register(module)?;
    #[cfg(feature = "callback-fixture")]
    callback_fixture::register(module)?;
    Ok(())
}
