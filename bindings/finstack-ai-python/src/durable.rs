//! Coarse Python conversion over the Rust-owned durable host.

use finstack_ai::DEFAULT_MAX_CYCLES;
use finstack_ai::durable::{DurableHost, DurableHostBuilder, DurableHostError, ResolutionInput};
use finstack_ai::runtime::ports::journal::StoreLimits;
use finstack_ai_kernel::{RawJson, TerminalState};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::agent::PyAgent;
use crate::json_bridge::{json_to_py, py_to_json};
use crate::locator::{PyLocator, locator_dict};
use crate::run::{PyAttachment, collect_attachments, run_request, stage_attachments};

/// Embedded `SQLite` host; all scheduling and recovery remains in Rust.
#[pyclass(module = "finstack_ai._finstack_ai", name = "DurableHost", frozen)]
pub(crate) struct PyDurableHost {
    inner: Arc<DurableHost>,
    definitions: BTreeMap<String, PyAgent>,
    tenant_scope: String,
}

fn host_error(error: DurableHostError) -> PyErr {
    let message = error.to_string();
    let DurableHostError { code } = error;
    Python::attach(|py| {
        let result = py.get_type::<crate::RuntimeError>().call1((message,));
        match result {
            Ok(value) => {
                let _ = value.setattr("code", code.as_ref());
                let _ = value.setattr("retryable", false);
                let _ = value.setattr("context", py.None());
                PyErr::from_value(value)
            }
            Err(error) => error,
        }
    })
}

#[pymethods]
impl PyDurableHost {
    /// Open durable `SQLite` state and register application definitions by kind.
    /// Agents are resolved against the host journal. Their credentials, typed
    /// toolsets and artifact stores remain supplied by the application.
    #[staticmethod]
    #[pyo3(signature = (path, agents, *, tenant_scope = "python-local", worker_id = None, drive_timeout_seconds = 30.0, lease_ttl_seconds = 60.0))]
    fn open<'py>(
        py: Python<'py>,
        path: String,
        agents: &Bound<'py, PyDict>,
        tenant_scope: &str,
        worker_id: Option<String>,
        drive_timeout_seconds: f64,
        lease_ttl_seconds: f64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let definitions = agents
            .iter()
            .map(|(kind, value)| {
                Ok((
                    kind.extract::<String>()?,
                    value.cast::<PyAgent>()?.borrow().clone_ref(py),
                ))
            })
            .collect::<PyResult<BTreeMap<_, _>>>()?;
        let drive_timeout = std::time::Duration::try_from_secs_f64(drive_timeout_seconds)
            .map_err(|_| PyValueError::new_err("invalid drive_timeout_seconds"))?;
        let lease_ttl = std::time::Duration::try_from_secs_f64(lease_ttl_seconds)
            .map_err(|_| PyValueError::new_err("invalid lease_ttl_seconds"))?;
        let lease_ttl_ms = u64::try_from(lease_ttl.as_millis())
            .map_err(|_| PyValueError::new_err("lease_ttl_seconds out of range"))?;
        let tenant_scope = tenant_scope.to_owned();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut builder = DurableHostBuilder::try_open(
                &tenant_scope,
                path,
                StoreLimits {
                    sessions: 1024,
                    batches_per_session: 16_384,
                    records_per_session: 65_536,
                    snapshot_bytes: 8 * 1024 * 1024,
                },
            )
            .map_err(host_error)?;
            builder = builder
                .drive_timeout(drive_timeout)
                .lease_ttl_ms(lease_ttl_ms);
            if let Some(worker_id) = worker_id {
                builder = builder.worker_id(worker_id);
            }
            for (kind, agent) in &definitions {
                builder = builder
                    .register(kind, agent.inner.as_ref().clone())
                    .await
                    .map_err(host_error)?;
            }
            let inner = Arc::new(builder.build().map_err(host_error)?);
            Ok(Self {
                inner,
                definitions,
                tenant_scope,
            })
        })
    }

    /// Persist one admission without dispatching a model or tool. Call tick to
    /// advance it; retain the returned locator for inspection after restart.
    #[pyo3(signature = (workflow_kind, input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, attachments = None))]
    #[pyo3(
        text_signature = "($self, workflow_kind, input, *, timeout_seconds=None, max_cycles=16, max_output_retries=1, attachments=None)"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "same bounded run request inputs as Agent.start"
    )]
    fn start<'py>(
        &self,
        py: Python<'py>,
        workflow_kind: String,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        attachments: Option<Vec<Py<PyAttachment>>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = self
            .definitions
            .get(&workflow_kind)
            .ok_or_else(|| PyValueError::new_err("unknown durable workflow kind"))?
            .clone_ref(py);
        let timeout_seconds = timeout_seconds.unwrap_or(agent.default_timeout_seconds);
        let attachments = collect_attachments(py, attachments);
        let inner = Arc::clone(&self.inner);
        let tenant_scope = self.tenant_scope.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let attachments =
                stage_attachments(agent.artifact_store.as_ref(), &tenant_scope, attachments)
                    .await
                    .map_err(|error| crate::errors::agent_error(&error, None))?;
            let request = run_request(
                &agent.model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                None,
                agent.settings,
                &tenant_scope,
                attachments,
                agent.compaction_authorization,
            )
            .map_err(|error| crate::errors::agent_error(&error, None))?;
            let locator = inner
                .start(&workflow_kind, request)
                .await
                .map_err(host_error)?;
            Ok(PyLocator {
                locator: Arc::new(locator),
            })
        })
    }

    /// Advance due runs to their next durable wait or terminal state.
    fn tick<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let report = inner.tick().await.map_err(host_error)?;
            Python::attach(|py| {
                json_to_py(
                    py,
                    &serde_json::json!({ "cron_fires": report.cron_fires, "runs_started": report.runs_started, "sessions_resumed": report.sessions_resumed, "sessions_reparked": report.sessions_reparked, "sessions_expired": report.sessions_expired, "responses_rejected": report.responses_rejected, "failures": report.failures }),
                )
            })
        })
    }

    /// Inspect journal-derived state; missing descriptors, configuration drift,
    /// unavailable artifacts and unresolved effects fail with stable Rust codes.
    fn inspect<'py>(
        &self,
        py: Python<'py>,
        locator: &Bound<'py, PyLocator>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let locator = locator.borrow().locator.as_ref().clone();
        let inner = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inspected = inner.inspect(&locator).await.map_err(host_error)?;
            let output = match inspected.state.terminal() {
                Some(TerminalState::Completed(completed)) => inspected
                    .state
                    .messages()
                    .iter()
                    .find(|message| message.id() == &completed.result_message_id),
                _ => None,
            };
            Python::attach(|py| {
                let value = PyDict::new(py);
                value.set_item("locator", locator_dict(py, &locator)?)?;
                value.set_item("workflow_kind", inspected.workflow_kind.as_ref())?;
                value.set_item(
                    "phase",
                    json_to_py(
                        py,
                        &serde_json::to_value(inspected.state.phase())
                            .map_err(|_| PyValueError::new_err("phase conversion failed"))?,
                    )?,
                )?;
                value.set_item("terminal", inspected.state.terminal().is_some())?;
                value.set_item(
                    "state",
                    json_to_py(
                        py,
                        &serde_json::to_value(&inspected.state)
                            .map_err(|_| PyValueError::new_err("state conversion failed"))?,
                    )?,
                )?;
                value.set_item(
                    "message",
                    json_to_py(
                        py,
                        &serde_json::to_value(output)
                            .map_err(|_| PyValueError::new_err("message conversion failed"))?,
                    )?,
                )?;
                Ok(value.unbind())
            })
        })
    }

    /// Return pending interactions with the exact accepted principal and
    /// evidence needed to construct an authorized resolution.
    fn pending(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let rows = self.inner.pending().map_err(host_error)?;
        let values = rows.into_iter().map(|row| {
            let request = RawJson::parse(&row.request).map_err(|_| PyValueError::new_err("invalid stored interaction"))?;
            Ok(serde_json::json!({ "interaction_id": row.interaction_id, "tenant_scope": row.tenant_scope, "session_id": row.session_id, "lane_id": row.lane_id, "run_id": row.run_id, "request": request, "principal": row.accepted_principal, "evidence": row.accepted_evidence, "status": row.status.as_str() }))
        }).collect::<PyResult<Vec<_>>>()?;
        json_to_py(py, &serde_json::Value::Array(values))
    }

    /// Buffer a resolution containing `resolution_id`, principal, evidence,
    /// payload and optional note. Rust validates authority and exact identity.
    fn resolve(&self, interaction_id: &str, resolution: &Bound<'_, PyAny>) -> PyResult<()> {
        let input: ResolutionInput = serde_json::from_value(py_to_json(resolution)?)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        self.inner
            .resolve(interaction_id, input)
            .map_err(host_error)
    }

    /// Stop admissions and wait for the current tick to join all local work.
    /// This preserves accepted runs for a fresh process.
    fn shutdown<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner.shutdown().await;
            Ok(())
        })
    }
}
