//! Python constructors for the native `finstack-ai-memory` composition.
//!
//! # Integration shape
//!
//! `MemoryExtension` is a factory for three *handles* — a context provider,
//! a toolset, and an observer — that are passed to the existing agent
//! factory parameters:
//!
//! ```python
//! memory = MemoryExtension.sqlite(path="memory.db", tenant="t1")
//! agent = await Agent.from_python(
//!     model,
//!     [memory.toolset()],
//!     context_providers=[memory.context_provider()],
//!     observers=[memory.observer()],
//! )
//! ```
//!
//! This mirrors the existing `PyToolsetArg` enum, which already accepts the
//! native `ElicitationToolset` beside trusted Python callback toolsets, and
//! keeps the seven agent factory signatures untouched (only the element
//! types of the three list parameters widen). The alternative — an extra
//! `memory=` keyword on every factory — would have added a parameter to
//! seven signatures and hidden the composition from the caller.
//!
//! The memory toolset is the one component that cannot be fully built here:
//! it needs an [`ArtifactStore`] for oversized memory bodies, and the agent
//! owns that store (the same `InProcessArtifactStore` that backs document
//! ingestion and `Agent.read_artifact`). `PyMemoryToolset` therefore carries
//! its configuration and is materialized inside `linked_ports`, against the
//! agent's own artifact store, so blobs staged by `remember` are readable
//! through `Agent.read_artifact`.

use std::path::Path;
use std::sync::Arc;

use finstack_ai::runtime::artifact::ArtifactStore;
use finstack_ai::runtime::ports::context::ContextProvider;
use finstack_ai::runtime::ports::observer::Observer;
use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai_kernel::{ComponentRef, Version};
use finstack_ai_memory::extract::RuleBasedExtractor;
use finstack_ai_memory::observer::MemoryObserver;
use finstack_ai_memory::provider::{MemoryContextProvider, RecallConfig};
use finstack_ai_memory::record::{MemoryScope, system_clock};
use finstack_ai_memory::store::SqliteMemoryStore;
use finstack_ai_memory::store::{InProcessMemoryStore, MemoryStore};
use finstack_ai_memory::toolset::{MemoryPolicy, MemoryToolset};
use pyo3::prelude::*;

use crate::component_ref;
use crate::errors::{agent_error, configuration_error};

/// Tenant scope `Agent.run` and `Agent.start` bind their runs to; the
/// default tenant, so recall works without configuration on that path.
const DEFAULT_TENANT: &str = "python-local";

/// Registered component identity of the native memory toolset.
const MEMORY_TOOLSET_COMPONENT: &str = "finstack.tools.memory";

/// Registration version of the native memory toolset.
///
/// `MemoryToolset`'s `ToolsetDescriptor` carries no invocation identity
/// (`finstack-ai-memory::toolset`), so `validate_toolset_descriptor` only
/// checks the descriptor name and this version is owned by the binding
/// rather than mirrored from the memory crate.
const MEMORY_TOOLSET_VERSION: Version = Version {
    major: 0,
    minor: 1,
    patch: 0,
};

/// Registered component identity of the native memory recall provider.
const MEMORY_CONTEXT_COMPONENT: &str = "finstack.context.memory";

/// `MemoryContextProvider`'s declared invocation version
/// (`MemoryContextProvider::try_new` in `finstack-ai-memory::provider`).
/// `validate_context_descriptor` requires the registered `ComponentRef`
/// to match the handle's own reported `(component id, version)` exactly.
/// 0.2.0: the recall contract gained the optional semantic leg, whether
/// or not this extension configures one.
const MEMORY_CONTEXT_VERSION: Version = Version {
    major: 0,
    minor: 2,
    patch: 0,
};

/// Map any memory-layer error onto the binding's `ConfigurationError`.
fn memory_py_error(error: &impl std::fmt::Display) -> PyErr {
    agent_error(&configuration_error(error.to_string()), None)
}

fn build_scope(
    tenant: &str,
    user: Option<String>,
    agent: Option<String>,
    workspace: Option<String>,
) -> PyResult<MemoryScope> {
    let mut scope = MemoryScope::try_new(tenant).map_err(|error| memory_py_error(&error))?;
    if let Some(user) = user {
        scope = scope
            .try_with_user(&user)
            .map_err(|error| memory_py_error(&error))?;
    }
    if let Some(agent) = agent {
        scope = scope
            .try_with_agent(&agent)
            .map_err(|error| memory_py_error(&error))?;
    }
    if let Some(workspace) = workspace {
        scope = scope
            .try_with_workspace(&workspace)
            .map_err(|error| memory_py_error(&error))?;
    }
    Ok(scope)
}

/// Native memory composition: one store, one scope, one policy.
///
/// Construct with [`MemoryExtension.in_process`] for an ephemeral
/// process-local store, or [`MemoryExtension.sqlite`] for a durable one.
/// The three accessors return handles accepted by the agent factories'
/// `toolsets`, `context_providers`, and `observers` parameters. All handles
/// share the extension's store, so one extension can back several agents.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "MemoryExtension",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyMemoryExtension {
    store: Arc<dyn MemoryStore>,
    scope: MemoryScope,
    policy: MemoryPolicy,
}

#[pymethods]
impl PyMemoryExtension {
    /// Build an extension over a process-local, non-durable store.
    ///
    /// `tenant` must equal the tenant scope of the runs that recall from
    /// it. It defaults to `"python-local"`, the scope `Agent.run` and
    /// `Agent.start` use; pass the session's tenant scope instead when the
    /// recalling runs execute on a lane. Recall on a mismatched tenant
    /// fails the run with `context_contribution_invalid`; writes through
    /// the toolset are not affected.
    ///
    /// `user`, `agent`, and `workspace` narrow the scope that every read
    /// and write is bound to. The policy flags gate the exposed tools:
    /// `read` covers `search_memory`/`inspect_memory`, `write` covers
    /// `remember`, and `manage` covers `forget_memory`/`correct_memory`.
    #[staticmethod]
    #[pyo3(signature = (*, tenant = DEFAULT_TENANT, user = None, agent = None, workspace = None, read = true, write = true, manage = false))]
    #[pyo3(
        text_signature = "(*, tenant='python-local', user=None, agent=None, workspace=None, read=True, write=True, manage=False)"
    )]
    fn in_process(
        tenant: &str,
        user: Option<String>,
        agent: Option<String>,
        workspace: Option<String>,
        read: bool,
        write: bool,
        manage: bool,
    ) -> PyResult<Self> {
        Ok(Self {
            store: Arc::new(InProcessMemoryStore::default()),
            scope: build_scope(tenant, user, agent, workspace)?,
            policy: MemoryPolicy {
                read,
                write,
                manage,
            },
        })
    }

    /// Build an extension over a durable `SQLite` store at `path`.
    ///
    /// The file and its schema are created on first open. Parameters other
    /// than `path` match [`MemoryExtension.in_process`], including the
    /// `tenant` default and the rule that it must equal the tenant scope of
    /// the runs that recall from this store.
    #[staticmethod]
    #[pyo3(signature = (*, path, tenant = DEFAULT_TENANT, user = None, agent = None, workspace = None, read = true, write = true, manage = false))]
    #[pyo3(
        text_signature = "(*, path, tenant='python-local', user=None, agent=None, workspace=None, read=True, write=True, manage=False)"
    )]
    #[expect(
        clippy::too_many_arguments,
        reason = "scope narrowing and policy flags are distinct keyword parameters"
    )]
    fn sqlite(
        path: &str,
        tenant: &str,
        user: Option<String>,
        agent: Option<String>,
        workspace: Option<String>,
        read: bool,
        write: bool,
        manage: bool,
    ) -> PyResult<Self> {
        let scope = build_scope(tenant, user, agent, workspace)?;
        let store = SqliteMemoryStore::try_open(Path::new(path))
            .map_err(|error| memory_py_error(&error))?;
        Ok(Self {
            store: Arc::new(store),
            scope,
            policy: MemoryPolicy {
                read,
                write,
                manage,
            },
        })
    }

    /// Tenant this extension is bound to.
    #[getter]
    fn tenant(&self) -> String {
        self.scope.tenant().to_string()
    }

    /// Recall context provider handle for `context_providers=[...]`.
    ///
    /// `max_hits` bounds one recall.
    #[pyo3(signature = (*, max_hits = None))]
    fn context_provider(&self, max_hits: Option<usize>) -> PyResult<PyMemoryContextProvider> {
        Ok(PyMemoryContextProvider {
            component: component_ref(MEMORY_CONTEXT_COMPONENT, MEMORY_CONTEXT_VERSION)?,
            store: Arc::clone(&self.store),
            scope: self.scope.clone(),
            config: RecallConfig {
                max_hits: max_hits.unwrap_or(RecallConfig::default().max_hits),
            },
        })
    }

    /// Memory toolset handle for `toolsets=[...]`.
    #[pyo3(signature = (*, read = true))]
    fn toolset(&self, read: bool) -> PyResult<PyMemoryToolset> {
        Ok(PyMemoryToolset {
            component: component_ref(MEMORY_TOOLSET_COMPONENT, MEMORY_TOOLSET_VERSION)?,
            store: Arc::clone(&self.store),
            scope: self.scope.clone(),
            policy: MemoryPolicy {
                read: self.policy.read && read,
                ..self.policy
            },
        })
    }

    /// Capture observer handle for `observers=[...]`.
    ///
    /// The observer extracts candidate memories from observed run events
    /// with the rule-based extractor and writes them to the same store.
    fn observer(&self) -> PyResult<PyMemoryObserver> {
        let observer = MemoryObserver::try_new(
            Arc::clone(&self.store),
            self.scope.clone(),
            Arc::new(RuleBasedExtractor::default()),
            system_clock(),
        )
        .map_err(|error| memory_py_error(&error))?;
        Ok(PyMemoryObserver {
            inner: Arc::new(observer),
        })
    }
}

/// Recall provider handle produced by [`PyMemoryExtension::context_provider`].
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "MemoryContextProvider",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyMemoryContextProvider {
    component: ComponentRef,
    store: Arc<dyn MemoryStore>,
    scope: MemoryScope,
    config: RecallConfig,
}

#[pymethods]
impl PyMemoryContextProvider {
    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }
}

impl PyMemoryContextProvider {
    pub(crate) fn registration(
        &self,
        artifact_store: &Arc<dyn ArtifactStore>,
    ) -> PyResult<(ComponentRef, Arc<dyn ContextProvider>)> {
        let provider = MemoryContextProvider::try_new(
            Arc::clone(&self.store),
            artifact_store.as_ref(),
            self.scope.clone(),
            self.config,
        )
        .map_err(|error| memory_py_error(&error))?;
        Ok((self.component.clone(), Arc::new(provider)))
    }
}

/// Memory toolset handle produced by [`PyMemoryExtension::toolset`].
///
/// The native `MemoryToolset` is built at agent-assembly time so it can
/// share the agent's artifact store; see the module documentation.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "MemoryToolset",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyMemoryToolset {
    component: ComponentRef,
    store: Arc<dyn MemoryStore>,
    scope: MemoryScope,
    policy: MemoryPolicy,
}

#[pymethods]
impl PyMemoryToolset {
    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.component.id().to_string()
    }

    /// Number of memory tools the configured policy exposes.
    #[getter]
    fn tool_count(&self) -> usize {
        usize::from(self.policy.read) * 2
            + usize::from(self.policy.write)
            + usize::from(self.policy.manage) * 2
    }
}

impl PyMemoryToolset {
    /// Materialize the native toolset against the agent's artifact store.
    pub(crate) fn registration(
        &self,
        artifact_store: Arc<dyn ArtifactStore>,
    ) -> PyResult<(ComponentRef, Arc<dyn Toolset>)> {
        let toolset = MemoryToolset::try_new(
            Arc::clone(&self.store),
            artifact_store,
            self.scope.clone(),
            self.policy,
            system_clock(),
        )
        .map_err(|error| memory_py_error(&error))?;
        Ok((self.component.clone(), Arc::new(toolset)))
    }
}

/// Capture observer handle produced by [`PyMemoryExtension::observer`].
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "MemoryObserver",
    frozen,
    skip_from_py_object
)]
pub(crate) struct PyMemoryObserver {
    inner: Arc<MemoryObserver>,
}

#[pymethods]
impl PyMemoryObserver {
    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor().component.id().to_string()
    }
}

impl PyMemoryObserver {
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Observer>) {
        (self.inner.descriptor().component, self.inner.clone())
    }
}

impl PyMemoryExtension {
    pub(crate) fn search_store(
        &self,
        scope: &finstack_ai_search_core::SearchScope,
    ) -> PyResult<Arc<dyn MemoryStore>> {
        let bound = serde_json::to_value(&self.scope)
            .map_err(|_| memory_py_error(&"invalid memory scope"))?;
        let requested =
            serde_json::to_value(scope).map_err(|_| memory_py_error(&"invalid search scope"))?;
        if !self.policy.read || bound != requested {
            return Err(crate::search::error(
                finstack_ai_search_core::SearchError::SearchScopeDenied,
            ));
        }
        Ok(self.store.clone())
    }
}
