use super::{
    error, json,
    source::{Maintenance, PySearchSource},
};
use finstack_ai::runtime::{
    artifact::ArtifactStore,
    ports::{context::ContextProvider, observer::Observer, tool::Toolset},
};
use finstack_ai_index_documents::{DocumentIndexToolset, DocumentSearchSource};
use finstack_ai_index_journal::{JournalIndexObserver, JournalSearchSource};
use finstack_ai_kernel::{ComponentRef, Version};
use finstack_ai_search::{SearchContextProvider, SearchEngine, SearchError, SearchToolset};
use pyo3::prelude::*;
use std::sync::Arc;

const TOOL_VERSION: Version = Version {
    major: 0,
    minor: 1,
    patch: 0,
};
/// Read-only native search toolset accepted by all agent factories.
#[pyclass(module = "finstack_ai._finstack_ai", name = "SearchToolset", frozen)]
pub(crate) struct PySearchToolset {
    inner: Arc<SearchToolset>,
}
impl PySearchToolset {
    pub(super) fn new(engine: Arc<SearchEngine>) -> PyResult<Self> {
        Ok(Self {
            inner: Arc::new(SearchToolset::try_new(engine).map_err(error)?),
        })
    }
    pub(crate) fn registration(&self) -> PyResult<(ComponentRef, Arc<dyn Toolset>)> {
        Ok((
            crate::component_ref("finstack.tools.search", TOOL_VERSION)?,
            self.inner.clone(),
        ))
    }
}
/// Global recall replaces memory recall in federated compositions.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "SearchContextProvider",
    frozen
)]
pub(crate) struct PySearchContextProvider {
    inner: Arc<SearchContextProvider>,
}
impl PySearchContextProvider {
    pub(super) fn new(engine: Arc<SearchEngine>, max_hits: usize) -> PyResult<Self> {
        Ok(Self {
            inner: Arc::new(SearchContextProvider::try_new(engine, max_hits).map_err(error)?),
        })
    }
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn ContextProvider>) {
        let invocation = self.inner.descriptor().invocation;
        (
            ComponentRef::new(invocation.component, Some(invocation.version)),
            self.inner.clone(),
        )
    }
}
/// Explicit indexing tool effect over the composing agent's artifact store.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "DocumentIndexToolset",
    frozen
)]
pub(crate) struct PyDocumentIndexToolset {
    source: Arc<DocumentSearchSource>,
    artifacts: Arc<dyn ArtifactStore>,
}
#[pymethods]
impl PyDocumentIndexToolset {
    #[new]
    fn new(source: &Bound<'_, PySearchSource>) -> PyResult<Self> {
        match &source.borrow().maintenance {
            Maintenance::Documents(source, artifacts) => Ok(Self {
                source: source.clone(),
                artifacts: artifacts.clone(),
            }),
            _ => Err(super::invalid()),
        }
    }
}
impl PyDocumentIndexToolset {
    pub(crate) fn registration(
        &self,
        artifacts: &Arc<dyn ArtifactStore>,
    ) -> PyResult<(ComponentRef, Arc<dyn Toolset>)> {
        if !Arc::ptr_eq(artifacts, &self.artifacts) {
            return Err(error(SearchError::SearchScopeDenied));
        }
        Ok((
            crate::component_ref("finstack.tools.document-index", TOOL_VERSION)?,
            Arc::new(DocumentIndexToolset::try_new(self.source.clone()).map_err(error)?),
        ))
    }
}
/// Metadata-only indexing hints; drain explicitly to read committed records.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "JournalIndexObserver",
    frozen
)]
pub(crate) struct PyJournalIndexObserver {
    inner: Arc<JournalIndexObserver>,
}
impl PyJournalIndexObserver {
    pub(super) fn new(source: Arc<JournalSearchSource>) -> PyResult<Self> {
        Ok(Self {
            inner: Arc::new(JournalIndexObserver::try_new(source).map_err(error)?),
        })
    }
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.inner.descriptor().component, self.inner.clone())
    }
}
#[pymethods]
impl PyJournalIndexObserver {
    #[getter]
    fn dropped_hints(&self) -> u64 {
        self.inner.dropped_hints()
    }
    fn drain<'py>(
        &self,
        py: Python<'py>,
        max_sessions: usize,
        max_records: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = inner
                .drain(max_sessions, max_records)
                .await
                .map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
}
