use super::{error, invalid, json, parse};
use crate::{agent::PyAgent, memory::PyMemoryExtension};
use finstack_ai::runtime::artifact::ArtifactStore;
use finstack_ai_embedder_ollama::{config::OllamaEmbedderConfig, embedder::OllamaEmbedder};
use finstack_ai_embeddings::embedder::{HashEmbedder, TextEmbedder};
use finstack_ai_index_documents::{DocumentIndexConfig, DocumentInput, DocumentSearchSource};
use finstack_ai_index_graph::{GraphIndexConfig, GraphSearchSource};
use finstack_ai_index_journal::{JournalIndexConfig, JournalSearchSource};
use finstack_ai_kernel::SessionId;
use finstack_ai_memory::search::MemorySearchSource;
use finstack_ai_search_core::{
    ScopeMapping, SearchError, SearchLimits, SearchQuery, SearchScope, SearchSource, SourceRef,
};
use pyo3::prelude::*;
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

/// Explicit embedder; construction never sends content.
#[pyclass(module = "finstack_ai._finstack_ai", name = "_TextEmbedder", frozen)]
pub(crate) struct PyTextEmbedder {
    inner: Arc<dyn TextEmbedder>,
}
#[pymethods]
impl PyTextEmbedder {
    #[staticmethod]
    fn hash(dimensions: usize) -> PyResult<Self> {
        Ok(Self {
            inner: Arc::new(HashEmbedder::try_new(dimensions).map_err(|_| invalid())?),
        })
    }
    #[staticmethod]
    fn ollama(
        py: Python<'_>,
        base_url: String,
        model: String,
        dimensions: usize,
    ) -> PyResult<Self> {
        py.detach(move || {
            let config = OllamaEmbedderConfig::try_new(&base_url, &model, dimensions)
                .map_err(|_| invalid())?;
            Ok(Self {
                inner: Arc::new(OllamaEmbedder::try_new(config).map_err(|_| invalid())?),
            })
        })
    }
    #[getter]
    fn space(&self) -> String {
        self.inner.descriptor().embedder_id.to_string()
    }
}
fn embedder(py: Python<'_>, value: Option<Py<PyTextEmbedder>>) -> Option<Arc<dyn TextEmbedder>> {
    value.map(|v| v.borrow(py).inner.clone())
}

pub(super) enum Maintenance {
    Memory(Arc<MemorySearchSource>),
    Documents(Arc<DocumentSearchSource>, Arc<dyn ArtifactStore>),
    Journal(Arc<JournalSearchSource>),
    Graph(Arc<GraphSearchSource>),
}
/// Native source handle retaining its exact bound authority.
#[pyclass(module = "finstack_ai._finstack_ai", name = "_SearchSource", frozen)]
pub(crate) struct PySearchSource {
    pub(super) inner: Arc<dyn SearchSource>,
    scope: SearchScope,
    pub(super) maintenance: Maintenance,
}
pub(super) fn sources(
    py: Python<'_>,
    values: Vec<Py<PySearchSource>>,
) -> PyResult<Vec<Arc<dyn SearchSource>>> {
    if values.len() > 32 {
        return Err(invalid());
    }
    Ok(values
        .into_iter()
        .map(|s| s.borrow(py).inner.clone())
        .collect())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryConfig {
    source_id: String,
    scope: SearchScope,
    #[serde(default)]
    scope_mapping: ScopeMapping,
    #[serde(default)]
    limits: SearchLimits,
}
#[pymethods]
impl PySearchSource {
    #[staticmethod]
    #[pyo3(signature=(memory, config_json, embedder_handle=None))]
    fn memory(
        py: Python<'_>,
        memory: &Bound<'_, PyMemoryExtension>,
        config_json: &str,
        embedder_handle: Option<Py<PyTextEmbedder>>,
    ) -> PyResult<Self> {
        let config: MemoryConfig = parse(config_json)?;
        let store = memory.borrow().search_store(&config.scope)?;
        let embedder = embedder(py, embedder_handle);
        py.detach(move || {
            let inner = Arc::new(
                MemorySearchSource::try_new(
                    &config.source_id,
                    store,
                    config.scope.clone(),
                    config.scope_mapping,
                    config.limits,
                    embedder,
                )
                .map_err(error)?,
            );
            Ok(Self {
                inner: inner.clone(),
                scope: config.scope,
                maintenance: Maintenance::Memory(inner),
            })
        })
    }
    #[staticmethod]
    #[pyo3(signature=(agent, path, config_json, embedder_handle=None))]
    fn documents(
        py: Python<'_>,
        agent: &Bound<'_, PyAgent>,
        path: PathBuf,
        config_json: &str,
        embedder_handle: Option<Py<PyTextEmbedder>>,
    ) -> PyResult<Self> {
        let config: DocumentIndexConfig = parse(config_json)?;
        let artifacts = agent.borrow().artifact_store.clone();
        let embedder = embedder(py, embedder_handle);
        py.detach(move || {
            let scope = config.scope.clone();
            let inner = Arc::new(
                DocumentSearchSource::try_open(&path, artifacts.clone(), config, embedder)
                    .map_err(error)?,
            );
            Ok(Self {
                inner: inner.clone(),
                scope,
                maintenance: Maintenance::Documents(inner, artifacts),
            })
        })
    }
    #[staticmethod]
    fn journal(
        py: Python<'_>,
        agent: &Bound<'_, PyAgent>,
        path: PathBuf,
        config_json: &str,
    ) -> PyResult<Self> {
        let config: JournalIndexConfig = parse(config_json)?;
        let store = agent.borrow().inner.journal_store().clone();
        py.detach(move || {
            let scope = config.scope.clone();
            let inner =
                Arc::new(JournalSearchSource::try_open(&path, store, config).map_err(error)?);
            Ok(Self {
                inner: inner.clone(),
                scope,
                maintenance: Maintenance::Journal(inner),
            })
        })
    }
    #[staticmethod]
    fn graph(
        py: Python<'_>,
        path: PathBuf,
        config_json: &str,
        source_handles: Vec<Py<Self>>,
    ) -> PyResult<Self> {
        let config: GraphIndexConfig = parse(config_json)?;
        let sources = sources(py, source_handles)?;
        py.detach(move || {
            let scope = config.scope.clone();
            let inner =
                Arc::new(GraphSearchSource::try_open(&path, config, sources).map_err(error)?);
            Ok(Self {
                inner: inner.clone(),
                scope,
                maintenance: Maintenance::Graph(inner),
            })
        })
    }
    fn references<'py>(
        &self,
        py: Python<'py>,
        cursor: Option<String>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let source = self.inner.clone();
        let scope = self.scope.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source
                .references(scope, cursor, limit)
                .await
                .map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn descriptor(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json(py, &self.inner.descriptor())
    }
    fn search<'py>(
        &self,
        py: Python<'py>,
        query_json: &str,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let query: SearchQuery = parse(query_json)?;
        let source = self.inner.clone();
        let scope = self.scope.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source.search(scope, query, limit).await.map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn read_evidence<'py>(
        &self,
        py: Python<'py>,
        reference_json: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let reference: SourceRef = parse(reference_json)?;
        let source = self.inner.clone();
        let scope = self.scope.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source
                .read_evidence(scope, reference)
                .await
                .map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn index_document<'py>(
        &self,
        py: Python<'py>,
        input_json: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input: DocumentInput = parse(input_json)?;
        let source = self.documents_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source.index_document(input).await.map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn indexed_document<'py>(
        &self,
        py: Python<'py>,
        input_json: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input: DocumentInput = parse(input_json)?;
        let source = self.documents_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source.indexed_document(input).await.map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn document_inputs<'py>(
        &self,
        py: Python<'py>,
        after: Option<String>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let source = self.documents_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source.inputs(after, limit).await.map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn reconcile_documents<'py>(
        &self,
        py: Python<'py>,
        after: Option<String>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let source = self.documents_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source
                .reconcile_sources(after, limit)
                .await
                .map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn reconcile_memory_embeddings<'py>(
        &self,
        py: Python<'py>,
        offset: usize,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let Maintenance::Memory(source) = &self.maintenance else {
            return Err(invalid());
        };
        let source = source.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source
                .reconcile_embeddings(offset, limit)
                .await
                .map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn reconcile_embeddings<'py>(
        &self,
        py: Python<'py>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let source = self.documents_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            source.reconcile_embeddings(limit).await.map_err(error)
        })
    }
    fn sync_session<'py>(
        &self,
        py: Python<'py>,
        session_json: &str,
        max_records: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let session: SessionId = parse(session_json)?;
        let source = self.journal_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source
                .sync_session(session, max_records)
                .await
                .map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn index_reference<'py>(
        &self,
        py: Python<'py>,
        source_id: String,
        reference_json: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let reference: SourceRef = parse(reference_json)?;
        let source = self.graph_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source
                .index_reference(&source_id, reference)
                .await
                .map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn rebuild_graph<'py>(
        &self,
        py: Python<'py>,
        cursor_json: &str,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let cursor: Option<finstack_ai_index_graph::GraphBuildCursor> = parse(cursor_json)?;
        let source = self.graph_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source.rebuild_step(cursor, limit).await.map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn reconcile_graph<'py>(
        &self,
        py: Python<'py>,
        after: Option<String>,
        limit: usize,
    ) -> PyResult<Bound<'py, PyAny>> {
        let source = self.graph_source()?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = source.reconcile(after, limit).await.map_err(error)?;
            Python::attach(|py| json(py, &result))
        })
    }
    fn observer(&self) -> PyResult<super::PyJournalIndexObserver> {
        super::PyJournalIndexObserver::new(self.journal_source()?)
    }
}
impl PySearchSource {
    fn documents_source(&self) -> PyResult<Arc<DocumentSearchSource>> {
        match &self.maintenance {
            Maintenance::Documents(source, _) => Ok(source.clone()),
            _ => Err(error(SearchError::invalid("document_source_required"))),
        }
    }
    fn journal_source(&self) -> PyResult<Arc<JournalSearchSource>> {
        match &self.maintenance {
            Maintenance::Journal(source) => Ok(source.clone()),
            _ => Err(error(SearchError::invalid("journal_source_required"))),
        }
    }
    fn graph_source(&self) -> PyResult<Arc<GraphSearchSource>> {
        match &self.maintenance {
            Maintenance::Graph(source) => Ok(source.clone()),
            _ => Err(error(SearchError::invalid("graph_source_required"))),
        }
    }
}
