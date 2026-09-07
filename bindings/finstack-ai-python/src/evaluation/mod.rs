//! Coarse Python handles over Rust-owned evaluation execution and reports.
mod scorer;
mod subject;
use finstack_ai_eval::{
    EvalError, EvalReport, EvalRunner, EvalSpec, EvalStore, MemoryEvalStore, SqliteEvalStore,
    StoreSnapshot, ThresholdGate,
};
use pyo3::prelude::*;
pub(crate) use scorer::PyEvalScorer;
use std::{io::BufWriter, path::PathBuf, sync::Arc};
pub(crate) use subject::PyEvalSubject;

#[allow(
    clippy::needless_pass_by_value,
    reason = "Owned error adapter for Result::map_err"
)]
pub(super) fn error(error: EvalError) -> PyErr {
    Python::attach(|py| {
        match py
            .get_type::<crate::RuntimeError>()
            .call1((error.to_string(),))
        {
            Ok(value) => {
                let _ = value.setattr("code", error.code());
                let _ = value.setattr("retryable", false);
                let _ = value.setattr("context", py.None());
                PyErr::from_value(value)
            }
            Err(error) => error,
        }
    })
}
pub(super) fn invalid() -> PyErr {
    error(EvalError::new(
        finstack_ai_eval::EVAL_SPEC_INVALID,
        "invalid evaluation configuration",
    ))
}
pub(super) fn parse<T: serde::de::DeserializeOwned>(text: &str, max_bytes: usize) -> PyResult<T> {
    if text.len() > max_bytes {
        return Err(invalid());
    }
    serde_json::from_str(text).map_err(|_| invalid())
}
fn json(py: Python<'_>, value: &impl serde::Serialize) -> PyResult<Py<PyAny>> {
    crate::json_bridge::json_to_py(py, &serde_json::to_value(value).map_err(|_| invalid())?)
}

/// Private handle behind typed public memory/SQLite store wrappers.
#[pyclass(module = "finstack_ai._finstack_ai", name = "_EvalStore", frozen)]
pub(crate) struct PyEvalStore {
    inner: Arc<dyn EvalStore>,
}
#[pymethods]
impl PyEvalStore {
    #[staticmethod]
    fn memory() -> Self {
        Self {
            inner: Arc::new(MemoryEvalStore::new()),
        }
    }
    #[staticmethod]
    fn sqlite(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        py.detach(move || {
            Ok(Self {
                inner: Arc::new(SqliteEvalStore::try_open(path).map_err(error)?),
            })
        })
    }
    fn snapshot(&self, py: Python<'_>) -> PyResult<PyEvalResult> {
        let inner = Arc::clone(&self.inner);
        py.detach(move || PyEvalResult::new(inner.snapshot().map_err(error)?, None))
    }
}
/// Immutable result retaining the Rust snapshot; Python copies never mutate storage.
#[pyclass(module = "finstack_ai._finstack_ai", name = "_EvalResult", frozen)]
pub(crate) struct PyEvalResult {
    snapshot: Arc<StoreSnapshot>,
    report: Arc<EvalReport>,
    stop_reason: Option<Arc<str>>,
}
impl PyEvalResult {
    fn new(snapshot: StoreSnapshot, stop_reason: Option<Arc<str>>) -> PyResult<Self> {
        let report = Arc::new(EvalReport::from_snapshot(&snapshot).map_err(error)?);
        Ok(Self {
            snapshot: Arc::new(snapshot),
            report,
            stop_reason,
        })
    }
}
#[pymethods]
impl PyEvalResult {
    #[getter]
    fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }
    fn report(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json(py, self.report.as_ref())
    }
    fn snapshot(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json(py, self.snapshot.as_ref())
    }
    fn gate(&self, py: Python<'_>, policy_json: &str) -> PyResult<Py<PyAny>> {
        let policy: ThresholdGate = parse(policy_json, 4096)?;
        json(py, &policy.evaluate(&self.report).map_err(error)?)
    }
    fn export_jsonl(&self, py: Python<'_>, path: PathBuf) -> PyResult<()> {
        let snapshot = Arc::clone(&self.snapshot);
        py.detach(move || {
            let file = std::fs::File::create(path).map_err(|_| store_error())?;
            let mut writer = BufWriter::new(file);
            finstack_ai_eval::export_jsonl(&snapshot, &mut writer).map_err(error)?;
            std::io::Write::flush(&mut writer).map_err(|_| store_error())
        })
    }
    fn write_summary(&self, py: Python<'_>, path: PathBuf) -> PyResult<()> {
        let report = Arc::clone(&self.report);
        py.detach(move || {
            let file = std::fs::File::create(path).map_err(|_| store_error())?;
            let mut writer = BufWriter::new(file);
            report.write_json(&mut writer).map_err(error)?;
            std::io::Write::flush(&mut writer).map_err(|_| store_error())
        })
    }
}
/// Private native runner behind `finstack_ai.eval.EvalRunner`.
#[pyclass(module = "finstack_ai._finstack_ai", name = "_EvalRunner", frozen)]
pub(crate) struct PyEvalRunner {
    inner: EvalRunner,
}
#[pymethods]
impl PyEvalRunner {
    #[new]
    fn new(
        py: Python<'_>,
        spec_json: &str,
        store: &Bound<'_, PyEvalStore>,
        subjects: Vec<Py<PyEvalSubject>>,
        scorers: Vec<Py<PyEvalScorer>>,
    ) -> PyResult<Self> {
        let spec: EvalSpec = parse(spec_json, finstack_ai_eval::MAX_SPEC_BYTES)?;
        let subjects = subjects
            .into_iter()
            .map(|s| s.borrow(py).bind(&spec.tasks))
            .collect::<PyResult<Vec<_>>>()?;
        let scorers = scorers
            .into_iter()
            .map(|s| Arc::clone(&s.borrow(py).inner))
            .collect();
        let store = Arc::clone(&store.borrow().inner);
        py.detach(move || {
            Ok(Self {
                inner: EvalRunner::new(spec, store, subjects, scorers).map_err(error)?,
            })
        })
    }
    fn cancel(&self) {
        self.inner.cancel();
    }
    fn run<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let runner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = runner.run().await.map_err(error)?;
            PyEvalResult::new(result.snapshot, result.stop_reason)
        })
    }
    fn resume<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.run(py)
    }
    fn rescore<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let runner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let result = runner.rescore().await.map_err(error)?;
            PyEvalResult::new(result.snapshot, result.stop_reason)
        })
    }
}
#[pyfunction]
pub(crate) fn _eval_spec_digest(py: Python<'_>, spec_json: String) -> PyResult<String> {
    py.detach(move || {
        let spec: EvalSpec = parse(&spec_json, finstack_ai_eval::MAX_SPEC_BYTES)?;
        Ok(spec.digest().map_err(error)?.to_string())
    })
}

fn store_error() -> PyErr {
    error(EvalError::new(
        finstack_ai_eval::EVAL_STORE_UNAVAILABLE,
        "evaluation export writer failed",
    ))
}
