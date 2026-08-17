//! Rust-owned resolved agent handle and linked-provider builders.

use std::sync::Arc;

use finstack_ai::runtime::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentRef, JournalStore, Model, ModelName,
    ModelSettings, RawJson, Version,
};
use finstack_ai::{Agent, AgentRunError, CapabilitySpec, Session};
use finstack_ai_provider_anthropic::{
    AnthropicConfig, AnthropicModelConfig, AnthropicProvider,
    Authentication as AnthropicAuthentication, SecretString as AnthropicSecret,
};
use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_provider_openai::{
    Authentication as OpenAiAuthentication, OpenAiConfig, OpenAiModelConfig, OpenAiProvider,
    SecretString as OpenAiSecret,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::ConfigurationError;
use crate::callbacks::{
    PyPythonContextProvider, PyPythonMiddleware, PyPythonModel, PyPythonObserver, PyPythonToolset,
};
use crate::capability::PyCapability;
use crate::errors::{
    agent_error, configuration_error, model_configuration_error, session_py_error,
};
use crate::run::{
    PreparedPydanticOutput, PyRun, prepare_pydantic_output, result_to_python_with_locator,
    run_request,
};
use crate::session::PySession;

const DEFAULT_TIMEOUT_SECONDS: f64 = 30.0;
const OPENAI_TIMEOUT_SECONDS: f64 = 120.0;
const DEFAULT_MAX_CYCLES: u64 = 16;
const LINKED_CONTEXT_WINDOW_TOKENS: u64 = 1_050_000;
const LINKED_RESERVED_OUTPUT_TOKENS: u64 = 128_000;
/// Anthropic rejects `max_tokens` above the selected model's own ceiling, and
/// the current Claude generation tops out at 64,000 output tokens.
const LINKED_ANTHROPIC_OUTPUT_TOKENS: u64 = 64_000;
const LINKED_PROVIDER_OVERHEAD_TOKENS: u64 = 64;
const REASONING_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
const REASONING_SUMMARIES: &[&str] = &["auto", "concise", "detailed"];
const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

/// Rust-owned resolved agent handle.
#[pyclass(module = "finstack_ai._finstack_ai", name = "Agent", frozen)]
pub(crate) struct PyAgent {
    inner: Arc<Agent>,
    model: ModelName,
    output_adapter: Option<Py<PyAny>>,
    settings: ModelSettings,
    default_timeout_seconds: f64,
}

#[pymethods]
impl PyAgent {
    /// Construct a Rust-backed official `OpenAI` Responses agent.
    ///
    /// `api_key` is required and keyword-only. The factory always targets
    /// `https://api.openai.com/v1/responses` and does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (model, instruction = None, capabilities = None, active_capabilities = None, *, api_key, reasoning_effort = None, reasoning_summary = None, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None))]
    #[expect(
        clippy::too_many_arguments,
        clippy::needless_pass_by_value,
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
    ) -> PyResult<Bound<'_, PyAny>> {
        let settings =
            reasoning_settings(reasoning_effort.as_deref(), reasoning_summary.as_deref())?;
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let ports = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_openai_agent(
                model,
                instruction,
                capabilities,
                active_capabilities,
                api_key,
                settings,
                ports,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Construct a Rust-backed Anthropic Messages agent.
    ///
    /// `api_key` stays positional. Python port lists are keyword-only. HTTPS is
    /// required when `api_key` is set; the binding does not read environment
    /// variables.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None, *, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None))]
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
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let ports = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_anthropic_agent(
                base_url,
                model,
                api_key,
                instruction,
                capabilities,
                active_capabilities,
                ports,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Construct a keyless Rust-backed Ollama/local agent.
    ///
    /// Python port lists are keyword-only. This factory does not accept an
    /// API key.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None, *, toolsets = None, context_providers = None, middleware = None, observers = None, output_type = None))]
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
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        let ports = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_ollama_agent(
                base_url,
                model,
                instruction,
                capabilities,
                active_capabilities,
                ports,
            )
            .await;
            Python::attach(|py| match built {
                Ok(value) => Py::new(py, value),
                Err(error) => Err(agent_error(py, &error, None)),
            })
        })
    }

    /// Construct an agent from trusted coarse Python model and Toolset callbacks.
    #[staticmethod]
    #[pyo3(signature = (model, toolsets = None, instruction = None, output_type = None, capabilities = None, active_capabilities = None, context_providers = None, middleware = None, observers = None))]
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
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = model.borrow();
        let model_name = model.model_name();
        let model = model.registration();
        let ports = linked_ports(
            py,
            toolsets,
            context_providers,
            middleware,
            observers,
            output_type,
        )?;
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_python_agent(
                model_name,
                model,
                instruction,
                ports,
                capabilities,
                active_capabilities,
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
            let session_id = finstack_ai::runtime::SessionId::parse(&session_id)
                .map_err(|error| ConfigurationError::new_err(error.to_string()))?;
            match Session::open(store, session_id, tenant_scope).await {
                Ok(inner) => Python::attach(|py| Py::new(py, PySession { inner })),
                Err(error) => Python::attach(|py| Err(session_py_error(py, &error))),
            }
        })
    }

    /// Start a run and return its shared control handle immediately.
    #[pyo3(signature = (input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None))]
    fn start(
        &self,
        py: Python<'_>,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
    ) -> PyResult<PyRun> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        let settings = self.settings.clone();
        let timeout_seconds = timeout_seconds.unwrap_or(self.default_timeout_seconds);
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        py.detach(move || {
            let request = run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                settings,
            )?;
            let runtime = pyo3_async_runtimes::tokio::get_runtime();
            let _guard = runtime.enter();
            agent.start(request).map(|inner| PyRun {
                inner,
                output_adapter,
            })
        })
        .map_err(|error| agent_error(py, &error, None))
    }

    /// Execute one run and await its committed result.
    #[pyo3(signature = (input, *, timeout_seconds = None, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
        let settings = self.settings.clone();
        let timeout_seconds = timeout_seconds.unwrap_or(self.default_timeout_seconds);
        let output_adapter = self
            .output_adapter
            .as_ref()
            .map(|adapter| adapter.clone_ref(py));
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let request = match run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                settings,
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

async fn build_openai_agent(
    model: String,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    api_key: String,
    settings: ModelSettings,
    ports: LinkedPorts,
) -> Result<PyAgent, AgentRunError> {
    let config = OpenAiConfig::try_new("https://api.openai.com")
        .map_err(model_configuration_error)?
        .with_authentication(OpenAiAuthentication::Bearer(
            OpenAiSecret::try_new(api_key).map_err(model_configuration_error)?,
        ));
    let model_config = linked_openai_model_config(&model, true)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OpenAiProvider::try_new(config, vec![model_config]).map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.openai",
        bundle_id: "python.bundle.openai",
        model: (component("python.model.openai")?, provider),
        model_name,
        instruction,
        capabilities,
        active_capabilities,
        ports,
        settings,
        default_timeout_seconds: OPENAI_TIMEOUT_SECONDS,
    })
    .await
}

async fn build_anthropic_agent(
    base_url: String,
    model: String,
    api_key: Option<String>,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    ports: LinkedPorts,
) -> Result<PyAgent, AgentRunError> {
    let mut config = AnthropicConfig::try_new(base_url).map_err(model_configuration_error)?;
    if let Some(api_key) = api_key {
        config = config.with_authentication(AnthropicAuthentication::ApiKey(
            AnthropicSecret::try_new(api_key).map_err(model_configuration_error)?,
        ));
    }
    let model_config = AnthropicModelConfig::try_new(
        &model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_ANTHROPIC_OUTPUT_TOKENS,
        LINKED_ANTHROPIC_OUTPUT_TOKENS,
        LINKED_PROVIDER_OVERHEAD_TOKENS,
    )
    .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        AnthropicProvider::try_new(config, vec![model_config])
            .map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.anthropic",
        bundle_id: "python.bundle.anthropic",
        model: (component("python.model.anthropic")?, provider),
        model_name,
        instruction,
        capabilities,
        active_capabilities,
        ports,
        settings: empty_model_settings()?,
        default_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
    })
    .await
}

async fn build_ollama_agent(
    base_url: String,
    model: String,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    ports: LinkedPorts,
) -> Result<PyAgent, AgentRunError> {
    let config = OllamaConfig::try_new(base_url).map_err(model_configuration_error)?;
    let model_config = OllamaModelConfig::try_new(
        &model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_PROVIDER_OVERHEAD_TOKENS,
    )
    .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OllamaProvider::try_new(config, vec![model_config]).map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.ollama",
        bundle_id: "python.bundle.ollama",
        model: (component("python.model.ollama")?, provider),
        model_name,
        instruction,
        capabilities,
        active_capabilities,
        ports,
        settings: empty_model_settings()?,
        default_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
    })
    .await
}

struct LinkedPorts {
    toolsets: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Observer>)>,
    output: Option<PreparedPydanticOutput>,
}

fn linked_ports(
    py: Python<'_>,
    toolsets: Option<Vec<Py<PyPythonToolset>>>,
    context_providers: Option<Vec<Py<PyPythonContextProvider>>>,
    middleware: Option<Vec<Py<PyPythonMiddleware>>>,
    observers: Option<Vec<Py<PyPythonObserver>>>,
    output_type: Option<Py<PyAny>>,
) -> PyResult<LinkedPorts> {
    Ok(LinkedPorts {
        toolsets: toolsets
            .unwrap_or_default()
            .into_iter()
            .map(|toolset| toolset.bind(py).borrow().registration())
            .collect(),
        context_providers: context_providers
            .unwrap_or_default()
            .into_iter()
            .map(|provider| provider.bind(py).borrow().registration())
            .collect(),
        middleware: middleware
            .unwrap_or_default()
            .into_iter()
            .map(|middleware| middleware.bind(py).borrow().registration())
            .collect(),
        observers: observers
            .unwrap_or_default()
            .into_iter()
            .map(|observer| observer.bind(py).borrow().registration())
            .collect(),
        output: output_type
            .map(|target| prepare_pydantic_output(py, target))
            .transpose()?,
    })
}

struct LinkedAgentSpec {
    agent_id: &'static str,
    bundle_id: &'static str,
    model: (ComponentRef, Arc<dyn Model>),
    model_name: ModelName,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
    ports: LinkedPorts,
    settings: ModelSettings,
    default_timeout_seconds: f64,
}

async fn finish_linked_agent(spec: LinkedAgentSpec) -> Result<PyAgent, AgentRunError> {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 64,
            batches_per_session: 256,
            records_per_session: 4_096,
            snapshot_bytes: 64 * 1_024,
        })
        .map_err(|error| configuration_error(error.to_string()))?,
    );
    let mut builder = Agent::builder(
        AgentId::parse(spec.agent_id).map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse(spec.bundle_id).map_err(|error| configuration_error(error.to_string()))?,
        spec.model,
        (component("python.store.memory")?, store),
    );
    for (component, toolset) in spec.ports.toolsets {
        builder = builder.toolset(component, toolset);
    }
    for (component, provider) in spec.ports.context_providers {
        builder = builder.context_provider(component, provider);
    }
    for (component, middleware) in spec.ports.middleware {
        builder = builder.middleware(component, middleware);
    }
    for (component, observer) in spec.ports.observers {
        builder = builder.observer(component, observer);
    }
    if let Some(instruction) = spec.instruction {
        builder = builder.try_instruction(instruction)?;
    }
    for capability in spec.capabilities {
        builder = builder.capability(capability);
    }
    for capability in spec.active_capabilities {
        builder = builder.activate_application(capability);
    }
    let agent = builder.build().await?;
    let (agent, output_adapter) = if let Some(output) = spec.ports.output {
        (
            agent.try_with_output_schema(&output.schema)?,
            Some(output.adapter),
        )
    } else {
        (agent, None)
    };
    Ok(PyAgent {
        inner: Arc::new(agent),
        model: spec.model_name,
        output_adapter,
        settings: spec.settings,
        default_timeout_seconds: spec.default_timeout_seconds,
    })
}

async fn build_python_agent(
    model_name: ModelName,
    model: (ComponentRef, Arc<dyn Model>),
    instruction: Option<String>,
    ports: LinkedPorts,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<PyAgent, AgentRunError> {
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.callbacks",
        bundle_id: "python.bundle.callbacks",
        model,
        model_name,
        instruction,
        capabilities,
        active_capabilities,
        ports,
        settings: empty_model_settings()?,
        default_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
    })
    .await
}

fn linked_openai_model_config(
    model: &str,
    reasoning: bool,
) -> Result<OpenAiModelConfig, AgentRunError> {
    let mut config = OpenAiModelConfig::try_new(
        model,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_CONTEXT_WINDOW_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_RESERVED_OUTPUT_TOKENS,
        LINKED_PROVIDER_OVERHEAD_TOKENS,
    )
    .map_err(model_configuration_error)?;
    if reasoning {
        config = config.with_reasoning(true);
    }
    Ok(config)
}

pub(crate) fn empty_model_settings() -> Result<ModelSettings, AgentRunError> {
    Ok(ModelSettings {
        values: RawJson::parse(b"{}").map_err(|error| configuration_error(error.to_string()))?,
    })
}

fn reasoning_settings(effort: Option<&str>, summary: Option<&str>) -> PyResult<ModelSettings> {
    let mut fields = Vec::new();
    if let Some(value) = effort {
        if !REASONING_EFFORTS.contains(&value) {
            return Err(ConfigurationError::new_err(
                "reasoning_effort must be one of none, minimal, low, medium, high, xhigh, max",
            ));
        }
        fields.push(format!(r#""reasoning_effort":"{value}""#));
    }
    if let Some(value) = summary {
        if !REASONING_SUMMARIES.contains(&value) {
            return Err(ConfigurationError::new_err(
                "reasoning_summary must be one of auto, concise, detailed",
            ));
        }
        fields.push(format!(r#""reasoning_summary":"{value}""#));
    }
    if fields.is_empty() {
        return empty_model_settings()
            .map_err(|error| ConfigurationError::new_err(error.to_string()));
    }
    let payload = format!("{{{}}}", fields.join(","));
    RawJson::parse(payload.as_bytes())
        .map(|values| ModelSettings { values })
        .map_err(|error| ConfigurationError::new_err(error.to_string()))
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
