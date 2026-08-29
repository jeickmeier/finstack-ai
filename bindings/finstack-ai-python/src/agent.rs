//! Rust-owned resolved agent handle and linked-provider builders.

use std::sync::Arc;

use crate::approval_grant::PyApprovalGrantMode;
use crate::child_policy::PyChildRunPolicy;
use crate::store::{PySqliteDurability, open_journal_store};
use finstack_ai::runtime::artifact::{ArtifactStore, InProcessArtifactStore};
use finstack_ai::runtime::ports::middleware::Middleware;
use finstack_ai::runtime::ports::model::{Model, ModelName, ModelSettings};
use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai::{
    Agent, AgentRunError, AnthropicAgentSpec, ApprovalGrantMode, CapabilitySpec, ChildRunPolicy,
    DEFAULT_MAX_CYCLES, DEFAULT_RUN_TIMEOUT, GatewayAgentSpec, GeminiAgentSpec, HistoryCachePolicy,
    LinkedAgent, LinkedAgentPorts, LinkedCommon, LinkedProviderSpec, OllamaAgentSpec,
    OpenAiAgentSpec, OpenRouterAgentSpec, OpenRouterMediaToolsSpec, Session,
};
use finstack_ai_kernel::{
    AgentId, ArtifactRef, BundleId, CapabilityId, ComponentId, ComponentRef, RawJson, Sensitivity,
    SessionId, Version,
};
use finstack_ai_middleware_document_ingest::DocumentIngestMiddleware;
use finstack_ai_store_artifact::LocalArtifactStore;
use finstack_ai_tools_document::DocumentToolset;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::ConfigurationError;
use crate::callbacks::{
    PyPythonContextProvider, PyPythonMiddleware, PyPythonModel, PyPythonObserver, PyPythonToolset,
};
use crate::capability::PyCapability;
use crate::e2b::PyE2bSandboxToolset;
use crate::elicitation::PyElicitationToolset;
use crate::errors::{agent_error, configuration_error, session_py_error};
use crate::fetch::PyHttpFetchToolset;
use crate::memory::{PyMemoryContextProvider, PyMemoryObserver, PyMemoryToolset};
use crate::middleware::PyInstructionsMiddleware;
use crate::run::{
    PreparedPydanticOutput, PyAttachment, PyRun, collect_attachments, prepare_pydantic_output,
    result_to_python_with_locator, run_request, stage_attachments,
};
use crate::session::PySession;

/// Toolset argument accepted by every agent factory.
#[derive(FromPyObject)]
pub(crate) enum PyToolsetArg {
    /// Trusted Python callback toolset.
    Python(Py<PyPythonToolset>),
    /// Rust elicitation toolset.
    Elicitation(Py<PyElicitationToolset>),
    /// Rust memory toolset handle from `MemoryExtension.toolset()`.
    Memory(Py<PyMemoryToolset>),
    /// Rust bounded HTTP fetch toolset.
    HttpFetch(Py<PyHttpFetchToolset>),
    /// Rust E2B sandbox toolset.
    E2b(Py<PyE2bSandboxToolset>),
}

impl PyToolsetArg {
    /// `artifact_store` is the agent's own store; only the memory toolset
    /// consumes it (for oversized memory bodies staged as blobs).
    fn registration(
        &self,
        py: Python<'_>,
        artifact_store: &Arc<dyn ArtifactStore>,
    ) -> PyResult<(ComponentRef, Arc<dyn Toolset>)> {
        match self {
            Self::Python(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::Elicitation(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::Memory(toolset) => toolset
                .bind(py)
                .borrow()
                .registration(py, Arc::clone(artifact_store)),
            Self::HttpFetch(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::E2b(toolset) => Ok(toolset.bind(py).borrow().registration()),
        }
    }
}

/// Context-provider argument accepted by every agent factory.
#[derive(FromPyObject)]
pub(crate) enum PyContextProviderArg {
    /// Trusted Python callback context provider.
    Python(Py<PyPythonContextProvider>),
    /// Rust memory recall provider from `MemoryExtension.context_provider()`.
    Memory(Py<PyMemoryContextProvider>),
}

impl PyContextProviderArg {
    fn registration(
        &self,
        py: Python<'_>,
        artifact_store: &Arc<dyn ArtifactStore>,
    ) -> PyResult<(
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::context::ContextProvider>,
    )> {
        match self {
            Self::Python(provider) => Ok(provider.bind(py).borrow().registration()),
            Self::Memory(provider) => provider.bind(py).borrow().registration(py, artifact_store),
        }
    }
}

/// Middleware argument accepted by every agent factory.
#[derive(FromPyObject)]
pub(crate) enum PyMiddlewareArg {
    /// Trusted Python callback middleware.
    Python(Py<PyPythonMiddleware>),
    /// Rust frozen policy-instructions middleware.
    Instructions(Py<PyInstructionsMiddleware>),
}

impl PyMiddlewareArg {
    fn registration(&self, py: Python<'_>) -> (ComponentRef, Arc<dyn Middleware>) {
        match self {
            Self::Python(middleware) => middleware.bind(py).borrow().registration(),
            Self::Instructions(middleware) => middleware.bind(py).borrow().registration(),
        }
    }
}

/// Observer argument accepted by every agent factory.
#[derive(FromPyObject)]
pub(crate) enum PyObserverArg {
    /// Trusted Python callback observer.
    Python(Py<PyPythonObserver>),
    /// Rust memory capture observer from `MemoryExtension.observer()`.
    Memory(Py<PyMemoryObserver>),
}

impl PyObserverArg {
    fn registration(
        &self,
        py: Python<'_>,
    ) -> (
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::observer::Observer>,
    ) {
        match self {
            Self::Python(observer) => observer.bind(py).borrow().registration(),
            Self::Memory(observer) => observer.bind(py).borrow().registration(),
        }
    }
}

const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

/// Bounded process-local history checkpoint cache policy.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "HistoryCachePolicy",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Copy)]
pub(crate) struct PyHistoryCachePolicy {
    inner: HistoryCachePolicy,
}

#[pymethods]
impl PyHistoryCachePolicy {
    #[new]
    #[pyo3(signature = (max_entries = HistoryCachePolicy::DEFAULT_MAX_ENTRIES, max_bytes = HistoryCachePolicy::DEFAULT_MAX_BYTES))]
    #[pyo3(text_signature = "(max_entries=64, max_bytes=16777216)")]
    fn new(max_entries: usize, max_bytes: usize) -> PyResult<Self> {
        HistoryCachePolicy::try_new(max_entries, max_bytes)
            .map(|inner| Self { inner })
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }

    /// Return a policy that disables checkpoint reuse.
    #[staticmethod]
    fn disabled() -> Self {
        Self {
            inner: HistoryCachePolicy::disabled(),
        }
    }

    /// Maximum number of retained checkpoints.
    #[getter]
    fn max_entries(&self) -> usize {
        self.inner.max_entries()
    }

    /// Maximum aggregate bytes retained by the cache.
    #[getter]
    fn max_bytes(&self) -> usize {
        self.inner.max_bytes()
    }
}

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
    /// artifact store. In-process by default; `artifact_path=` swaps in a
    /// filesystem-backed `LocalArtifactStore` so persisted sessions can
    /// re-resolve attachments from a fresh process.
    pub(crate) artifact_store: Arc<dyn ArtifactStore>,
}

#[pymethods]
impl PyAgent {
    /// Compose an agent with a fresh bounded process-local history cache.
    fn with_history_cache(&self, py: Python<'_>, policy: &Bound<'_, PyHistoryCachePolicy>) -> Self {
        let policy = policy.borrow();
        Self {
            inner: Arc::new(
                self.inner
                    .as_ref()
                    .clone()
                    .with_history_cache_policy(policy.inner),
            ),
            model: self.model.clone(),
            output_adapter: self
                .output_adapter
                .as_ref()
                .map(|adapter| adapter.clone_ref(py)),
            settings: self.settings.clone(),
            default_timeout_seconds: self.default_timeout_seconds,
            artifact_store: Arc::clone(&self.artifact_store),
        }
    }

    /// Construct a Rust-backed official `OpenAI` Responses agent.
    ///
    /// `api_key` is required and keyword-only. The factory always targets
    /// `https://api.openai.com/v1/responses` and does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, reasoning_effort = None, reasoning_summary = None, media_tools = false, openrouter_media_api_key = None, openrouter_media_referer = None, openrouter_media_title = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth, reasoning, media toolsets, and primary port components distinctly"
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
        media_tools: bool,
        openrouter_media_api_key: Option<String>,
        openrouter_media_referer: Option<String>,
        openrouter_media_title: Option<String>,
        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let openrouter_media = openrouter_media_spec(
            openrouter_media_api_key,
            openrouter_media_referer,
            openrouter_media_title,
        )?;
        let (ports, artifact_store) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        let approval_grant = approval_grant_or_per_call(py, approval_grant);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::linked(LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
                model,
                api_key,
                reasoning_effort,
                reasoning_summary,
                media_tools,
                common: LinkedCommon {
                    openrouter_media,
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                    approval_grant,
                },
            }))
            .await;
            Python::attach(|py| wrap_linked_agent(py, built, output_adapter, artifact_store))
        })
    }

    /// Construct a Rust-backed `OpenRouter` Responses agent.
    ///
    /// `api_key` is required and keyword-only. The factory always targets
    /// `https://openrouter.ai/api/v1/responses` and does not read environment
    /// variables. `referer` and `title` set the non-secret attribution
    /// headers. Does not attach a `MediaResolver`; vision, file, and audio
    /// input require a host-built provider because Python factories do not
    /// accept host callback resolvers across FFI.
    /// `media_tools` registers outbound media-generation tools only.
    /// `openrouter_media_*` registers the same toolset from an explicit key
    /// and cannot be combined with `media_tools`.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, referer = None, title = None, reasoning_effort = None, reasoning_summary = None, media_tools = false, openrouter_media_api_key = None, openrouter_media_referer = None, openrouter_media_title = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth, attribution, reasoning, media toolset, and primary port components distinctly"
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
        media_tools: bool,
        openrouter_media_api_key: Option<String>,
        openrouter_media_referer: Option<String>,
        openrouter_media_title: Option<String>,
        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let openrouter_media = openrouter_media_spec(
            openrouter_media_api_key,
            openrouter_media_referer,
            openrouter_media_title,
        )?;
        let (ports, artifact_store) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        let approval_grant = approval_grant_or_per_call(py, approval_grant);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::linked(LinkedProviderSpec::OpenRouter(OpenRouterAgentSpec {
                model,
                api_key,
                referer,
                title,
                reasoning_effort,
                reasoning_summary,
                media_tools,
                common: LinkedCommon {
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                    approval_grant,
                    openrouter_media,
                },
            }))
            .await;
            Python::attach(|py| wrap_linked_agent(py, built, output_adapter, artifact_store))
        })
    }

    /// Construct a Rust-backed Anthropic Messages agent.
    ///
    /// `api_key` stays positional. Python port lists are keyword-only. HTTPS is
    /// required when `api_key` is set; the binding does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None, *, openrouter_media_api_key = None, openrouter_media_referer = None, openrouter_media_title = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth, media toolset, and primary port components distinctly"
    )]
    fn anthropic(
        py: Python<'_>,
        base_url: String,
        model: String,
        api_key: Option<String>,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        openrouter_media_api_key: Option<String>,
        openrouter_media_referer: Option<String>,
        openrouter_media_title: Option<String>,
        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let openrouter_media = openrouter_media_spec(
            openrouter_media_api_key,
            openrouter_media_referer,
            openrouter_media_title,
        )?;
        let (ports, artifact_store) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        let approval_grant = approval_grant_or_per_call(py, approval_grant);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::linked(LinkedProviderSpec::Anthropic(AnthropicAgentSpec {
                base_url,
                model,
                api_key,
                common: LinkedCommon {
                    openrouter_media,
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                    approval_grant,
                },
            }))
            .await;
            Python::attach(|py| wrap_linked_agent(py, built, output_adapter, artifact_store))
        })
    }

    /// Construct a Rust-backed Gemini `generateContent` agent.
    ///
    /// `api_key` stays positional. Python port lists are keyword-only. HTTPS is
    /// required when `api_key` is set; the binding does not read environment
    /// variables. Does not hardcode the Google host: `endpoint` is passed
    /// straight into the provider's `GeminiConfig::try_new`.
    #[staticmethod]
    #[pyo3(signature = (endpoint, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None, *, openrouter_media_api_key = None, openrouter_media_referer = None, openrouter_media_title = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth, media toolset, and primary port components distinctly"
    )]
    fn gemini(
        py: Python<'_>,
        endpoint: String,
        model: String,
        api_key: Option<String>,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        openrouter_media_api_key: Option<String>,
        openrouter_media_referer: Option<String>,
        openrouter_media_title: Option<String>,
        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let openrouter_media = openrouter_media_spec(
            openrouter_media_api_key,
            openrouter_media_referer,
            openrouter_media_title,
        )?;
        let (ports, artifact_store) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        let approval_grant = approval_grant_or_per_call(py, approval_grant);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::linked(LinkedProviderSpec::Gemini(GeminiAgentSpec {
                endpoint,
                model,
                api_key,
                common: LinkedCommon {
                    openrouter_media,
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                    approval_grant,
                },
            }))
            .await;
            Python::attach(|py| wrap_linked_agent(py, built, output_adapter, artifact_store))
        })
    }

    /// Construct a keyless Rust-backed Ollama/local agent.
    ///
    /// Python port lists are keyword-only. This factory does not accept an
    /// API key.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None, *, openrouter_media_api_key = None, openrouter_media_referer = None, openrouter_media_title = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards media toolset and primary port components distinctly"
    )]
    fn ollama(
        py: Python<'_>,
        base_url: String,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        openrouter_media_api_key: Option<String>,
        openrouter_media_referer: Option<String>,
        openrouter_media_title: Option<String>,
        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let openrouter_media = openrouter_media_spec(
            openrouter_media_api_key,
            openrouter_media_referer,
            openrouter_media_title,
        )?;
        let (ports, artifact_store) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        let approval_grant = approval_grant_or_per_call(py, approval_grant);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::linked(LinkedProviderSpec::Ollama(OllamaAgentSpec {
                base_url,
                model,
                common: LinkedCommon {
                    openrouter_media,
                    instruction,
                    capabilities,
                    active_capabilities,
                    ports,
                    child_runs,
                    approval_grant,
                },
            }))
            .await;
            Python::attach(|py| wrap_linked_agent(py, built, output_adapter, artifact_store))
        })
    }

    /// Construct a Rust-backed agent that dispatches to a dedicated provider.
    ///
    /// `hard_input_bytes`, `wire_protocol`, and `credential_name` are
    /// required. `openai_chat` is a configuration error. The binding does
    /// not read environment variables.
    #[staticmethod]
    #[pyo3(signature = (endpoint, model, instruction = None, capabilities = None, active_capabilities = None, *, wire_protocol, credential_name, hard_input_bytes = None, auth = None, api_key = None, openrouter_media_api_key = None, openrouter_media_referer = None, openrouter_media_title = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards gateway route, auth, media toolset, and primary port components distinctly"
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
        openrouter_media_api_key: Option<String>,
        openrouter_media_referer: Option<String>,
        openrouter_media_title: Option<String>,
        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let openrouter_media = openrouter_media_spec(
            openrouter_media_api_key,
            openrouter_media_referer,
            openrouter_media_title,
        )?;
        let (ports, artifact_store) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
        )?;
        let child_runs = child_runs_or_deny(py, child_runs);
        let approval_grant = approval_grant_or_per_call(py, approval_grant);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let (ports, output_adapter) = split_linked_ports(ports);
            let built = Agent::linked(LinkedProviderSpec::Gateway(GatewayAgentSpec {
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
                    approval_grant,
                    openrouter_media,
                },
            }))
            .await;
            Python::attach(|py| wrap_linked_agent(py, built, output_adapter, artifact_store))
        })
    }

    /// Construct an agent from trusted coarse Python model and Toolset callbacks.
    #[staticmethod]
    #[pyo3(signature = (model, toolsets = None, instruction = None, output_type = None, capabilities = None, active_capabilities = None, context_providers = None, middleware = None, observers = None, *, child_runs = None, approval_grant = None, sqlite_path = None, sqlite_durability = None, artifact_path = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "Python callback factory forwards all primary port components distinctly"
    )]
    fn from_python<'py>(
        py: Python<'py>,
        model: &Bound<'py, PyPythonModel>,
        toolsets: Option<Vec<PyToolsetArg>>,
        instruction: Option<String>,
        output_type: Option<Py<PyAny>>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
        artifact_path: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = model.borrow();
        let model_name = model.model_name();
        let model = model.registration();
        let (ports, artifact_store) = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
        )?;
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let child_runs = child_runs_or_deny(py, child_runs);
        let approval_grant = approval_grant_or_per_call(py, approval_grant);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_python_agent(
                model_name,
                model,
                instruction,
                ports,
                capabilities,
                active_capabilities,
                child_runs,
                approval_grant,
                (sqlite_path, sqlite_durability),
                artifact_store,
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

    /// Replay one stored session into a provisional Rust-owned inspect snapshot.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for an invalid identity and a runtime
    /// error when the journal cannot be recovered.
    fn inspect_session<'py>(
        &self,
        py: Python<'py>,
        session_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let agent = Arc::clone(&self.inner);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let session_id = SessionId::parse(&session_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            let snapshot = agent
                .inspect_session(session_id)
                .await
                .map_err(|error| Python::attach(|py| session_py_error(py, &error)))?;
            Python::attach(|py| {
                let value = PyDict::new(py);
                value.set_item("session_id", snapshot.session_id.to_string())?;
                value.set_item("head_sequence", snapshot.head_sequence)?;
                value.set_item("phase", snapshot.phase.as_str())?;
                if let Some(result_text) = snapshot.result_text {
                    value.set_item("result_text", result_text)?;
                }
                if let Some(last_record_kind) = snapshot.last_record_kind {
                    value.set_item("last_record_kind", last_record_kind)?;
                }
                Ok(value.unbind())
            })
        })
    }

    /// Start a run and return its shared control handle immediately.
    #[pyo3(signature = (input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None, attachments = None))]
    #[pyo3(
        text_signature = "($self, input, *, timeout_seconds=None, max_cycles=16, max_output_retries=1, capability=None, attachments=None)"
    )]
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
        let attachments = collect_attachments(py, attachments);
        py.detach(move || {
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            let staged = runtime.block_on(stage_attachments(
                artifact_store.as_ref(),
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

    /// Read back the bytes behind an artifact reference a tool returned.
    ///
    /// Toolsets that produce binary output stage it and return a reference
    /// rather than inlining base64 the model cannot read, so generated
    /// images and audio arrive as the `artifact` field of a tool result.
    /// This resolves one of those references against the agent's own store.
    #[pyo3(signature = (artifact,))]
    fn read_artifact<'py>(
        &self,
        py: Python<'py>,
        artifact: Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let json = py
            .import("json")?
            .call_method1("dumps", (artifact,))?
            .extract::<String>()?;
        let reference: ArtifactRef = serde_json::from_str(&json).map_err(|error| {
            PyValueError::new_err(format!("invalid artifact reference: {error}"))
        })?;
        let store = Arc::clone(&self.artifact_store);
        let bytes = py.detach(move || {
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            runtime.block_on(store.get(
                finstack_ai::runtime::artifact::ArtifactScope {
                    tenant_scope: Arc::from("python-local"),
                    session_id: SessionId::from_bytes([0_u8; 16]),
                    run_id: None,
                    sensitivity: Sensitivity::Internal,
                },
                reference,
            ))
        });
        let bytes = bytes
            .map_err(|error| PyValueError::new_err(format!("artifact read failed: {error}")))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Execute one run and await its committed result.
    #[pyo3(signature = (input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None, attachments = None))]
    #[pyo3(
        text_signature = "($self, input, *, timeout_seconds=None, max_cycles=16, max_output_retries=1, capability=None, attachments=None)"
    )]
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
        let attachments = collect_attachments(py, attachments);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let staged =
                match stage_attachments(artifact_store.as_ref(), "python-local", attachments).await
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
        let attachments = collect_attachments(py, attachments);
        let lane = lane.clone();
        let tenant_scope = lane.session().tenant_scope().to_string();
        py.detach(move || {
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            let staged = runtime.block_on(stage_attachments(
                artifact_store.as_ref(),
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
            artifact_store: ports.artifact_store,
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
    artifact_store: Arc<dyn ArtifactStore>,
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
            },
        ),
        Err(error) => Err(agent_error(py, &error, None)),
    }
}

struct LinkedPorts {
    toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    context_providers: Vec<(
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::context::ContextProvider>,
    )>,
    middleware: Vec<(ComponentRef, Arc<dyn Middleware>)>,
    observers: Vec<(
        ComponentRef,
        Arc<dyn finstack_ai::runtime::ports::observer::Observer>,
    )>,
    output: Option<PreparedPydanticOutput>,
}

/// Build the shared artifact store plus the
/// `DocumentToolset`/`DocumentIngestMiddleware` registrations that share it.
///
/// Every agent factory registers these unconditionally (mirroring the
/// document-ingest lane test's wiring) so `Agent.run`/`start` can stage
/// `Attachment` inputs against the exact store instance the toolset and
/// middleware read from. Without `artifact_path` the store is a fresh
/// process-local `InProcessArtifactStore`; with it, a filesystem-backed
/// `LocalArtifactStore` at that directory, so sessions persisted via
/// `sqlite_path=` can re-resolve historical attachments from a new process
/// (the ingest middleware re-reads attachment bytes on every later turn).
/// `(artifact_store, toolset registration, middleware registration)`.
type DocumentIngestPorts = (
    Arc<dyn ArtifactStore>,
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

fn document_ingest_ports(
    artifact_path: Option<String>,
) -> Result<DocumentIngestPorts, AgentRunError> {
    let dyn_store: Arc<dyn ArtifactStore> = match artifact_path {
        Some(path) => Arc::new(
            LocalArtifactStore::try_new(std::path::PathBuf::from(path))
                .map_err(|error| configuration_error(error.to_string()))?,
        ),
        None => Arc::new(InProcessArtifactStore::default()),
    };
    let toolset = DocumentToolset::try_new(Arc::clone(&dyn_store))
        .map_err(|error| configuration_error(error.to_string()))?;
    let middleware = DocumentIngestMiddleware::try_new(Arc::clone(&dyn_store))
        .map_err(|error| configuration_error(error.to_string()))?;
    Ok((
        Arc::clone(&dyn_store),
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
    toolsets: Option<Vec<PyToolsetArg>>,
    context_providers: Option<Vec<PyContextProviderArg>>,
    middleware: Option<Vec<PyMiddlewareArg>>,
    observers: Option<Vec<PyObserverArg>>,
    output_type: Option<Py<PyAny>>,
    artifact_path: Option<String>,
) -> PyResult<(LinkedPorts, Arc<dyn ArtifactStore>)> {
    let (artifact_store, document_toolset, document_middleware) =
        document_ingest_ports(artifact_path).map_err(|error| agent_error(py, &error, None))?;
    let dyn_artifact_store: Arc<dyn ArtifactStore> = Arc::clone(&artifact_store);
    let mut toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)> = toolsets
        .unwrap_or_default()
        .into_iter()
        .map(|toolset| toolset.registration(py, &dyn_artifact_store))
        .collect::<PyResult<Vec<_>>>()?;
    toolsets.push(document_toolset);
    let mut middleware: Vec<(ComponentRef, Arc<dyn Middleware>)> = middleware
        .unwrap_or_default()
        .into_iter()
        .map(|middleware| middleware.registration(py))
        .collect();
    middleware.push(document_middleware);
    Ok((
        LinkedPorts {
            toolsets,
            artifact_store: Some(Arc::clone(&dyn_artifact_store)),
            context_providers: context_providers
                .unwrap_or_default()
                .into_iter()
                .map(|provider| provider.registration(py, &dyn_artifact_store))
                .collect::<PyResult<Vec<_>>>()?,
            middleware,
            observers: observers
                .unwrap_or_default()
                .into_iter()
                .map(|observer| observer.registration(py))
                .collect(),
            output: output_type
                .map(|target| prepare_pydantic_output(py, target))
                .transpose()?,
        },
        artifact_store,
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
    approval_grant: ApprovalGrantMode,
    sqlite: (Option<String>, Option<PySqliteDurability>),
    artifact_store: Arc<dyn ArtifactStore>,
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
                artifact_store: ports.artifact_store,
                context_providers: ports.context_providers,
                middleware: ports.middleware,
                observers: ports.observers,
                output_schema: output.as_ref().map(|value| value.schema.clone()),
            },
            child_runs,
            approval_grant,
            openrouter_media: None,
        },
        model_name,
        empty_model_settings()?,
        DEFAULT_RUN_TIMEOUT,
    )
    .await?;
    Ok(PyAgent {
        inner: Arc::new(built.agent),
        model: built.model,
        output_adapter: output.map(|value| value.adapter),
        settings: built.settings,
        default_timeout_seconds: built.default_timeout.as_secs_f64(),
        artifact_store,
    })
}

fn child_runs_or_deny(py: Python<'_>, child_runs: Option<Py<PyChildRunPolicy>>) -> ChildRunPolicy {
    child_runs.map_or(ChildRunPolicy::Deny, |policy| {
        policy.bind(py).borrow().to_rust()
    })
}

fn approval_grant_or_per_call(
    py: Python<'_>,
    approval_grant: Option<Py<PyApprovalGrantMode>>,
) -> ApprovalGrantMode {
    approval_grant.map_or(ApprovalGrantMode::PerCall, |mode| {
        mode.bind(py).borrow().to_rust()
    })
}

/// Build an optional `OpenRouter` media-toolset spec from the keyword-only
/// binding arguments. `api_key = None` with `referer`/`title` set is a
/// configuration error, since attribution without a credential is nonsensical.
fn openrouter_media_spec(
    api_key: Option<String>,
    referer: Option<String>,
    title: Option<String>,
) -> PyResult<Option<OpenRouterMediaToolsSpec>> {
    match api_key {
        Some(api_key) => Ok(Some(OpenRouterMediaToolsSpec {
            api_key,
            referer,
            title,
        })),
        None if referer.is_some() || title.is_some() => Err(PyValueError::new_err(
            "openrouter_media_referer/openrouter_media_title require openrouter_media_api_key",
        )),
        None => Ok(None),
    }
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
