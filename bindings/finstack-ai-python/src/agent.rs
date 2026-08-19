//! Rust-owned resolved agent handle and linked-provider builders.

use std::sync::Arc;
use std::time::Duration;

use crate::child_policy::PyChildRunPolicy;
use crate::store::{PySqliteDurability, open_journal_store};
use finstack_ai::runtime::{ArtifactStore, Middleware, Model, ModelName, ModelSettings, Toolset};
use finstack_ai::{
    Agent, AgentRunError, AnthropicAgentSpec, CapabilitySpec, ChildRunPolicy, E2bSandboxAgentSpec,
    GatewayAgentSpec, LinkedAgent, LinkedAgentPorts, LinkedCommon, OllamaAgentSpec,
    OpenAiAgentSpec, OpenRouterAgentSpec, Session,
};
use finstack_ai_context_memory::InProcessArtifactStore;
use finstack_ai_kernel::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentRef, RawJson, Version,
};
use finstack_ai_middleware_document_ingest::{AttachmentIndex, DocumentIngestMiddleware};
use finstack_ai_tools_document::DocumentToolset;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::ConfigurationError;
use crate::callbacks::{
    PyPythonContextProvider, PyPythonMiddleware, PyPythonModel, PyPythonObserver, PyPythonToolset,
};
use crate::capability::PyCapability;
use crate::errors::{agent_error, configuration_error, session_py_error};
use crate::run::{
    PreparedPydanticOutput, PyAttachment, PyRun, collect_attachments, prepare_pydantic_output,
    result_to_python_with_locator, run_request, stage_attachments,
};
use crate::session::PySession;

const DEFAULT_TIMEOUT_SECONDS: f64 = 30.0;
pub(crate) const DEFAULT_MAX_CYCLES: u64 = 16;
const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

/// Rust-owned resolved agent handle.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Agent", frozen)]
pub(crate) struct PyAgent {
    pub(crate) inner: Arc<Agent>,
    pub(crate) model: ModelName,
    pub(crate) output_adapter: Option<Py<PyAny>>,
    pub(crate) settings: ModelSettings,
    pub(crate) default_timeout_seconds: f64,
    /// Shared with the registered `DocumentToolset` and
    /// `DocumentIngestMiddleware`. Run attachments are staged here before
    /// submission so the middleware can resolve them back off the
    /// `AttachmentIndex`.
    pub(crate) artifact_store: Arc<InProcessArtifactStore>,
    pub(crate) attachment_index: Arc<AttachmentIndex>,
}

#[pymethods]
impl PyAgent {
    /// Construct a Rust-backed official `OpenAI` Responses agent.
    ///
    /// `api_key` is required and keyword-only. The factory always targets
    /// `https://api.openai.com/v1/responses` and does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, reasoning_effort = None, reasoning_summary = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth, reasoning, and primary port components distinctly"
    )]
    fn openai(
        py: Python<'_>,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        api_key: String,
        reasoning_effort: Option<String>,
        reasoning_summary: Option<String>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
        middleware: Option<Vec<Py<PyPythonMiddleware>>>,
        observers: Option<Vec<Py<PyPythonObserver>>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let (ports, artifact_store, attachment_index) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::openai(OpenAiAgentSpec {
                model,
                api_key,
                reasoning_effort,
                reasoning_summary,
                common: LinkedCommon {
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                },
            })
            .await;
            Python::attach(|py| {
                wrap_linked_agent(py, built, output_adapter, artifact_store, attachment_index)
            })
        })
    }

    /// Construct a Rust-backed `OpenRouter` Responses agent.
    ///
    /// `api_key` is required and keyword-only. The factory always targets
    /// `https://openrouter.ai/api/v1/responses` and does not read environment
    /// variables. `referer` and `title` set the non-secret attribution
    /// headers.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, referer = None, title = None, reasoning_effort = None, reasoning_summary = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth, attribution, reasoning, and primary port components distinctly"
    )]
    fn openrouter(
        py: Python<'_>,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        api_key: String,
        referer: Option<String>,
        title: Option<String>,
        reasoning_effort: Option<String>,
        reasoning_summary: Option<String>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
        middleware: Option<Vec<Py<PyPythonMiddleware>>>,
        observers: Option<Vec<Py<PyPythonObserver>>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let (ports, artifact_store, attachment_index) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::openrouter(OpenRouterAgentSpec {
                model,
                api_key,
                referer,
                title,
                reasoning_effort,
                reasoning_summary,
                common: LinkedCommon {
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                },
            })
            .await;
            Python::attach(|py| {
                wrap_linked_agent(py, built, output_adapter, artifact_store, attachment_index)
            })
        })
    }

    /// Construct a Rust-backed Anthropic Messages agent.
    ///
    /// `api_key` stays positional. Python port lists are keyword-only. HTTPS is
    /// required when `api_key` is set; the binding does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None, *, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth and primary port components distinctly"
    )]
    fn anthropic(
        py: Python<'_>,
        base_url: String,
        model: String,
        api_key: Option<String>,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
        middleware: Option<Vec<Py<PyPythonMiddleware>>>,
        observers: Option<Vec<Py<PyPythonObserver>>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let (ports, artifact_store, attachment_index) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::anthropic(AnthropicAgentSpec {
                base_url,
                model,
                api_key,
                common: LinkedCommon {
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                },
            })
            .await;
            Python::attach(|py| {
                wrap_linked_agent(py, built, output_adapter, artifact_store, attachment_index)
            })
        })
    }

    /// Construct a keyless Rust-backed Ollama/local agent.
    ///
    /// Python port lists are keyword-only. This factory does not accept an
    /// API key.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None, *, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards primary port components distinctly"
    )]
    fn ollama(
        py: Python<'_>,
        base_url: String,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
        middleware: Option<Vec<Py<PyPythonMiddleware>>>,
        observers: Option<Vec<Py<PyPythonObserver>>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let (ports, artifact_store, attachment_index) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::ollama(OllamaAgentSpec {
                base_url,
                model,
                common: LinkedCommon {
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                },
            })
            .await;
            Python::attach(|py| {
                wrap_linked_agent(py, built, output_adapter, artifact_store, attachment_index)
            })
        })
    }

    /// Construct a Rust-backed agent that dispatches to a dedicated provider.
    ///
    /// `hard_input_bytes`, `wire_protocol`, and `credential_name` are
    /// required. `openai_chat` is a configuration error. The binding does
    /// not read environment variables.
    #[staticmethod]
    #[pyo3(signature = (endpoint, model, instruction = None, capabilities = None, active_capabilities = None, *, wire_protocol, credential_name, hard_input_bytes = None, auth = None, api_key = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards gateway route, auth, and primary port components distinctly"
    )]
    fn gateway(
        py: Python<'_>,
        endpoint: String,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        wire_protocol: String,
        credential_name: String,
        hard_input_bytes: Option<u64>,
        auth: Option<String>,
        api_key: Option<String>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
        middleware: Option<Vec<Py<PyPythonMiddleware>>>,
        observers: Option<Vec<Py<PyPythonObserver>>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let (ports, artifact_store, attachment_index) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::gateway(GatewayAgentSpec {
                endpoint,
                model,
                wire_protocol,
                credential_name,
                hard_input_bytes,
                auth_kind: auth,
                api_key,
                common: LinkedCommon {
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                },
            })
            .await;
            Python::attach(|py| {
                wrap_linked_agent(py, built, output_adapter, artifact_store, attachment_index)
            })
        })
    }

    /// Construct a Rust-backed T4 E2B sandbox agent.
    ///
    /// `api_key` is required and keyword-only. The binding does not read
    /// environment variables.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, endpoint = None, template = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards e2b route and primary port components distinctly"
    )]
    fn e2b_sandbox(
        py: Python<'_>,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        api_key: String,
        endpoint: Option<String>,
        template: Option<String>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
        middleware: Option<Vec<Py<PyPythonMiddleware>>>,
        observers: Option<Vec<Py<PyPythonObserver>>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let (ports, artifact_store, attachment_index) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::e2b_sandbox(E2bSandboxAgentSpec {
                model,
                api_key,
                endpoint,
                template,
                common: LinkedCommon {
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                },
            })
            .await;
            Python::attach(|py| {
                wrap_linked_agent(py, built, output_adapter, artifact_store, attachment_index)
            })
        })
    }

    /// Construct an agent from trusted coarse Python model and Toolset callbacks.
    #[staticmethod]
    #[pyo3(signature = (model, toolsets = None, instruction = None, output_type = None, capabilities = None, active_capabilities = None, context_providers = None, middleware = None, observers = None, *, child_runs = None, sqlite_path = None, sqlite_durability = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "Python callback factory forwards all primary port components distinctly"
    )]
    fn from_python<'py>(
        py: Python<'py>,
        model: &Bound<'py, PyPythonModel>,
        toolsets: Option<Vec<Py<PyPythonToolset>>>,
        instruction: Option<String>,
        output_type: Option<Py<PyAny>>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
        middleware: Option<Vec<Py<PyPythonMiddleware>>>,
        observers: Option<Vec<Py<PyPythonObserver>>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = model.borrow();
        let model_name = model.model_name();
        let model = model.registration();
        let (ports, artifact_store, attachment_index) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let child_runs = child_runs_or_deny(py, child_runs);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_python_agent(
                model_name,
                model,
                instruction,
                ports,
                capabilities,
                active_capabilities,
                child_runs,
                (sqlite_path, sqlite_durability),
                artifact_store,
                attachment_index,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Return the bounded model-activated catalog in stable identity order.
    fn capability_catalog(&self, py: Python<'_>) -> PyResult<Vec<Py<PyDict>>> {
        self.inner
            .capability_catalog()
            .into_iter()
            .map(|entry| {
                let value = PyDict::new(py);
                value.set_item("id", entry.id().as_str())?;
                value.set_item("description", entry.description())?;
                Ok(value.unbind())
            })
            .collect()
    }

    /// Render the compact model-facing catalog without activating a capability.
    fn compact_capability_catalog(&self) -> String {
        self.inner.compact_capability_catalog()
    }

    /// Compose a new agent from reconstructed catalogs.
    ///
    /// In-flight runs keep the previous lock.
    fn re_resolve<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let agent = Arc::clone(&self.inner);
        let model = self.model.clone();
        let settings = self.settings.clone();
        let default_timeout_seconds = self.default_timeout_seconds;
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        let artifact_store = Arc::clone(&self.artifact_store);
        let attachment_index = Arc::clone(&self.attachment_index);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match agent.re_resolve().await {
                Ok(inner) => Python::attach(|py| {
                    Py::new(
                        py,
                        PyAgent {
                            inner: Arc::new(inner),
                            model,
                            output_adapter,
                            settings,
                            default_timeout_seconds,
                            artifact_store,
                            attachment_index,
                        },
                    )
                }),
                Err(error) => Python::attach(|py| Err(agent_error(py, &error, None))),
            }
        })
    }

    /// Create a live session on this agent's journal store.
    #[pyo3(signature = (tenant_scope = "default"))]
    fn create_session<'py>(
        &self,
        py: Python<'py>,
        tenant_scope: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let store = self.inner.journal_store();
        let tenant_scope = tenant_scope.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            match Session::create(store, tenant_scope).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PySession { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Open an existing session without respawning parked runs.
    fn open_session<'py>(
        &self,
        py: Python<'py>,
        session_id: String,
        tenant_scope: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let store = self.inner.journal_store();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let session_id = finstack_ai_kernel::SessionId::parse(&session_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match Session::open(store, session_id, tenant_scope).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PySession { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Start a run and return its shared control handle immediately.
    #[pyo3(signature = (input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None, attachments = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "run start forwards the same bounded run inputs plus staged attachments"
    )]
    fn start(
        &self,
        py: Python<'_>,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
        attachments: Option<Vec<Py<PyAttachment>>>,
    ) -> PyResult<PyRun> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        let settings = self.settings.clone();
        let timeout_seconds = timeout_seconds.unwrap_or(self.default_timeout_seconds);
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        let artifact_store = Arc::clone(&self.artifact_store);
        let attachment_index = Arc::clone(&self.attachment_index);
        let attachments = collect_attachments(py, attachments);
        py.detach(move || {
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            let staged = runtime.block_on(stage_attachments(
                artifact_store.as_ref(),
                attachment_index.as_ref(),
                "python-local",
                attachments,
            ))?;
            let request = run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                settings,
                "python-local",
                staged,
            )?;
            agent.start(request).map(|inner| PyRun {
                inner,
                output_adapter,
            })
        })
        .map_err(|error| agent_error(py, &error, None))
    }

    /// Execute one run and await its committed result.
    #[pyo3(signature = (input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None, attachments = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "run forwards the same bounded run inputs plus staged attachments"
    )]
    fn run<'py>(
        &self,
        py: Python<'py>,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
        attachments: Option<Vec<Py<PyAttachment>>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        let settings = self.settings.clone();
        let timeout_seconds = timeout_seconds.unwrap_or(self.default_timeout_seconds);
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        let artifact_store = Arc::clone(&self.artifact_store);
        let attachment_index = Arc::clone(&self.attachment_index);
        let attachments = collect_attachments(py, attachments);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let staged = match stage_attachments(
                artifact_store.as_ref(),
                attachment_index.as_ref(),
                "python-local",
                attachments,
            )
            .await
            {
                Ok(staged) => staged,
                Err(error) => return Python::attach(|py| Err(agent_error(py, &error, None))),
            };
            let request = match run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                settings,
                "python-local",
                staged,
            ) {
                Ok(request) => request,
                Err(error) => return Python::attach(|py| Err(agent_error(py, &error, None))),
            };
            let run = match agent.start(request) {
                Ok(run) => run,
                Err(error) => return Python::attach(|py| Err(agent_error(py, &error, None))),
            };
            let locator = run.locator().clone();
            let result = run.result().await;
            Python::attach(|py| {
                result_to_python_with_locator(py, result, Some(&locator), output_adapter)
            })
        })
    }
}

impl PyAgent {
    pub(crate) fn clone_inner(&self) -> Arc<Agent> {
        Arc::clone(&self.inner)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "lane start forwards the same bounded run inputs plus staged attachments"
    )]
    pub(crate) fn start_on_lane(
        &self,
        py: Python<'_>,
        lane: &finstack_ai::Lane,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
        attachments: Option<Vec<Py<PyAttachment>>>,
    ) -> PyResult<PyRun> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        let settings = self.settings.clone();
        let timeout_seconds = timeout_seconds.unwrap_or(self.default_timeout_seconds);
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        let artifact_store = Arc::clone(&self.artifact_store);
        let attachment_index = Arc::clone(&self.attachment_index);
        let attachments = collect_attachments(py, attachments);
        let lane = lane.clone();
        let tenant_scope = lane.session().tenant_scope().to_string();
        py.detach(move || {
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            let staged = runtime.block_on(stage_attachments(
                artifact_store.as_ref(),
                attachment_index.as_ref(),
                &tenant_scope,
                attachments,
            ))?;
            let request = run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                settings,
                &tenant_scope,
                staged,
            )?;
            lane.run(&agent, request).map(|inner| PyRun {
                inner,
                output_adapter,
            })
        })
        .map_err(|error| agent_error(py, &error, None))
    }
}

fn split_linked_ports(ports: LinkedPorts) -> (LinkedAgentPorts, Option<Py<PyAny>>) {
    (
        LinkedAgentPorts {
            toolsets: ports.toolsets,
            context_providers: ports.context_providers,
            middleware: ports.middleware,
            observers: ports.observers,
            output_schema: ports.output.as_ref().map(|output| output.schema.clone()),
        },
        ports.output.map(|output| output.adapter),
    )
}

fn wrap_linked_agent(
    py: Python<'_>,
    built: Result<LinkedAgent, AgentRunError>,
    output_adapter: Option<Py<PyAny>>,
    artifact_store: Arc<InProcessArtifactStore>,
    attachment_index: Arc<AttachmentIndex>,
) -> PyResult<Py<PyAgent>> {
    match built {
        Ok(value) => Py::new(
            py,
            PyAgent {
                inner: Arc::new(value.agent),
                model: value.model,
                output_adapter,
                settings: value.settings,
                default_timeout_seconds: value.default_timeout.as_secs_f64(),
                artifact_store,
                attachment_index,
            },
        ),
        Err(error) => Err(agent_error(py, &error, None)),
    }
}

struct LinkedPorts {
    toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Observer>)>,
    output: Option<PreparedPydanticOutput>,
}

/// Build a fresh, dedicated `InProcessArtifactStore` + `AttachmentIndex`
/// pair plus the `DocumentToolset`/`DocumentIngestMiddleware` registrations
/// that share them.
///
/// Every agent factory registers these unconditionally (mirroring the
/// document-ingest lane test's wiring) so `Agent.run`/`start` can stage
/// `Attachment` inputs against the exact store/index instances the
/// toolset and middleware read from.
/// `(artifact_store, attachment_index, toolset registration, middleware registration)`.
type DocumentIngestPorts = (
    Arc<InProcessArtifactStore>,
    Arc<AttachmentIndex>,
    (ComponentRef, Arc<dyn Toolset>),
    (ComponentRef, Arc<dyn Middleware>),
);

/// `DocumentIngestMiddleware`'s checked-in invocation version
/// (`INGEST_VERSION` in `finstack-ai-middleware-document-ingest::lib`).
/// `validate_middleware_descriptor` requires the registered `ComponentRef`
/// to match the handle's own reported `(component id, version)` exactly, so
/// this must track that crate's constant rather than the binding's generic
/// `PREVIEW_VERSION`. `DocumentToolset` has no equivalent version check, but
/// the same value is reused for its registration for consistency (mirrors
/// `crates/finstack-ai-test/tests/lanes/document_ingest.rs`).
const DOCUMENT_INGEST_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

fn document_ingest_component(id: &str) -> Result<ComponentRef, AgentRunError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(|error| configuration_error(error.to_string()))?,
        Some(DOCUMENT_INGEST_VERSION),
    ))
}

fn document_ingest_ports() -> Result<DocumentIngestPorts, AgentRunError> {
    let artifact_store = Arc::new(InProcessArtifactStore::default());
    let attachment_index = Arc::new(AttachmentIndex::default());
    let dyn_store: Arc<dyn ArtifactStore> = Arc::clone(&artifact_store) as Arc<dyn ArtifactStore>;
    let toolset = DocumentToolset::try_new()
        .map_err(|error| configuration_error(error.to_string()))?
        .with_artifact_store(Arc::clone(&dyn_store));
    let middleware =
        DocumentIngestMiddleware::try_new(Arc::clone(&dyn_store), Arc::clone(&attachment_index))
            .map_err(|error| configuration_error(error.to_string()))?;
    Ok((
        artifact_store,
        attachment_index,
        (
            document_ingest_component("finstack.tools.document")?,
            Arc::new(toolset) as Arc<dyn Toolset>,
        ),
        (
            document_ingest_component("finstack.middleware.document-ingest")?,
            Arc::new(middleware) as Arc<dyn Middleware>,
        ),
    ))
}

fn linked_ports(
    py: Python<'_>,
    toolsets: Option<Vec<Py<PyPythonToolset>>>,
    context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
    middleware: Option<Vec<Py<PyPythonMiddleware>>>,
    observers: Option<Vec<Py<PyPythonObserver>>>,
    output_type: Option<Py<PyAny>>,
) -> PyResult<(
    LinkedPorts,
    Arc<InProcessArtifactStore>,
    Arc<AttachmentIndex>,
)> {
    let (artifact_store, attachment_index, document_toolset, document_middleware) =
        document_ingest_ports().map_err(|error| agent_error(py, &error, None))?;
    let mut toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)> = toolsets
        .unwrap_or_default()
        .into_iter()
        .map(|toolset| toolset.bind(py).borrow().registration())
        .collect();
    toolsets.push(document_toolset);
    let mut middleware: Vec<(ComponentRef, Arc<dyn Middleware>)> = middleware
        .unwrap_or_default()
        .into_iter()
        .map(|middleware| middleware.bind(py).borrow().registration())
        .collect();
    middleware.push(document_middleware);
    Ok((
        LinkedPorts {
            toolsets,
            context_providers: context_providers
                .unwrap_or_default()
                .into_iter()
                .map(|provider| provider.bind(py).borrow().registration())
                .collect(),
            middleware,
            observers: observers
                .unwrap_or_default()
                .into_iter()
                .map(|observer| observer.bind(py).borrow().registration())
                .collect(),
            output: output_type
                .map(|target| prepare_pydantic_output(py, target))
                .transpose()?,
        },
        artifact_store,
        attachment_index,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "callback factory forwards ports, child-run policy, and sqlite store distinctly"
)]
async fn build_python_agent(
    model_name: ModelName,
    model: (ComponentRef, Arc<dyn Model>),
    instruction: Option<String>,
    ports: LinkedPorts,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    child_runs: ChildRunPolicy,
    sqlite: (Option<String>, Option<PySqliteDurability>),
    artifact_store: Arc<InProcessArtifactStore>,
    attachment_index: Arc<AttachmentIndex>,
) -> Result<PyAgent, AgentRunError> {
    let store_component = if sqlite.0.is_some() {
        "python.store.sqlite"
    } else {
        "python.store.memory"
    };
    let store = open_journal_store(sqlite.0, sqlite.1)?;
    let output = ports.output;
    let built = Agent::builder(
        AgentId::parse("python.agent.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse("python.bundle.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        model,
        (component(store_component)?, store),
    )
    .build_linked(
        LinkedCommon {
            instruction,
            capabilities,
            active_capabilities,
            ports: LinkedAgentPorts {
                toolsets: ports.toolsets,
                context_providers: ports.context_providers,
                middleware: ports.middleware,
                observers: ports.observers,
                output_schema: output.as_ref().map(|value| value.schema.clone()),
            },
            child_runs,
        },
        model_name,
        empty_model_settings()?,
        Duration::from_secs_f64(DEFAULT_TIMEOUT_SECONDS),
    )
    .await?;
    Ok(PyAgent {
        inner: Arc::new(built.agent),
        model: built.model,
        output_adapter: output.map(|value| value.adapter),
        settings: built.settings,
        default_timeout_seconds: built.default_timeout.as_secs_f64(),
        artifact_store,
        attachment_index,
    })
}

fn child_runs_or_deny(py: Python<'_>, child_runs: Option<Py<PyChildRunPolicy>>) -> ChildRunPolicy {
    child_runs.map_or(ChildRunPolicy::Deny, |policy| {
        policy.bind(py).borrow().to_rust()
    })
}

pub(crate) fn empty_model_settings() -> Result<ModelSettings, AgentRunError> {
    Ok(ModelSettings {
        values: RawJson::parse(b"{}").map_err(|error| configuration_error(error.to_string()))?,
    })
}

fn capability_configuration(
    py: Python<'_>,
    capabilities: Option<Vec<Py<PyCapability>>>,
    active_capabilities: Option<Vec<String>>,
) -> PyResult<(Vec<CapabilitySpec>, Vec<CapabilityId>)> {
    let capabilities = capabilities
        .unwrap_or_default()
        .into_iter()
        .map(|capability| capability.bind(py).borrow().inner.clone())
        .collect();
    let active_capabilities = active_capabilities
        .unwrap_or_default()
        .into_iter()
        .map(CapabilityId::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| PyTypeError::new_err(error.to_string()))?;
    Ok((capabilities, active_capabilities))
}

pub(crate) fn component(id: &str) -> Result<ComponentRef, AgentRunError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(|error| configuration_error(error.to_string()))?,
        Some(PREVIEW_VERSION),
    ))
}
