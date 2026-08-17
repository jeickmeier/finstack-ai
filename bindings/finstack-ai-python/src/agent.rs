//! Rust-owned resolved agent handle and linked-provider builders.

use std::sync::Arc;

use finstack_ai::runtime::{
    AgentId, BundleId, CapabilityId, ComponentId, ComponentRef, JournalStore, Model, ModelName,
    Version,
};
use finstack_ai::{Agent, AgentRunError, CapabilitySpec, Session};
use finstack_ai_provider_anthropic::{
    AnthropicConfig, AnthropicModelConfig, AnthropicProvider,
    Authentication as AnthropicAuthentication, SecretString as AnthropicSecret,
};
use finstack_ai_provider_openai_compatible::{
    EndpointKind, OpenAiCompatibleConfig, OpenAiCompatibleProvider, OpenAiModelConfig,
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
const DEFAULT_MAX_CYCLES: u64 = 16;
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
}

#[pymethods]
impl PyAgent {
    /// Construct a keyless Rust-backed OpenAI-compatible agent.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None))]
    fn openai_compatible(
        py: Python<'_>,
        base_url: String,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_openai_compatible_agent(
                base_url,
                model,
                instruction,
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

    /// Construct a Rust-backed Anthropic Messages agent.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, api_key = None, instruction = None, capabilities = None, active_capabilities = None))]
    fn anthropic(
        py: Python<'_>,
        base_url: String,
        model: String,
        api_key: Option<String>,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_anthropic_agent(
                base_url,
                model,
                api_key,
                instruction,
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

    /// Construct a keyless Rust-backed Ollama/local agent.
    #[staticmethod]
    #[pyo3(signature = (base_url, model, instruction = None, capabilities = None, active_capabilities = None))]
    fn ollama(
        py: Python<'_>,
        base_url: String,
        model: String,
        instruction: Option<String>,
        capabilities: Option<Vec<Py<PyCapability>>>,
        active_capabilities: Option<Vec<String>>,
    ) -> PyResult<Bound<'_, PyAny>> {
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_ollama_agent(
                base_url,
                model,
                instruction,
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
        let toolsets = toolsets
            .unwrap_or_default()
            .into_iter()
            .map(|toolset| toolset.bind(py).borrow().registration())
            .collect::<Vec<_>>();
        let context_providers = context_providers
            .unwrap_or_default()
            .into_iter()
            .map(|provider| provider.bind(py).borrow().registration())
            .collect::<Vec<_>>();
        let middleware = middleware
            .unwrap_or_default()
            .into_iter()
            .map(|middleware| middleware.bind(py).borrow().registration())
            .collect::<Vec<_>>();
        let observers = observers
            .unwrap_or_default()
            .into_iter()
            .map(|observer| observer.bind(py).borrow().registration())
            .collect::<Vec<_>>();
        let output = output_type
            .map(|target| prepare_pydantic_output(py, target))
            .transpose()?;
        let (capabilities, active_capabilities) =
            capability_configuration(py, capabilities, active_capabilities)?;
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let built = build_python_agent(
                model_name,
                model,
                toolsets,
                context_providers,
                middleware,
                observers,
                instruction,
                output,
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
    #[pyo3(signature = (input, *, timeout_seconds = DEFAULT_TIMEOUT_SECONDS, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None))]
    fn start(
        &self,
        py: Python<'_>,
        input: String,
        timeout_seconds: f64,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
    ) -> PyResult<PyRun> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
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
    #[pyo3(signature = (input, *, timeout_seconds = DEFAULT_TIMEOUT_SECONDS, max_cycles = DEFAULT_MAX_CYCLES, max_output_retries = 1, capability = None))]
    fn run<'py>(
        &self,
        py: Python<'py>,
        input: String,
        timeout_seconds: f64,
        max_cycles: u64,
        max_output_retries: u32,
        capability: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let model = self.model.clone();
        let agent = Arc::clone(&self.inner);
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

async fn build_openai_compatible_agent(
    base_url: String,
    model: String,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<PyAgent, AgentRunError> {
    let config = OpenAiCompatibleConfig::try_new(base_url, EndpointKind::Gateway)
        .map_err(model_configuration_error)?;
    let model_config = OpenAiModelConfig::try_new(&model, 1_048_576, 1_048_576, 512, 512, 64)
        .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OpenAiCompatibleProvider::try_new(config, vec![model_config])
            .map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.openai-compatible",
        bundle_id: "python.bundle.openai-compatible",
        model_id: "python.model.openai-compatible",
        model_name,
        provider,
        instruction,
        capabilities,
        active_capabilities,
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
) -> Result<PyAgent, AgentRunError> {
    let mut config = AnthropicConfig::try_new(base_url).map_err(model_configuration_error)?;
    if let Some(api_key) = api_key {
        config = config.with_authentication(AnthropicAuthentication::ApiKey(
            AnthropicSecret::try_new(api_key).map_err(model_configuration_error)?,
        ));
    }
    let model_config = AnthropicModelConfig::try_new(&model, 1_048_576, 1_048_576, 512, 512, 64)
        .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        AnthropicProvider::try_new(config, vec![model_config])
            .map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.anthropic",
        bundle_id: "python.bundle.anthropic",
        model_id: "python.model.anthropic",
        model_name,
        provider,
        instruction,
        capabilities,
        active_capabilities,
    })
    .await
}

async fn build_ollama_agent(
    base_url: String,
    model: String,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<PyAgent, AgentRunError> {
    let config =
        OpenAiCompatibleConfig::ollama_local(base_url).map_err(model_configuration_error)?;
    let model_config = OpenAiModelConfig::try_new(&model, 1_048_576, 1_048_576, 512, 512, 64)
        .map_err(model_configuration_error)?;
    let model_name = model_config.name.clone();
    let provider: Arc<dyn Model> = Arc::new(
        OpenAiCompatibleProvider::try_new(config, vec![model_config])
            .map_err(model_configuration_error)?,
    );
    finish_linked_agent(LinkedAgentSpec {
        agent_id: "python.agent.ollama",
        bundle_id: "python.bundle.ollama",
        model_id: "python.model.ollama",
        model_name,
        provider,
        instruction,
        capabilities,
        active_capabilities,
    })
    .await
}

struct LinkedAgentSpec {
    agent_id: &'static str,
    bundle_id: &'static str,
    model_id: &'static str,
    model_name: ModelName,
    provider: Arc<dyn Model>,
    instruction: Option<String>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
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
        (component(spec.model_id)?, spec.provider),
        (component("python.store.memory")?, store),
    );
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
    Ok(PyAgent {
        inner: Arc::new(agent),
        model: spec.model_name,
        output_adapter: None,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Python callback builder forwards all primary port components distinctly"
)]
async fn build_python_agent(
    model_name: ModelName,
    model: (ComponentRef, Arc<dyn Model>),
    toolsets: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Observer>)>,
    instruction: Option<String>,
    output: Option<PreparedPydanticOutput>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<PyAgent, AgentRunError> {
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
        AgentId::parse("python.agent.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        BundleId::parse("python.bundle.callbacks")
            .map_err(|error| configuration_error(error.to_string()))?,
        model,
        (component("python.store.memory")?, store),
    );
    for (component, toolset) in toolsets {
        builder = builder.toolset(component, toolset);
    }
    for (component, provider) in context_providers {
        builder = builder.context_provider(component, provider);
    }
    for (component, middleware) in middleware {
        builder = builder.middleware(component, middleware);
    }
    for (component, observer) in observers {
        builder = builder.observer(component, observer);
    }
    if let Some(instruction) = instruction {
        builder = builder.try_instruction(instruction)?;
    }
    for capability in capabilities {
        builder = builder.capability(capability);
    }
    for capability in active_capabilities {
        builder = builder.activate_application(capability);
    }
    let agent = builder.build().await?;
    let (agent, output_adapter) = if let Some(output) = output {
        (
            agent.try_with_output_schema(&output.schema)?,
            Some(output.adapter),
        )
    } else {
        (agent, None)
    };
    Ok(PyAgent {
        inner: Arc::new(agent),
        model: model_name,
        output_adapter,
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
