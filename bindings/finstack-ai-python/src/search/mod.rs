//! Typed Python facade handles; search, authority and persistence remain Rust-owned.
mod components;
mod source;

pub(crate) use components::{
    PyDocumentIndexToolset, PyJournalIndexObserver, PySearchContextProvider, PySearchToolset,
};
use finstack_ai_search::{SearchConfig, SearchEngine, SearchError, SearchRequest};
use pyo3::prelude::*;
pub(crate) use source::{PySearchSource, PyTextEmbedder};
use std::sync::Arc;

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn error(error: SearchError) -> PyErr {
    Python::attach(
        |py| match py.get_type::<crate::RuntimeError>().call1((error.code(),)) {
            Ok(value) => {
                let _ = value.setattr("code", error.code());
                let _ = value.setattr("retryable", false);
                if let Ok(context) = json(py, &error) {
                    let _ = value.setattr("context", context);
                }
                PyErr::from_value(value)
            }
            Err(error) => error,
        },
    )
}
fn invalid() -> PyErr {
    error(SearchError::invalid("python_search_configuration"))
}
fn parse<T: serde::de::DeserializeOwned>(text: &str) -> PyResult<T> {
    if text.len() > 1_048_576 {
        return Err(invalid());
    }
    serde_json::from_str(text).map_err(|_| invalid())
}
fn json(py: Python<'_>, value: &impl serde::Serialize) -> PyResult<Py<PyAny>> {
    crate::json_bridge::json_to_py(py, &serde_json::to_value(value).map_err(|_| invalid())?)
}
/// Native search federation behind the typed public wrapper.
#[pyclass(module = "finstack_ai._finstack_ai", name = "_SearchEngine", frozen)]
pub(crate) struct PySearchEngine {
    inner: Arc<SearchEngine>,
}
#[pymethods]
impl PySearchEngine {
    #[new]
    fn new(py: Python<'_>, config_json: &str, sources: Vec<Py<PySearchSource>>) -> PyResult<Self> {
        let config: SearchConfig = parse(config_json)?;
        let sources = source::sources(py, sources)?;
        py.detach(move || {
            Ok(Self {
                inner: Arc::new(SearchEngine::try_new(config, sources).map_err(error)?),
            })
        })
    }
    fn search<'py>(&self, py: Python<'py>, request_json: &str) -> PyResult<Bound<'py, PyAny>> {
        let request: SearchRequest = parse(request_json)?;
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = inner.search(request).await.map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn toolset(&self) -> PyResult<PySearchToolset> {
        PySearchToolset::new(self.inner.clone())
    }
    fn context_provider(&self, max_hits: usize) -> PyResult<PySearchContextProvider> {
        PySearchContextProvider::new(self.inner.clone(), max_hits)
    }
}
