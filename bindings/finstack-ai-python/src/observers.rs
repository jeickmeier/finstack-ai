//! Python constructors for native Rust observer extensions.

use std::io::Write;
use std::sync::{Arc, Mutex};

use finstack_ai::runtime::ports::observer::Observer;
use finstack_ai_kernel::ComponentRef;
use finstack_ai_observer_billing::BillingObserver;
use finstack_ai_observer_log::LogObserver;
use finstack_ai_observer_metrics::MetricsObserver;
use finstack_ai_observer_notify::{
    DeliveryPolicy, InteractionNotification, NotificationSink, NotifyObserver, SinkError,
};
use finstack_ai_observer_otel::OtelObserver;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::callbacks::parse_payload_mode;

/// Structured NDJSON run-event observer backed by the Rust implementation.
///
/// Writes one JSON line per observed run event to a file (append mode) or
/// to the process's stderr. Payload visibility is bounded by
/// `payload_mode`: `"metadata_only"` (default) never includes event
/// bodies, `"redacted"` includes only public-sensitivity bodies, and
/// `"full"` includes everything except credential-sensitivity bodies.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "LogObserver",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyLogObserver {
    inner: Arc<LogObserver>,
}

impl PyLogObserver {
    fn from_writer(payload_mode: &str, writer: Arc<Mutex<dyn Write + Send>>) -> PyResult<Self> {
        let observer = LogObserver::try_new(parse_payload_mode(payload_mode)?, writer)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(observer),
        })
    }

    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.inner.descriptor().component, self.inner.clone())
    }
}

#[pymethods]
impl PyLogObserver {
    /// Append NDJSON event lines to the file at `path`.
    #[new]
    #[pyo3(signature = (path, payload_mode = "metadata_only"))]
    fn new(path: &str, payload_mode: &str) -> PyResult<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| PyValueError::new_err(format!("log path unwritable: {error}")))?;
        Self::from_writer(payload_mode, Arc::new(Mutex::new(file)))
    }

    /// Write NDJSON event lines to the process's stderr.
    #[staticmethod]
    #[pyo3(signature = (payload_mode = "metadata_only"))]
    fn stderr(payload_mode: &str) -> PyResult<Self> {
        Self::from_writer(payload_mode, Arc::new(Mutex::new(std::io::stderr())))
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor().component.id().to_string()
    }
}

/// Prometheus text-exposition metrics observer backed by the Rust
/// implementation.
///
/// Metadata-only: it aggregates run/effect/latency counters and renders
/// them in Prometheus text format via `encode_prometheus()`.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "MetricsObserver",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyMetricsObserver {
    inner: Arc<MetricsObserver>,
}

#[pymethods]
impl PyMetricsObserver {
    /// Construct the metadata-only metrics observer.
    #[new]
    fn new() -> PyResult<Self> {
        let observer =
            MetricsObserver::try_new().map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(observer),
        })
    }

    /// Render the aggregated counters in Prometheus text format.
    fn encode_prometheus(&self) -> String {
        self.inner.encode_prometheus()
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor().component.id().to_string()
    }
}

impl PyMetricsObserver {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.inner.descriptor().component, self.inner.clone())
    }
}

/// OpenTelemetry span observer with an in-process capture buffer.
///
/// Spans carry identifier and classification attributes only unless a
/// public body is authorized by `payload_mode`. `spans()` snapshots the
/// bounded in-memory capture for inspection.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "OtelObserver",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyOtelObserver {
    inner: Arc<OtelObserver>,
}

#[pymethods]
impl PyOtelObserver {
    /// Construct the observer with a bounded in-memory span capture.
    #[new]
    #[pyo3(signature = (payload_mode = "metadata_only", capture_capacity = 1024))]
    fn new(payload_mode: &str, capture_capacity: usize) -> PyResult<Self> {
        let observer = OtelObserver::try_new(parse_payload_mode(payload_mode)?, capture_capacity)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(observer),
        })
    }

    /// Snapshot captured spans as `{"name", "attributes"}` mappings.
    fn spans(&self, py: Python<'_>) -> PyResult<Vec<Py<pyo3::types::PyDict>>> {
        let captured = self
            .inner
            .snapshot()
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        captured
            .into_iter()
            .map(|span| {
                let entry = pyo3::types::PyDict::new(py);
                entry.set_item("name", span.name)?;
                let attributes = pyo3::types::PyDict::new(py);
                for (key, value) in span.attributes {
                    attributes.set_item(key, value)?;
                }
                entry.set_item("attributes", attributes)?;
                Ok(entry.unbind())
            })
            .collect()
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor().component.id().to_string()
    }
}

impl PyOtelObserver {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.inner.descriptor().component, self.inner.clone())
    }
}

/// Bounded in-memory billing ledger observer.
///
/// Aggregates spend and usage attribution rows; `export_jsonl()` renders
/// the ledger as JSON lines for reconciliation.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "BillingObserver",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyBillingObserver {
    inner: Arc<BillingObserver>,
}

#[pymethods]
impl PyBillingObserver {
    /// Construct the ledger with a bounded entry count.
    #[new]
    #[pyo3(signature = (max_entries = 1024))]
    fn new(max_entries: usize) -> PyResult<Self> {
        let observer = BillingObserver::try_new(max_entries)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(observer),
        })
    }

    /// Render the ledger as JSON lines.
    fn export_jsonl(&self) -> PyResult<String> {
        self.inner
            .export_jsonl()
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor().component.id().to_string()
    }
}

impl PyBillingObserver {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.inner.descriptor().component, self.inner.clone())
    }
}

/// Bridge from a Python callable to the notify crate's outbound sink.
struct PythonNotificationSink {
    callback: Py<PyAny>,
}

impl NotificationSink for PythonNotificationSink {
    fn name(&self) -> &'static str {
        "python-callback"
    }

    fn deliver(
        &self,
        notification: InteractionNotification,
    ) -> finstack_ai::runtime::ports::PortFuture<Result<(), SinkError>> {
        let outcome = serde_json::to_string(&notification)
            .map_err(|_| SinkError::Unavailable {
                reason: "notification_encoding_failed",
            })
            .and_then(|encoded| {
                Python::attach(|py| {
                    self.callback
                        .bind(py)
                        .call1((encoded,))
                        .map(|_| ())
                        .map_err(|_| SinkError::Unavailable {
                            reason: "python_sink_raised",
                        })
                })
            });
        Box::pin(async move { outcome })
    }
}

/// Announce-only interaction lifecycle observer over a Python sink.
///
/// The sink callable receives one JSON string per interaction lifecycle
/// event (identifiers and whitelisted detail only — never prompt content
/// or resolution payloads). A raising sink counts as a failed delivery
/// after the policy's attempts are exhausted.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "NotifyObserver",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyNotifyObserver {
    inner: Arc<NotifyObserver>,
}

#[pymethods]
impl PyNotifyObserver {
    /// Construct the observer over a Python sink callable.
    #[new]
    #[pyo3(signature = (sink, *, request_timeout_ms = 1_000, max_attempts = 1, retry_backoff_ms = 0))]
    fn new(
        sink: Py<PyAny>,
        request_timeout_ms: u64,
        max_attempts: u32,
        retry_backoff_ms: u64,
    ) -> PyResult<Self> {
        let policy = DeliveryPolicy::try_new(
            std::time::Duration::from_millis(request_timeout_ms),
            max_attempts,
            std::time::Duration::from_millis(retry_backoff_ms),
        )
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
        let observer =
            NotifyObserver::try_new(Arc::new(PythonNotificationSink { callback: sink }), policy)
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(observer),
        })
    }

    /// Notifications delivered successfully.
    #[getter]
    fn delivered(&self) -> u64 {
        self.inner.delivered()
    }

    /// Notifications dropped after exhausting delivery attempts.
    #[getter]
    fn failed(&self) -> u64 {
        self.inner.failed()
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor().component.id().to_string()
    }
}

impl PyNotifyObserver {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.inner.descriptor().component, self.inner.clone())
    }
}
