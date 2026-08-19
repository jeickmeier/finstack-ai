//! Shared run handle, terminal result snapshot, and run-request construction.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai::runtime::{
    ArtifactMetadata, ArtifactStore, Bytes, ExternalRouteOutcome, ModelName, ModelSettings,
    stage_required_artifact,
};
use finstack_ai::{
    AgentRunError, AgentRunOutput, AgentRunRequest, AttachmentInput, MAX_RUN_ATTACHMENTS,
    PrincipalRef, RemoteChildRouteSpec, RunSecurityContext,
};
use finstack_ai_kernel::{
    CapabilityId, ChildPlacement, Metadata, OperationLocator, RawJson, Sensitivity, SessionId,
};
use finstack_ai_middleware_document_ingest::AttachmentIndex;
use pyo3::exceptions::{PyException, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::agent::PyAgent;
use crate::callbacks::normalize_pydantic_schema;
use crate::errors::{agent_error, configuration_error};
use crate::events::PyEventIterator;
use crate::json_bridge::{json_to_py, py_to_json};
use crate::locator::{PyLocator, locator_dict};
use crate::session::PySession;

pub(crate) const MAX_TIMEOUT_SECONDS: f64 = 86_400.0;

/// Shared control handle for one Rust-owned run.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Run", frozen)]
pub(crate) struct PyRun {
    pub(crate) inner: finstack_ai::AgentRun,
    pub(crate) output_adapter: Option<Py<PyAny>>,
}

#[pymethods]
impl PyRun {
    /// Live session handle for this run.
    #[getter]
    fn session(&self) -> PySession {
        PySession {
            inner: self.inner.session(),
        }
    }

    /// Immutable operation locator snapshot.
    #[getter]
    fn locator(&self) -> PyLocator {
        PyLocator {
            locator: Arc::new(self.inner.locator().clone()),
        }
    }

    /// Wait for the retained terminal result.
    fn result<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            let result = run.result().await;
            Python::attach(|py| {
                result_to_python_with_locator(py, result, Some(&locator), output_adapter)
            })
        })
    }

    /// List the outstanding typed interaction for this run (0 or 1).
    #[pyo3(text_signature = "($self)")]
    fn list_interactions<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.list_interactions().await {
                Ok(requests) => Python::attach(|py| {
                    let value = serde_json::to_value(&requests).map_err(|_| {
                        PyException::new_err("interaction list serialization failed")
                    })?;
                    json_to_py(py, &value)
                }),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, Some(&locator)))),
            }
        })
    }

    /// Resolve the outstanding interaction through the live Rust-owned run.
    #[pyo3(text_signature = "($self, resolution)")]
    fn resolve_interaction<'py>(
        &self,
        py: Python<'py>,
        resolution: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let resolution =
            serde_json::from_value::<finstack_ai::InteractionResolution>(py_to_json(resolution)?)
                .map_err(|error| PyTypeError::new_err(error.to_string()))?;
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.resolve_interaction(resolution).await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, Some(&locator)))),
            }
        })
    }

    /// Prepare and accept one child run through the Rust child-run router.
    #[pyo3(signature = (agent, input, *, placement = "isolated_child_session", timeout_seconds = None, max_cycles = crate::agent::DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None, route_endpoint = None, route_service = None, route_id = None, route_token = None))]
    #[pyo3(
        text_signature = "($self, agent, input, *, placement='isolated_child_session', timeout_seconds=None, max_cycles=16, max_output_retries=1, capability=None, route_endpoint=None, route_service=None, route_id=None, route_token=None)"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "child start forwards the same bounded run request fields as Agent.start"
    )]
    fn start_child<'py>(
        &self,
        py: Python<'py>,
        agent: &Bound<'py, PyAgent>,
        input: String,
        placement: &str,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
        route_endpoint: Option<String>,
        route_service: Option<String>,
        route_id: Option<String>,
        route_token: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let placement = parse_child_placement(placement)?;
        let remote = remote_route(route_endpoint, route_service, route_id, route_token)?;
        let borrowed = agent.borrow();
        let child = Arc::clone(&borrowed.inner);
        let model = borrowed.model.clone();
        let settings = borrowed.settings.clone();
        let timeout_seconds = timeout_seconds.unwrap_or(borrowed.default_timeout_seconds);
        let output_adapter = borrowed
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        drop(borrowed);
        let parent = self.inner.clone();
        let tenant_scope = parent.locator().tenant_scope.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let request = run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                settings,
                &tenant_scope,
                Vec::new(),
            )
            .map_err(|error| Python::attach(|py| agent_error(py, &error, None)))?;
            match Box::pin(parent.start_child(&child, request, placement, remote)).await {
                Ok(inner) => Python::attach(|py| {
                    Py::new(
                        py,
                        PyRun {
                            inner,
                            output_adapter,
                        },
                    )
                }),
                Err(error) => {
                    Python::attach(|py| Err(agent_error(py, &error, Some(parent.locator()))))
                }
            }
        })
    }

    /// Route one authenticated external completion through the Rust ingress.
    #[pyo3(text_signature = "($self, command)")]
    fn complete_external<'py>(
        &self,
        py: Python<'py>,
        command: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let encoded = serde_json::to_string(&py_to_json(command)?)
            .map_err(|_| PyTypeError::new_err("command is not JSON serializable"))?;
        let normalized = crate::protocol::normalize_encoded_shape::<
            finstack_ai_kernel::ExternalEffectCompletionCommand,
        >(&encoded)?;
        let command = serde_json::from_str::<finstack_ai_kernel::ExternalEffectCompletionCommand>(
            &normalized,
        )
        .map_err(|error| PyTypeError::new_err(error.to_string()))?;
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match Box::pin(run.complete_external(command)).await {
                Ok(outcome) => Python::attach(|py| route_outcome_to_python(py, &outcome)),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, Some(&locator)))),
            }
        })
    }

    /// Submit idempotent durable cancellation.
    fn cancel<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let locator = run.locator().clone();
            match run.cancel().await {
                Ok(()) => Ok(()),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, Some(&locator)))),
            }
        })
    }

    /// Return the batch-first asynchronous event iterator.
    fn events(&self) -> PyEventIterator {
        PyEventIterator {
            run: self.inner.clone(),
        }
    }

    /// Close event observation without cancelling execution.
    fn close_events<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let run = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            run.close_events();
            Ok(())
        })
    }
}

/// One in-memory run attachment staged at submit time.
///
/// Exactly one of `data`/`path` is required. A `path` is read (bounded by 4
/// MiB) at construction time; its basename becomes the default `name` when
/// `name` is not given explicitly.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "Attachment",
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyAttachment {
    pub(crate) data: Vec<u8>,
    pub(crate) media_type: String,
    pub(crate) name: Option<String>,
}

/// V1 individual byte-string ceiling shared with `finstack-ai-runtime`'s
/// `ArtifactStore` contract (spec decision 21).
pub(crate) const MAX_ATTACHMENT_PATH_BYTES: usize = 4 * 1024 * 1024;

/// Resolve exactly one of `data`/`path` into owned bytes.
///
/// Shared by [`PyAttachment::new`] and the debug `parse_document*` helpers
/// so both apply the same exactly-one-of contract and 4 MiB path cap.
pub(crate) fn resolve_data_or_path(data: Option<Vec<u8>>, path: Option<&str>) -> PyResult<Vec<u8>> {
    match (data, path) {
        (Some(data), None) => Ok(data),
        (None, Some(path)) => {
            let bytes =
                std::fs::read(path).map_err(|error| PyValueError::new_err(error.to_string()))?;
            if bytes.len() > MAX_ATTACHMENT_PATH_BYTES {
                return Err(PyValueError::new_err("attachment exceeds 4 MiB"));
            }
            Ok(bytes)
        }
        _ => Err(PyValueError::new_err(
            "exactly one of data or path is required",
        )),
    }
}

#[pymethods]
impl PyAttachment {
    #[new]
    #[pyo3(signature = (media_type, data = None, path = None, name = None))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "pyo3 #[new] constructors take owned Python-extracted arguments"
    )]
    fn new(
        media_type: String,
        data: Option<Vec<u8>>,
        path: Option<String>,
        name: Option<String>,
    ) -> PyResult<Self> {
        let data = resolve_data_or_path(data, path.as_deref())?;
        let name = name.or_else(|| {
            path.as_deref().and_then(|value| {
                std::path::Path::new(value)
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
            })
        });
        Ok(Self {
            data,
            media_type,
            name,
        })
    }
}

/// Clone owned attachment payloads out of the GIL-bound handles.
///
/// Must run while `py` is held; the returned values are plain `Send` data
/// that can move into a detached thread or async block.
pub(crate) fn collect_attachments(
    py: Python<'_>,
    attachments: Option<Vec<Py<PyAttachment>>>,
) -> Vec<PyAttachment> {
    attachments
        .unwrap_or_default()
        .into_iter()
        .map(|attachment| attachment.bind(py).borrow().clone())
        .collect()
}

/// Deterministic placeholder scope used to stage run attachments ahead of
/// the run's real session/run identity (unknown until `Agent::start`
/// accepts the request). `InProcessArtifactStore::get` resolves purely by
/// content-derived `ArtifactId` and ignores the scope passed to `get`, so a
/// stable placeholder session id is sufficient here; `stage_required_artifact`
/// only checks the staged artifact against the *same* scope passed to it.
fn attachment_scope(tenant_scope: &str) -> finstack_ai::runtime::ArtifactScope {
    finstack_ai::runtime::ArtifactScope {
        tenant_scope: std::sync::Arc::from(tenant_scope),
        session_id: SessionId::from_bytes([0_u8; 16]),
        run_id: None,
        sensitivity: Sensitivity::Internal,
    }
}

/// Stage every attachment into `store` and record it in `index` so
/// `DocumentIngestMiddleware` can resolve the `BlobRef` it sees on the
/// journaled `File` block back to the exact staged `ArtifactRef`.
pub(crate) async fn stage_attachments(
    store: &dyn ArtifactStore,
    index: &AttachmentIndex,
    tenant_scope: &str,
    attachments: Vec<PyAttachment>,
) -> Result<Vec<AttachmentInput>, AgentRunError> {
    if attachments.len() > MAX_RUN_ATTACHMENTS {
        return Err(configuration_error(
            "run attachments exceed MAX_RUN_ATTACHMENTS",
        ));
    }
    let scope = attachment_scope(tenant_scope);
    let mut staged = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let artifact = stage_required_artifact(
            store,
            scope.clone(),
            Bytes::from(attachment.data),
            ArtifactMetadata {
                kind: std::sync::Arc::from("attachment"),
                media_type: std::sync::Arc::from(attachment.media_type),
                name: attachment.name.map(std::sync::Arc::from),
                attributes: Metadata::empty(),
            },
        )
        .await
        .map_err(|error| configuration_error(error.to_string()))?;
        index.insert(artifact.clone());
        staged.push(AttachmentInput { artifact });
    }
    Ok(staged)
}

/// Immutable successful terminal result snapshot.
#[pyclass(module = "finstack_ai._finstack_ai", name = "RunResult", frozen)]
pub(crate) struct PyRunResult {
    inner: AgentRunOutput,
    output: Option<Py<PyAny>>,
}

pub(crate) struct PreparedPydanticOutput {
    pub(crate) adapter: Py<PyAny>,
    pub(crate) schema: RawJson,
}

pub(crate) fn prepare_pydantic_output(
    py: Python<'_>,
    target: Py<PyAny>,
) -> PyResult<PreparedPydanticOutput> {
    let pydantic = py.import("pydantic").map_err(|_| {
        PyTypeError::new_err(
            "output_type requires the optional Pydantic extra: install finstack-ai[pydantic]",
        )
    })?;
    let adapter_type = pydantic.getattr("TypeAdapter")?;
    let adapter = if target.bind(py).is_instance(&adapter_type)? {
        target
    } else {
        adapter_type.call1((target,))?.unbind()
    };
    let kwargs = PyDict::new(py);
    kwargs.set_item("mode", "validation")?;
    let schema = adapter
        .bind(py)
        .call_method("json_schema", (), Some(&kwargs))?;
    let raw = raw_pydantic_schema(&schema, "structured_output")?;
    Ok(PreparedPydanticOutput {
        adapter,
        schema: raw,
    })
}

fn raw_pydantic_schema(schema: &Bound<'_, PyAny>, kind: &str) -> PyResult<RawJson> {
    let value = py_to_json(schema)?;
    let normalized = normalize_pydantic_schema(value, kind).map_err(PyTypeError::new_err)?;
    let bytes = serde_json::to_vec(&normalized)
        .map_err(|_| PyException::new_err("Pydantic schema normalization failed"))?;
    RawJson::parse(bytes).map_err(|_| PyException::new_err("Pydantic schema is invalid JSON"))
}

#[pymethods]
impl PyRunResult {
    #[getter]
    fn text(&self) -> String {
        self.inner.text()
    }

    /// Typed structured output, when this run configured `output_type`.
    #[getter]
    fn output(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.output.as_ref().map(|output| output.clone_ref(py))
    }

    /// Durable retry attempts consumed by this run.
    #[getter]
    fn retry_attempts(&self) -> u32 {
        self.inner.retry_attempts()
    }

    /// Complete Rust-owned capability activation set for this run.
    #[getter]
    fn active_capabilities(&self, py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
        self.inner
            .active_capabilities()
            .iter()
            .map(|active| {
                let value = PyDict::new(py);
                value.set_item("id", active.capability_id.as_str())?;
                value.set_item(
                    "source",
                    match active.source {
                        finstack_ai::CapabilityActivationSource::Always => "always",
                        finstack_ai::CapabilityActivationSource::Application => "application",
                        finstack_ai::CapabilityActivationSource::Model => "model",
                    },
                )?;
                Ok(value.unbind())
            })
            .collect()
    }

    /// Stable Rust-owned committed record-kind trace in journal order.
    #[getter]
    fn trace(&self) -> Vec<&str> {
        self.inner
            .record_kinds()
            .iter()
            .map(AsRef::as_ref)
            .collect()
    }

    #[getter]
    fn locator(&self) -> PyLocator {
        PyLocator {
            locator: Arc::new(self.inner.locator.clone()),
        }
    }

    #[getter]
    fn session(&self) -> PyLocator {
        PyLocator {
            locator: Arc::new(self.inner.locator.clone()),
        }
    }

    /// Serialize the immutable snapshot on explicit request.
    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let value = locator_dict(py, &self.inner.locator)?;
        value.bind(py).set_item("text", self.inner.text())?;
        Ok(value)
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "run request forwards model, bounds, capability, settings, and tenant distinctly"
)]
pub(crate) fn run_request(
    model: &ModelName,
    input: String,
    timeout_seconds: f64,
    max_cycles: u64,
    max_output_retries: u32,
    capability: Option<String>,
    settings: ModelSettings,
    tenant_scope: &str,
    attachments: Vec<AttachmentInput>,
) -> Result<AgentRunRequest, AgentRunError> {
    if attachments.len() > MAX_RUN_ATTACHMENTS {
        return Err(configuration_error(
            "run attachments exceed MAX_RUN_ATTACHMENTS",
        ));
    }
    if !timeout_seconds.is_finite()
        || timeout_seconds <= 0.0
        || timeout_seconds > MAX_TIMEOUT_SECONDS
    {
        return Err(configuration_error(
            "timeout_seconds must be finite and in (0, 86400]",
        ));
    }
    let security = RunSecurityContext::try_new(
        tenant_scope,
        PrincipalRef::try_new("finstack-ai-python", "local-user", Some(tenant_scope))
            .map_err(|error| configuration_error(error.to_string()))?,
        "local",
        "python-embedded",
        "python-policy-v1",
        "python-decision-v1",
        None,
    )
    .map_err(|error| configuration_error(error.to_string()))?;
    let mut request = AgentRunRequest::try_new(model.clone(), input, security)?;
    request.settings = settings;
    request.timeout = Duration::from_secs_f64(timeout_seconds);
    request.max_cycles = max_cycles;
    request.max_output_retries = max_output_retries;
    if let Some(capability) = capability {
        request.capability = Some(
            CapabilityId::parse(&capability)
                .map_err(|error| configuration_error(error.to_string()))?,
        );
    }
    request.attachments = attachments.into();
    Ok(request)
}

fn remote_route(
    endpoint: Option<String>,
    service: Option<String>,
    route: Option<String>,
    token: Option<String>,
) -> PyResult<Option<RemoteChildRouteSpec>> {
    match (endpoint, service, route) {
        (None, None, None) => Ok(None),
        (Some(endpoint), Some(service), Some(route)) => Ok(Some(RemoteChildRouteSpec {
            endpoint,
            service,
            route,
            token,
        })),
        _ => Err(PyTypeError::new_err(
            "remote child route requires route_endpoint, route_service, and route_id",
        )),
    }
}

fn parse_child_placement(value: &str) -> PyResult<ChildPlacement> {
    match value {
        "compatible_lane_in_parent_session" => Ok(ChildPlacement::CompatibleLaneInParentSession),
        "isolated_child_session" => Ok(ChildPlacement::IsolatedChildSession),
        "remote_child_session" => Ok(ChildPlacement::RemoteChildSession),
        _ => Err(PyTypeError::new_err(format!(
            "unsupported child placement: {value}"
        ))),
    }
}

fn route_outcome_to_python(py: Python<'_>, outcome: &ExternalRouteOutcome) -> PyResult<Py<PyAny>> {
    let status = match outcome {
        ExternalRouteOutcome::Committed(_) => "committed",
        ExternalRouteOutcome::Idempotent { .. } => "idempotent",
        ExternalRouteOutcome::Rejected { .. } => "rejected",
    };
    let value = serde_json::json!({ "status": status });
    json_to_py(py, &value)
}

pub(crate) fn result_to_python_with_locator(
    py: Python<'_>,
    result: Result<AgentRunOutput, AgentRunError>,
    locator: Option<&OperationLocator>,
    output_adapter: Option<Py<PyAny>>,
) -> PyResult<Py<PyRunResult>> {
    match result {
        Ok(inner) => {
            let output = output_adapter
                .map(|adapter| {
                    let raw = inner.structured_json().ok_or_else(|| {
                        PyException::new_err("structured result is missing canonical JSON")
                    })?;
                    adapter.call_method1(py, "validate_json", (PyBytes::new(py, raw.as_bytes()),))
                })
                .transpose()?;
            Py::new(py, PyRunResult { inner, output })
        }
        Err(error) => Err(agent_error(py, &error, locator)),
    }
}
