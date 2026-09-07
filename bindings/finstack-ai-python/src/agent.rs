//! Rust-owned resolved agent handle and linked-provider builders.

use std::future::Future;
use std::sync::Arc;

use crate::approval_grant::PyApprovalGrantMode;
use crate::child_policy::PyChildRunPolicy;
use crate::store::{
    PyArtifactStore, PyArtifactStoreArg, PySqliteDurability, open_journal_registration,
};
use finstack_ai::runtime::artifact::{ArtifactStore, InProcessArtifactStore};
use finstack_ai::runtime::ports::context::ContextProvider;
use finstack_ai::runtime::ports::middleware::Middleware;
use finstack_ai::runtime::ports::model::{Model, ModelName, ModelSettings};
use finstack_ai::runtime::ports::observer::Observer;
use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai::{
    Agent, AgentRun, AgentRunError, AgentRunRequest, AnthropicAgentSpec, ApprovalGrantMode,
    CapabilitySpec, ChildRunPolicy, DEFAULT_MAX_CYCLES, DEFAULT_RUN_TIMEOUT, GatewayAgentSpec,
    GeminiAgentSpec, HistoryCachePolicy, LinkedAgent, LinkedAgentPorts, LinkedCommon,
    LinkedProviderSpec, OllamaAgentSpec, OpenAiAgentSpec, OpenRouterAgentSpec, Session,
};
use finstack_ai_kernel::{
    AgentId, ArtifactRef, BundleId, CapabilityId, CompactionAuthorization, ComponentId,
    ComponentRef, RawJson, Sensitivity, SessionId, Version,
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
use crate::errors::{agent_error, configuration_error, run_error, session_py_error};
use crate::fetch::PyHttpFetchToolset;
use crate::media::{
    MediaRegistrations, PyMediaPipelineToolset, PyOpenAiMediaToolset, PyOpenRouterMediaToolset,
    PyVideoComposeToolset,
};
use crate::memory::{PyMemoryContextProvider, PyMemoryObserver, PyMemoryToolset};
use crate::middleware::{
    PyCompactionMiddleware, PyInstructionsMiddleware, PyRedactionMiddleware,
    PyToolPolicyMiddleware, PyVerifyMiddleware,
};
use crate::observers::{
    PyBillingObserver, PyLogObserver, PyMetricsObserver, PyNotifyObserver, PyOtelObserver,
};
use crate::repository::PyRepositoryContextProvider;
use crate::run::{
    PyAttachment, PyRun, collect_attachments, prepare_pydantic_output, run_request, run_result,
    stage_attachments,
};
use crate::session::PySession;
use crate::skills::{PySkillsToolset, build_skills_ports};
use crate::toolsets::{PyCalculatorToolset, PyFileSystemToolset, PyMcpToolset, PyShellToolset};

/// Toolset argument accepted by every agent factory.
#[derive(FromPyObject)]
pub(crate) enum PyToolsetArg {
    /// Unified native search.
    Search(Py<crate::search::PySearchToolset>),
    /// Explicit document indexing effect.
    DocumentIndex(Py<crate::search::PyDocumentIndexToolset>),
    /// Explicit `OpenAI` media tools.
    OpenAiMedia(Py<PyOpenAiMediaToolset>),
    /// Explicit `OpenRouter` media tools.
    OpenRouterMedia(Py<PyOpenRouterMediaToolset>),
    /// Local composition tools.
    VideoCompose(Py<PyVideoComposeToolset>),
    /// `MoviePlan` orchestration.
    MediaPipeline(Py<PyMediaPipelineToolset>),
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
    /// Deferred native skills toolset (capability activation).
    Skills(Py<PySkillsToolset>),
    /// Rust deterministic calculator toolset.
    Calculator(Py<PyCalculatorToolset>),
    /// Rust capability-confined filesystem toolset (T2, not sandboxed).
    FileSystem(Py<PyFileSystemToolset>),
    /// Rust deny-by-default shell toolset (T2, not sandboxed by default).
    Shell(Py<PyShellToolset>),
    /// Rust MCP client toolset over an allowlisted stdio server.
    Mcp(Py<PyMcpToolset>),
}

impl PyToolsetArg {
    /// `artifact_store` is the agent's own store; only the memory toolset
    /// consumes it (for oversized memory bodies staged as blobs).
    fn registration(
        &self,
        py: Python<'_>,
        artifact_store: &Arc<dyn ArtifactStore>,
        media: &mut MediaRegistrations,
    ) -> PyResult<(ComponentRef, Arc<dyn Toolset>)> {
        match self {
            Self::Search(tools) => tools.bind(py).borrow().registration(),
            Self::DocumentIndex(tools) => tools.bind(py).borrow().registration(artifact_store),
            Self::OpenAiMedia(tools) => media.openai(&tools.bind(py).borrow()),
            Self::OpenRouterMedia(tools) => media.openrouter(&tools.bind(py).borrow()),
            Self::VideoCompose(tools) => media.compose(&tools.bind(py).borrow()),
            Self::MediaPipeline(tools) => media.pipeline(&tools.bind(py).borrow()),
            Self::Python(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::Elicitation(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::Memory(toolset) => toolset
                .bind(py)
                .borrow()
                .registration(Arc::clone(artifact_store)),
            Self::HttpFetch(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::E2b(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::Skills(_) => Err(PyValueError::new_err(
                "SkillsToolset is only supported in Agent.from_python's toolsets",
            )),
            Self::Calculator(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::FileSystem(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::Shell(toolset) => Ok(toolset.bind(py).borrow().registration()),
            Self::Mcp(toolset) => Ok(toolset.bind(py).borrow().registration()),
        }
    }
}

/// Context-provider argument accepted by every agent factory.
#[derive(FromPyObject)]
pub(crate) enum PyContextProviderArg {
    /// Global native recall.
    Search(Py<crate::search::PySearchContextProvider>),
    /// Trusted Python callback context provider.
    Python(Py<PyPythonContextProvider>),
    /// Rust memory recall provider from `MemoryExtension.context_provider()`.
    Memory(Py<PyMemoryContextProvider>),
    /// Rust repository-instructions provider.
    Repository(Py<PyRepositoryContextProvider>),
}

impl PyContextProviderArg {
    fn registration(
        &self,
        py: Python<'_>,
        artifact_store: &Arc<dyn ArtifactStore>,
    ) -> PyResult<(ComponentRef, Arc<dyn ContextProvider>)> {
        match self {
            Self::Search(provider) => Ok(provider.bind(py).borrow().registration()),
            Self::Python(provider) => Ok(provider.bind(py).borrow().registration()),
            Self::Memory(provider) => provider.bind(py).borrow().registration(artifact_store),
            Self::Repository(provider) => Ok(provider.bind(py).borrow().registration()),
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
    /// Rust deterministic context-compaction middleware.
    Compaction(Py<PyCompactionMiddleware>),
    /// Rust deterministic content-verification middleware.
    Verify(Py<PyVerifyMiddleware>),
    /// Rust fail-soft PII/secret redaction middleware.
    Redaction(Py<PyRedactionMiddleware>),
    /// Rust tool-policy filter middleware.
    ToolPolicy(Py<PyToolPolicyMiddleware>),
}

impl PyMiddlewareArg {
    fn registration(&self, py: Python<'_>) -> (ComponentRef, Arc<dyn Middleware>) {
        match self {
            Self::Python(middleware) => middleware.bind(py).borrow().registration(),
            Self::Instructions(middleware) => middleware.bind(py).borrow().registration(),
            Self::Compaction(middleware) => middleware.bind(py).borrow().registration(),
            Self::Verify(middleware) => middleware.bind(py).borrow().registration(),
            Self::Redaction(middleware) => middleware.bind(py).borrow().registration(),
            Self::ToolPolicy(middleware) => middleware.bind(py).borrow().registration(),
        }
    }
}

/// Observer argument accepted by every agent factory.
#[derive(FromPyObject)]
pub(crate) enum PyObserverArg {
    /// Committed journal indexing hints.
    JournalIndex(Py<crate::search::PyJournalIndexObserver>),
    /// Trusted Python callback observer.
    Python(Py<PyPythonObserver>),
    /// Rust memory capture observer from `MemoryExtension.observer()`.
    Memory(Py<PyMemoryObserver>),
    /// Rust structured NDJSON log observer.
    Log(Py<PyLogObserver>),
    /// Rust Prometheus metrics observer.
    Metrics(Py<PyMetricsObserver>),
    /// Rust OpenTelemetry span observer.
    Otel(Py<PyOtelObserver>),
    /// Rust bounded billing ledger observer.
    Billing(Py<PyBillingObserver>),
    /// Rust interaction lifecycle notify observer.
    Notify(Py<PyNotifyObserver>),
}

impl PyObserverArg {
    fn registration(&self, py: Python<'_>) -> (ComponentRef, Arc<dyn Observer>) {
        match self {
            Self::JournalIndex(observer) => observer.bind(py).borrow().registration(),
            Self::Python(observer) => observer.bind(py).borrow().registration(),
            Self::Memory(observer) => observer.bind(py).borrow().registration(),
            Self::Log(observer) => observer.bind(py).borrow().registration(),
            Self::Metrics(observer) => observer.bind(py).borrow().registration(),
            Self::Otel(observer) => observer.bind(py).borrow().registration(),
            Self::Billing(observer) => observer.bind(py).borrow().registration(),
            Self::Notify(observer) => observer.bind(py).borrow().registration(),
        }
    }
}

pub(crate) const PREVIEW_VERSION: Version = Version {
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
    /// Durable exact-model authorization folded into every run's security
    /// context when a summarize compaction middleware is registered.
    pub(crate) compaction_authorization: Option<CompactionAuthorization>,
}

#[pymethods]
impl PyAgent {
    /// Share this agent's exact artifact store with other agent compositions.
    #[getter]
    fn artifact_store(&self) -> PyArtifactStore {
        PyArtifactStore {
            inner: self.artifact_store.clone(),
        }
    }

    /// Return a newly resolved agent with explicit accepted limits and pricing policy.
    /// Existing runs keep their original limits; no model or tool is dispatched.
    fn with_limits<'py>(
        &self,
        py: Python<'py>,
        limits: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let limits = crate::json_bridge::py_to_json(limits)?;
        let limits: finstack_ai_kernel::RunLimits = serde_json::from_value(limits)
            .map_err(|_| PyValueError::new_err("invalid run limits"))?;
        let mut agent = self.clone_ref(py);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            agent.inner = Arc::new(
                agent
                    .inner
                    .with_limits(limits)
                    .await
                    .map_err(|error| agent_error(&error, None))?,
            );
            Ok(agent)
        })
    }
    /// Compose an agent with a fresh bounded process-local history cache.
    fn with_history_cache(&self, py: Python<'_>, policy: &Bound<'_, PyHistoryCachePolicy>) -> Self {
        let inner = self
            .inner
            .as_ref()
            .clone()
            .with_history_cache_policy(policy.borrow().inner);
        Self {
            inner: Arc::new(inner),
            ..self.clone_ref(py)
        }
    }

    /// Construct a Rust-backed official `OpenAI` Responses agent.
    ///
    /// `api_key` is required and keyword-only. The factory always targets
    /// `https://api.openai.com/v1/responses` and does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, reasoning_effort = None, reasoning_summary = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None, artifact_store = None, document_tools = true, sqlite_path = None, sqlite_durability = None, postgres_dsn = None))]
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

        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
        artifact_store: Option<PyArtifactStoreArg>,
        document_tools: bool,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
        postgres_dsn: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (common, sidecar) = linked_common(
            py,
            instruction,
            capabilities,
            active_capabilities,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            child_runs,
            approval_grant,
            artifact_path,
            artifact_store,
            document_tools,
        )?;
        sidecar.build(
            py,
            build_linked_agent(
                LinkedProviderSpec::OpenAi(OpenAiAgentSpec {
                    model,
                    api_key,
                    reasoning_effort,
                    reasoning_summary,

                    common,
                }),
                (sqlite_path, sqlite_durability, postgres_dsn),
            ),
        )
    }

    /// Construct a Rust-backed `OpenRouter` Responses agent.
    ///
    /// `api_key` is required and keyword-only. The factory always targets
    /// `https://openrouter.ai/api/v1/responses` and does not read environment
    /// variables. `referer` and `title` set the non-secret attribution
    /// headers. Does not attach a `MediaResolver`; vision, file, and audio
    /// input require a host-built provider because Python factories do not
    /// accept host callback resolvers across FFI.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, referer = None, title = None, reasoning_effort = None, reasoning_summary = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None, artifact_store = None, document_tools = true, sqlite_path = None, sqlite_durability = None, postgres_dsn = None))]
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

        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
        artifact_store: Option<PyArtifactStoreArg>,
        document_tools: bool,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
        postgres_dsn: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (common, sidecar) = linked_common(
            py,
            instruction,
            capabilities,
            active_capabilities,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            child_runs,
            approval_grant,
            artifact_path,
            artifact_store,
            document_tools,
        )?;
        sidecar.build(
            py,
            build_linked_agent(
                LinkedProviderSpec::OpenRouter(OpenRouterAgentSpec {
                    model,
                    api_key,
                    referer,
                    title,
                    reasoning_effort,
                    reasoning_summary,

                    common,
                }),
                (sqlite_path, sqlite_durability, postgres_dsn),
            ),
        )
    }

    /// Construct a Rust-backed Anthropic Messages agent.
    ///
    /// `api_key` stays positional. Python port lists are keyword-only. HTTPS is
    /// required when `api_key` is set; the binding does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None, *, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None, artifact_store = None, document_tools = true, sqlite_path = None, sqlite_durability = None, postgres_dsn = None))]
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

        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
        artifact_store: Option<PyArtifactStoreArg>,
        document_tools: bool,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
        postgres_dsn: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (common, sidecar) = linked_common(
            py,
            instruction,
            capabilities,
            active_capabilities,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            child_runs,
            approval_grant,
            artifact_path,
            artifact_store,
            document_tools,
        )?;
        sidecar.build(
            py,
            build_linked_agent(
                LinkedProviderSpec::Anthropic(AnthropicAgentSpec {
                    base_url,
                    model,
                    api_key,
                    common,
                }),
                (sqlite_path, sqlite_durability, postgres_dsn),
            ),
        )
    }

    /// Construct a Rust-backed Gemini `generateContent` agent.
    ///
    /// `api_key` stays positional. Python port lists are keyword-only. HTTPS is
    /// required when `api_key` is set; the binding does not read environment
    /// variables. Does not hardcode the Google host: `endpoint` is passed
    /// straight into the provider's `GeminiConfig::try_new`.
    #[staticmethod]
    #[pyo3(signature = (endpoint, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None, *, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None, artifact_store = None, document_tools = true, sqlite_path = None, sqlite_durability = None, postgres_dsn = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards provider auth and primary port components distinctly"
    )]
    fn gemini(
        py: Python<'_>,
        endpoint: String,
        model: String,
        api_key: Option<String>,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,

        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
        artifact_store: Option<PyArtifactStoreArg>,
        document_tools: bool,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
        postgres_dsn: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (common, sidecar) = linked_common(
            py,
            instruction,
            capabilities,
            active_capabilities,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            child_runs,
            approval_grant,
            artifact_path,
            artifact_store,
            document_tools,
        )?;
        sidecar.build(
            py,
            build_linked_agent(
                LinkedProviderSpec::Gemini(GeminiAgentSpec {
                    endpoint,
                    model,
                    api_key,
                    common,
                }),
                (sqlite_path, sqlite_durability, postgres_dsn),
            ),
        )
    }

    /// Construct a keyless Rust-backed Ollama/local agent.
    ///
    /// Python port lists are keyword-only. This factory does not accept an
    /// API key.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None, *, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None, artifact_store = None, document_tools = true, sqlite_path = None, sqlite_durability = None, postgres_dsn = None))]
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

        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
        artifact_store: Option<PyArtifactStoreArg>,
        document_tools: bool,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
        postgres_dsn: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (common, sidecar) = linked_common(
            py,
            instruction,
            capabilities,
            active_capabilities,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            child_runs,
            approval_grant,
            artifact_path,
            artifact_store,
            document_tools,
        )?;
        sidecar.build(
            py,
            build_linked_agent(
                LinkedProviderSpec::Ollama(OllamaAgentSpec {
                    base_url,
                    model,
                    common,
                }),
                (sqlite_path, sqlite_durability, postgres_dsn),
            ),
        )
    }

    /// Construct a Rust-backed agent that dispatches to a dedicated provider.
    ///
    /// `hard_input_bytes`, `wire_protocol`, and `credential_name` are
    /// required. `openai_chat` is a configuration error. The binding does
    /// not read environment variables.
    #[staticmethod]
    #[pyo3(signature = (endpoint, model, instruction = None, capabilities = None, active_capabilities = None, *, wire_protocol, credential_name, hard_input_bytes = None, auth = None, api_key = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None, child_runs = None, approval_grant = None, artifact_path = None, artifact_store = None, document_tools = true, sqlite_path = None, sqlite_durability = None, postgres_dsn = None))]
    #[expect(
        clippy::too_many_arguments,
        reason = "linked factory forwards gateway route, auth and primary port components distinctly"
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

        toolsets: Option<Vec<PyToolsetArg>>,
        context_providers: Option<Vec<PyContextProviderArg>>,
        middleware: Option<Vec<PyMiddlewareArg>>,
        observers: Option<Vec<PyObserverArg>>,
        output_type: Option<Py<PyAny>>,
        child_runs: Option<Py<PyChildRunPolicy>>,
        approval_grant: Option<Py<PyApprovalGrantMode>>,
        artifact_path: Option<String>,
        artifact_store: Option<PyArtifactStoreArg>,
        document_tools: bool,
        sqlite_path: Option<String>,
        sqlite_durability: Option<PySqliteDurability>,
        postgres_dsn: Option<String>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (common, sidecar) = linked_common(
            py,
            instruction,
            capabilities,
            active_capabilities,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            child_runs,
            approval_grant,
            artifact_path,
            artifact_store,
            document_tools,
        )?;
        sidecar.build(
            py,
            build_linked_agent(
                LinkedProviderSpec::Gateway(GatewayAgentSpec {
                    endpoint,
                    model,
                    wire_protocol,
                    credential_name,
                    hard_input_bytes,
                    auth_kind: auth,
                    api_key,
                    common,
                }),
                (sqlite_path, sqlite_durability, postgres_dsn),
            ),
        )
    }

    /// Construct an agent from trusted coarse Python model and Toolset callbacks.
    #[staticmethod]
    #[pyo3(signature = (model, toolsets = None, instruction = None, output_type = None, capabilities = None, active_capabilities = None, context_providers = None, middleware = None, observers = None, *, child_runs = None, approval_grant = None, sqlite_path = None, sqlite_durability = None, artifact_path = None, artifact_store = None, document_tools = true, capability_toolsets = None, postgres_dsn = None))]
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
        artifact_store: Option<PyArtifactStoreArg>,
        document_tools: bool,
        capability_toolsets: Option<Vec<PyToolsetArg>>,
        postgres_dsn: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = model.borrow();
        let model_name = model.model_name();
        let model = model.registration();
        let LinkedPorts {
            mut media,
            ports,
            sidecar,
            skills,
        } = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
            artifact_path,
            artifact_store,
            document_tools,
        )?;
        let capability_toolsets = capability_toolsets
            .unwrap_or_default()
            .into_iter()
            .map(|toolset| toolset.registration(py, &sidecar.artifact_store, &mut media))
            .collect::<PyResult<Vec<_>>>()?;
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let common = LinkedCommon {
            journal_store: None,
            instruction,
            capabilities,
            active_capabilities,
            ports,
            child_runs: child_runs_or_deny(py, child_runs),
            approval_grant: approval_grant_or_per_call(py, approval_grant),
        };
        sidecar.build(
            py,
            build_python_agent(
                model_name,
                model,
                common,
                (sqlite_path, sqlite_durability, postgres_dsn),
                skills,
                capability_toolsets,
            ),
        )
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
        let agent = self.clone_ref(py);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = agent
                .inner
                .re_resolve()
                .await
                .map_err(|error| agent_error(&error, None))?;
            Python::attach(|py| {
                Py::new(
                    py,
                    PyAgent {
                        inner: Arc::new(inner),
                        ..agent
                    },
                )
            })
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
            let inner = Session::create(store, tenant_scope)
                .await
                .map_err(|error| session_py_error(&error))?;
            Python::attach(|py| Py::new(py, PySession { inner }))
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
            let session_id = SessionId::parse(&session_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            let inner = Session::open(store, session_id, tenant_scope)
                .await
                .map_err(|error| session_py_error(&error))?;
            Python::attach(|py| Py::new(py, PySession { inner }))
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
                .map_err(|error| session_py_error(&error))?;
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
        self.launch(
            py,
            "python-local".to_owned(),
            input,
            timeout_seconds,
            max_cycles,
            max_output_retries,
            capability,
            attachments,
            Agent::start,
        )
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
        let agent = self.clone_ref(py);
        let timeout_seconds = timeout_seconds.unwrap_or(agent.default_timeout_seconds);
        let attachments = collect_attachments(py, attachments);
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let staged =
                stage_attachments(agent.artifact_store.as_ref(), "python-local", attachments)
                    .await
                    .map_err(|error| agent_error(&error, None))?;
            let request = run_request(
                &agent.model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                agent.settings,
                "python-local",
                staged,
                agent.compaction_authorization,
            )
            .map_err(|error| agent_error(&error, None))?;
            let run = agent
                .inner
                .start(request)
                .map_err(|error| agent_error(&error, None))?;
            let output = run
                .result()
                .await
                .map_err(|error| run_error(&run, &error))?;
            Python::attach(|py| run_result(py, output, agent.output_adapter))
        })
    }
}

impl PyAgent {
    /// Clone the handle; every field is a shared reference or plain data.
    pub(crate) fn clone_ref(&self, py: Python<'_>) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            model: self.model.clone(),
            output_adapter: self
                .output_adapter
                .as_ref()
                .map(|adapter| adapter.clone_ref(py)),
            settings: self.settings.clone(),
            default_timeout_seconds: self.default_timeout_seconds,
            artifact_store: Arc::clone(&self.artifact_store),
            compaction_authorization: self.compaction_authorization.clone(),
        }
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
        let lane = lane.clone();
        let tenant_scope = lane.session().tenant_scope().to_string();
        self.launch(
            py,
            tenant_scope,
            input,
            timeout_seconds,
            max_cycles,
            max_output_retries,
            capability,
            attachments,
            move |agent, request| lane.run(agent, request),
        )
    }

    /// Stage `attachments`, build the bounded run request, and hand it to
    /// `launch`, all with the GIL released.
    #[expect(
        clippy::too_many_arguments,
        reason = "forwards the bounded run inputs shared by Agent.start and Lane.run"
    )]
    fn launch(
        &self,
        py: Python<'_>,
        tenant_scope: String,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
        attachments: Option<Vec<Py<PyAttachment>>>,
        launch: impl FnOnce(&Agent, AgentRunRequest) -> Result<AgentRun, AgentRunError> + Send,
    ) -> PyResult<PyRun> {
        let PyAgent {
            inner,
            model,
            output_adapter,
            settings,
            default_timeout_seconds,
            artifact_store,
            compaction_authorization,
        } = self.clone_ref(py);
        let timeout_seconds = timeout_seconds.unwrap_or(default_timeout_seconds);
        let attachments = collect_attachments(py, attachments);
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
                compaction_authorization,
            )?;
            launch(&inner, request).map(|inner| PyRun {
                inner,
                output_adapter,
            })
        })
        .map_err(|error| agent_error(&error, None))
    }
}

/// Binding-side state the Python `Agent` carries beside the Rust agent.
struct AgentSidecar {
    output_adapter: Option<Py<PyAny>>,
    artifact_store: Arc<dyn ArtifactStore>,
    compaction_authorization: Option<CompactionAuthorization>,
}

impl AgentSidecar {
    /// Drive `build` on the Rust runtime and wrap its agent into the Python
    /// handle.
    fn build(
        self,
        py: Python<'_>,
        build: impl Future<Output = Result<LinkedAgent, AgentRunError>> + Send + 'static,
    ) -> PyResult<Bound<'_, PyAny>> {
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let linked = build.await.map_err(|error| agent_error(&error, None))?;
            Python::attach(|py| {
                Py::new(
                    py,
                    PyAgent {
                        inner: Arc::new(linked.agent),
                        model: linked.model,
                        output_adapter: self.output_adapter,
                        settings: linked.settings,
                        default_timeout_seconds: linked.default_timeout.as_secs_f64(),
                        artifact_store: self.artifact_store,
                        compaction_authorization: self.compaction_authorization,
                    },
                )
            })
        })
    }
}

/// Port registrations assembled from the factory keyword arguments.
struct LinkedPorts {
    media: MediaRegistrations,
    ports: LinkedAgentPorts,
    sidecar: AgentSidecar,
    /// Deferred skills toolset: its catalog depends on the declared
    /// capabilities, so it is built at agent assembly (`from_python` only).
    skills: Option<ComponentRef>,
}

/// Build the shared artifact store plus the
/// `DocumentToolset`/`DocumentIngestMiddleware` registrations that share it.
///
/// Every agent factory registers the shared store and ingestion middleware.
/// The document toolset is optional (`document_tools=false` creates tool-free
/// grader compositions when no explicit toolsets/capabilities are supplied).
/// The default preserves the
/// document-ingest lane test's wiring, so `Agent.run`/`start` can stage
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

/// Explicit registration version of the native document toolset.
const DOCUMENT_TOOLSET_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

fn document_ingest_ports(
    artifact_path: Option<String>,
    explicit_store: Option<Arc<dyn ArtifactStore>>,
) -> Result<DocumentIngestPorts, AgentRunError> {
    let dyn_store: Arc<dyn ArtifactStore> = match (explicit_store, artifact_path) {
        (Some(store), _) => store,
        (None, Some(path)) => Arc::new(
            LocalArtifactStore::try_new(std::path::PathBuf::from(path))
                .map_err(|error| configuration_error(error.to_string()))?,
        ),
        (None, None) => Arc::new(InProcessArtifactStore::default()),
    };
    let toolset = DocumentToolset::try_new(Arc::clone(&dyn_store))
        .map_err(|error| configuration_error(error.to_string()))?;
    let middleware = DocumentIngestMiddleware::try_new(Arc::clone(&dyn_store))
        .map_err(|error| configuration_error(error.to_string()))?;
    Ok((
        Arc::clone(&dyn_store),
        (
            component("finstack.tools.document", DOCUMENT_TOOLSET_VERSION)?,
            Arc::new(toolset) as Arc<dyn Toolset>,
        ),
        (
            ComponentRef::new(
                middleware.descriptor().invocation.component,
                Some(middleware.descriptor().invocation.version),
            ),
            Arc::new(middleware) as Arc<dyn Middleware>,
        ),
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per factory keyword; grouping would only rename them"
)]
fn linked_ports(
    py: Python<'_>,
    toolsets: Option<Vec<PyToolsetArg>>,
    context_providers: Option<Vec<PyContextProviderArg>>,
    middleware: Option<Vec<PyMiddlewareArg>>,
    observers: Option<Vec<PyObserverArg>>,
    output_type: Option<Py<PyAny>>,
    artifact_path: Option<String>,
    artifact_store: Option<PyArtifactStoreArg>,
    document_tools: bool,
) -> PyResult<LinkedPorts> {
    if artifact_path.is_some() && artifact_store.is_some() {
        return Err(PyValueError::new_err(
            "artifact_path and artifact_store are mutually exclusive",
        ));
    }
    let explicit_store = artifact_store.map(|store| store.inner(py));
    let (artifact_store, document_toolset, document_middleware) =
        document_ingest_ports(artifact_path, explicit_store)
            .map_err(|error| agent_error(&error, None))?;
    let mut skills: Option<ComponentRef> = None;
    let mut plain: Vec<PyToolsetArg> = Vec::new();
    for toolset in toolsets.unwrap_or_default() {
        match toolset {
            PyToolsetArg::Skills(handle) => {
                if skills.is_some() {
                    return Err(PyValueError::new_err(
                        "at most one SkillsToolset may be registered",
                    ));
                }
                skills = Some(handle.bind(py).borrow().component_ref());
            }
            other => plain.push(other),
        }
    }
    let mut media = MediaRegistrations::new(Arc::clone(&artifact_store));
    let mut toolsets = plain
        .into_iter()
        .map(|toolset| toolset.registration(py, &artifact_store, &mut media))
        .collect::<PyResult<Vec<_>>>()?;
    if document_tools {
        toolsets.push(document_toolset);
    }
    let middleware_args = middleware.unwrap_or_default();
    let compaction_authorization = middleware_args.iter().find_map(|entry| match entry {
        PyMiddlewareArg::Compaction(handle) => handle.bind(py).borrow().authorization(),
        _ => None,
    });
    let mut middleware = middleware_args
        .into_iter()
        .map(|middleware| middleware.registration(py))
        .collect::<Vec<_>>();
    middleware.push(document_middleware);
    let context_providers = context_providers
        .unwrap_or_default()
        .into_iter()
        .map(|provider| provider.registration(py, &artifact_store))
        .collect::<PyResult<Vec<_>>>()?;
    let observers: Vec<_> = observers
        .unwrap_or_default()
        .into_iter()
        .map(|observer| observer.registration(py))
        .collect();
    let (output_adapter, output_schema) = output_type
        .map(|target| prepare_pydantic_output(py, target))
        .transpose()?
        .unzip();
    Ok(LinkedPorts {
        media,
        ports: LinkedAgentPorts {
            toolsets,
            artifact_store: Some(Arc::clone(&artifact_store)),
            context_providers: context_providers
                .into_iter()
                .map(|(_, handle)| handle)
                .collect(),
            middleware: middleware.into_iter().map(|(_, handle)| handle).collect(),
            observers: observers.into_iter().map(|(_, handle)| handle).collect(),
            output_schema,
        },
        sidecar: AgentSidecar {
            output_adapter,
            artifact_store,
            compaction_authorization,
        },
        skills,
    })
}

/// Convert the keyword arguments every linked provider factory shares into
/// the facade's `LinkedCommon` and the binding-side sidecar.
#[expect(
    clippy::too_many_arguments,
    reason = "one parameter per shared factory keyword; grouping would only rename them"
)]
fn linked_common(
    py: Python<'_>,
    instruction: Option<String>,
    capabilities: Option<Vec<Py<PyCapability>>>,
    active_capabilities: Option<Vec<String>>,

    toolsets: Option<Vec<PyToolsetArg>>,
    context_providers: Option<Vec<PyContextProviderArg>>,
    middleware: Option<Vec<PyMiddlewareArg>>,
    observers: Option<Vec<PyObserverArg>>,
    output_type: Option<Py<PyAny>>,
    child_runs: Option<Py<PyChildRunPolicy>>,
    approval_grant: Option<Py<PyApprovalGrantMode>>,
    artifact_path: Option<String>,
    artifact_store: Option<PyArtifactStoreArg>,
    document_tools: bool,
) -> PyResult<(LinkedCommon, AgentSidecar)> {
    let (capabilities, active_capabilities) =
        capability_configuration(py, capabilities, active_capabilities)?;
    let LinkedPorts {
        media: _,
        ports,
        sidecar,
        skills,
    } = linked_ports(
        py,
        toolsets,
        context_providers,
        middleware,
        observers,
        output_type,
        artifact_path,
        artifact_store,
        document_tools,
    )?;
    // The linked provider factories build through `Agent::linked`, which has
    // no builder access, so the deferred skills toolset cannot attach its
    // activation host there yet.
    if skills.is_some() {
        return Err(PyValueError::new_err(
            "SkillsToolset is currently supported only by Agent.from_python",
        ));
    }
    Ok((
        LinkedCommon {
            journal_store: None,
            instruction,
            capabilities,
            active_capabilities,
            ports,
            child_runs: child_runs_or_deny(py, child_runs),
            approval_grant: approval_grant_or_per_call(py, approval_grant),
        },
        sidecar,
    ))
}

/// Open the same explicit journal choices for every linked provider.
async fn build_linked_agent(
    mut spec: LinkedProviderSpec,
    journal: (Option<String>, Option<PySqliteDurability>, Option<String>),
) -> Result<LinkedAgent, AgentRunError> {
    let store = open_journal_registration(journal).await?;
    let common = match &mut spec {
        LinkedProviderSpec::OpenAi(spec) => &mut spec.common,
        LinkedProviderSpec::OpenRouter(spec) => &mut spec.common,
        LinkedProviderSpec::Anthropic(spec) => &mut spec.common,
        LinkedProviderSpec::Gemini(spec) => &mut spec.common,
        LinkedProviderSpec::Ollama(spec) => &mut spec.common,
        LinkedProviderSpec::Gateway(spec) => &mut spec.common,
    };
    common.journal_store = Some(store);
    Agent::linked(spec).await
}

async fn build_python_agent(
    model_name: ModelName,
    model: (ComponentRef, Arc<dyn Model>),
    common: LinkedCommon,
    journal: (Option<String>, Option<PySqliteDurability>, Option<String>),
    skills: Option<ComponentRef>,
    capability_toolsets: Vec<(ComponentRef, Arc<dyn Toolset>)>,
) -> Result<LinkedAgent, AgentRunError> {
    let store = open_journal_registration(journal).await?;
    let mut builder = Agent::builder(
        AgentId::parse("python.agent.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse("python.bundle.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        model,
        store,
    );
    if let Some(skills_component) = skills {
        let (registration, host) = build_skills_ports(skills_component, &common.capabilities)?;
        builder = builder
            .toolset(registration.0, registration.1)
            .capability_activation_host(host);
    }
    for (component, toolset) in capability_toolsets {
        builder = builder.capability_toolset(component, toolset);
    }
    builder
        .build_linked(
            common,
            model_name,
            empty_model_settings()?,
            DEFAULT_RUN_TIMEOUT,
        )
        .await
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

/// Parse `id` into a versioned component reference inside agent assembly,
/// where failures surface as configuration errors.
pub(crate) fn component(id: &str, version: Version) -> Result<ComponentRef, AgentRunError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(|error| configuration_error(error.to_string()))?,
        Some(version),
    ))
}
