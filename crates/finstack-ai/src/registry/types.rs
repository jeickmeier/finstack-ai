use core::fmt;
use core::future::Future;
use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_runtime::{
    CancellationSignal, ComponentId, ComponentRef, Metadata, PortFuture, PortObject, RawJson,
    SchemaRef, Timestamp, Version,
};
use thiserror::Error;

use super::errors::{AgentBuildError, RegistrationError};

/// Stable missing-component build code.
pub const AGENT_BUILD_MISSING_COMPONENT: &str = "agent_build_missing_component";
/// Stable component-kind mismatch build code.
pub const AGENT_BUILD_KIND_MISMATCH: &str = "agent_build_kind_mismatch";
/// Stable component-version mismatch build code.
pub const AGENT_BUILD_VERSION_MISMATCH: &str = "agent_build_version_mismatch";
/// Stable duplicate-selection build code.
pub const AGENT_BUILD_DUPLICATE_SELECTION: &str = "agent_build_duplicate_selection";
/// Stable factory-construction build code.
pub const AGENT_BUILD_FACTORY_FAILED: &str = "agent_build_factory_failed";
/// Stable construction-cancellation build code.
pub const AGENT_BUILD_CANCELLED: &str = "agent_build_cancelled";
/// Stable construction-configuration conflict code.
pub const AGENT_BUILD_CONFIGURATION_CONFLICT: &str = "agent_build_configuration_conflict";
/// Stable middleware-chain construction code.
pub const AGENT_BUILD_MIDDLEWARE_INVALID: &str = "agent_build_middleware_invalid";
/// Stable invalid resolved-component descriptor code.
pub const AGENT_BUILD_INVALID_DESCRIPTOR: &str = "agent_build_invalid_descriptor";

const COMPONENT_FACTORY_FAILED: &str = "component_factory_failed";
const COMPONENT_LIFECYCLE_FAILED: &str = "component_lifecycle_failed";

/// Maximum components retained by one registry.
pub const MAX_REGISTERED_COMPONENTS: usize = 1_024;
/// Maximum aliases retained for one component.
pub const MAX_COMPONENT_ALIASES: usize = 16;
/// Maximum selected components of one repeated kind.
pub const MAX_SELECTED_COMPONENTS: usize = 1_024;
/// Maximum diagnostic source or alias bytes.
pub(super) const LABEL_MAX_BYTES: usize = 128;

/// The six primary extension kinds owned by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComponentKind {
    /// Provider-neutral model.
    Model,
    /// Coarse tool collection.
    Toolset,
    /// Context contribution provider.
    ContextProvider,
    /// Behavior-changing middleware.
    Middleware,
    /// Journal store.
    Store,
    /// Read-only observer.
    Observer,
}

impl fmt::Display for ComponentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Model => "model",
            Self::Toolset => "toolset",
            Self::ContextProvider => "context_provider",
            Self::Middleware => "middleware",
            Self::Store => "store",
            Self::Observer => "observer",
        })
    }
}

/// Validated local alias expanded to a namespaced component during resolution.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentAlias(Arc<str>);

impl ComponentAlias {
    /// Parse a local alias matching `[a-z][a-z0-9_-]{0,127}`.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::InvalidDescriptor`] for an empty, oversized,
    /// namespaced, NUL-bearing, or otherwise malformed alias.
    pub fn try_new(value: impl AsRef<str>) -> Result<Self, RegistrationError> {
        let value = value.as_ref();
        let valid = value.len() <= LABEL_MAX_BYTES
            && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
            });
        if !valid {
            return Err(RegistrationError::InvalidDescriptor {
                message: Arc::from("component alias is invalid"),
            });
        }
        Ok(Self(Arc::from(value)))
    }

    /// Borrow the local alias.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ComponentAlias {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ComponentAlias")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for ComponentAlias {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Explicit duplicate-registration behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuplicatePolicy {
    /// Reject an existing component identity.
    Reject,
    /// Replace only when the existing registration has this exact source.
    Replace {
        /// Expected current registration source.
        expected_source: ComponentId,
    },
}

/// Common metadata attached to one typed component registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationMetadata {
    pub(super) id: ComponentId,
    pub(super) version: Version,
    pub(super) aliases: Vec<ComponentAlias>,
    pub(super) configuration_schema: Option<SchemaRef>,
    pub(super) duplicate_policy: DuplicatePolicy,
}

impl RegistrationMetadata {
    /// Construct registration metadata with reject-on-duplicate behavior.
    #[must_use]
    pub fn new(id: ComponentId, version: Version) -> Self {
        Self {
            id,
            version,
            aliases: Vec::new(),
            configuration_schema: None,
            duplicate_policy: DuplicatePolicy::Reject,
        }
    }

    /// Add a validated local alias.
    ///
    /// # Errors
    ///
    /// Returns a stable registration error when the alias is duplicated or the
    /// per-component alias ceiling would be exceeded.
    pub fn with_alias(mut self, alias: ComponentAlias) -> Result<Self, RegistrationError> {
        if self.aliases.len() >= MAX_COMPONENT_ALIASES {
            return Err(RegistrationError::AliasLimit {
                component: self.id,
                limit: MAX_COMPONENT_ALIASES,
            });
        }
        if self.aliases.contains(&alias) {
            return Err(RegistrationError::InvalidDescriptor {
                message: Arc::from("component metadata contains a duplicate alias"),
            });
        }
        self.aliases.push(alias);
        Ok(self)
    }

    /// Attach the canonical configuration schema reference.
    #[must_use]
    pub fn with_configuration_schema(mut self, schema: SchemaRef) -> Self {
        self.configuration_schema = Some(schema);
        self
    }

    /// Declare an observable, source-checked replacement.
    #[must_use]
    pub fn replacing(mut self, expected_source: ComponentId) -> Self {
        self.duplicate_policy = DuplicatePolicy::Replace { expected_source };
        self
    }
}

/// Health returned by an optional long-lived component lifecycle hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentHealth {
    /// Whether the component is ready for new work.
    pub ready: bool,
    /// Bounded non-secret operational detail.
    pub detail: Arc<str>,
}

/// Stable lifecycle-hook failure safe to expose in SDK diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct LifecycleError {
    code: &'static str,
    message: Arc<str>,
}

impl LifecycleError {
    /// Construct a source-free lifecycle failure.
    #[must_use]
    pub fn failed(message: impl Into<Arc<str>>) -> Self {
        Self {
            code: COMPONENT_LIFECYCLE_FAILED,
            message: message.into(),
        }
    }

    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Safe source-free message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Context supplied to construction factories and lifecycle shutdown hooks.
#[derive(Debug, Clone)]
pub struct AgentConstructionContext {
    /// Construction cancellation signal. Hosts cancel it when the deadline fires.
    pub cancellation: CancellationSignal,
    /// Absolute semantic deadline communicated to the component.
    pub deadline: Option<Timestamp>,
    /// Bounded non-secret construction metadata.
    pub metadata: Metadata,
}

impl AgentConstructionContext {
    /// Construct an active context with no deadline or metadata.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancellation: CancellationSignal::new(),
            deadline: None,
            metadata: Metadata::empty(),
        }
    }
}

impl Default for AgentConstructionContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Component-specific construction context created by the registry.
#[derive(Debug, Clone)]
pub struct ComponentConstructionContext {
    /// Exact selected component.
    pub component: ComponentRef,
    /// Canonical non-secret configuration, when supplied.
    pub configuration: Option<RawJson>,
    /// Construction cancellation signal.
    pub cancellation: CancellationSignal,
    /// Absolute semantic deadline communicated to the component.
    pub deadline: Option<Timestamp>,
    /// Bounded non-secret construction metadata.
    pub metadata: Metadata,
}

/// Optional lifecycle hooks for a ready component.
pub trait ComponentLifecycle: PortObject {
    /// Report current readiness without exposing credentials.
    fn health(&self) -> PortFuture<Result<ComponentHealth, LifecycleError>>;

    /// Release owned resources. Implementations must be idempotent and honor
    /// the supplied cancellation/deadline context.
    fn shutdown(&self, context: AgentConstructionContext)
    -> PortFuture<Result<(), LifecycleError>>;
}

/// Who is responsible for invoking a component shutdown hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOwnership {
    /// The resolved agent invokes shutdown at most once.
    ResolvedAgent,
    /// The embedding host retains shutdown ownership.
    Host,
}

/// Lifecycle hook plus its declared shutdown owner.
#[derive(Clone)]
pub struct LifecycleBinding {
    pub(super) hooks: Arc<dyn ComponentLifecycle>,
    pub(super) ownership: ShutdownOwnership,
}

impl LifecycleBinding {
    /// Bind lifecycle hooks owned by the resolved agent.
    #[must_use]
    pub fn resolved_agent(hooks: Arc<dyn ComponentLifecycle>) -> Self {
        Self {
            hooks,
            ownership: ShutdownOwnership::ResolvedAgent,
        }
    }

    /// Bind lifecycle hooks owned by the embedding host.
    #[must_use]
    pub fn host(hooks: Arc<dyn ComponentLifecycle>) -> Self {
        Self {
            hooks,
            ownership: ShutdownOwnership::Host,
        }
    }

    /// Declared shutdown owner.
    #[must_use]
    pub const fn ownership(&self) -> ShutdownOwnership {
        self.ownership
    }
}

impl fmt::Debug for LifecycleBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LifecycleBinding")
            .field("ownership", &self.ownership)
            .finish_non_exhaustive()
    }
}

/// Ready direct handle returned by registration or a typed factory.
pub struct ReadyComponent<T: ?Sized> {
    pub(super) handle: Arc<T>,
    pub(super) lifecycle: Option<LifecycleBinding>,
}

impl<T: ?Sized> ReadyComponent<T> {
    /// Wrap one ready direct handle with host-owned resource lifetime.
    #[must_use]
    pub fn new(handle: Arc<T>) -> Self {
        Self {
            handle,
            lifecycle: None,
        }
    }

    /// Attach explicit lifecycle hooks and shutdown ownership.
    #[must_use]
    pub fn with_lifecycle(mut self, lifecycle: LifecycleBinding) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    /// Borrow the direct handle.
    #[must_use]
    pub fn handle(&self) -> &Arc<T> {
        &self.handle
    }
}

impl<T: ?Sized> Clone for ReadyComponent<T> {
    fn clone(&self) -> Self {
        Self {
            handle: Arc::clone(&self.handle),
            lifecycle: self.lifecycle.clone(),
        }
    }
}

/// Safe typed-factory construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {message}")]
pub struct ConstructionError {
    code: &'static str,
    message: Arc<str>,
}

impl ConstructionError {
    /// Construct a source-free factory failure.
    #[must_use]
    pub fn failed(message: impl Into<Arc<str>>) -> Self {
        Self {
            code: COMPONENT_FACTORY_FAILED,
            message: message.into(),
        }
    }

    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Safe source-free message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Target-correct typed factory for one primary port trait object.
pub trait ComponentFactory<T: ?Sized>: PortObject {
    /// Construct one ready direct handle.
    fn construct(
        &self,
        context: ComponentConstructionContext,
    ) -> PortFuture<Result<ReadyComponent<T>, ConstructionError>>;
}

#[cfg(not(target_arch = "wasm32"))]
impl<T, F, Fut> ComponentFactory<T> for F
where
    T: ?Sized + 'static,
    F: Fn(ComponentConstructionContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ReadyComponent<T>, ConstructionError>> + Send + 'static,
{
    fn construct(
        &self,
        context: ComponentConstructionContext,
    ) -> PortFuture<Result<ReadyComponent<T>, ConstructionError>> {
        Box::pin(self(context))
    }
}

#[cfg(target_arch = "wasm32")]
impl<T, F, Fut> ComponentFactory<T> for F
where
    T: ?Sized + 'static,
    F: Fn(ComponentConstructionContext) -> Fut + 'static,
    Fut: Future<Output = Result<ReadyComponent<T>, ConstructionError>> + 'static,
{
    fn construct(
        &self,
        context: ComponentConstructionContext,
    ) -> PortFuture<Result<ReadyComponent<T>, ConstructionError>> {
        Box::pin(self(context))
    }
}

/// Exact component identity or a local alias used during agent construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentSelector {
    /// Select a namespaced component, optionally with an exact version.
    Component(ComponentRef),
    /// Expand a local alias, optionally requiring an exact version.
    Alias {
        /// Validated local alias.
        alias: ComponentAlias,
        /// Required version, when present.
        version: Option<Version>,
    },
}

impl ComponentSelector {
    pub(super) fn display_name(&self) -> Arc<str> {
        match self {
            Self::Component(component) => Arc::from(component.id().as_str()),
            Self::Alias { alias, .. } => Arc::from(alias.as_str()),
        }
    }

    pub(super) fn required_version(&self) -> Option<Version> {
        match self {
            Self::Component(component) => component.version(),
            Self::Alias { version, .. } => *version,
        }
    }
}

/// Construction-only selection of the six primary port kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentComponentSelection {
    /// Required model.
    pub model: ComponentSelector,
    /// Ordered toolsets.
    pub toolsets: Arc<[ComponentSelector]>,
    /// Ordered context providers.
    pub context_providers: Arc<[ComponentSelector]>,
    /// Ordered middleware before dependency resolution.
    pub middleware: Arc<[ComponentSelector]>,
    /// Required journal store.
    pub store: ComponentSelector,
    /// Ordered read-only observers.
    pub observers: Arc<[ComponentSelector]>,
}

impl AgentComponentSelection {
    /// Construct a minimal selection with a model and journal store.
    #[must_use]
    pub fn new(model: ComponentSelector, store: ComponentSelector) -> Self {
        Self {
            model,
            toolsets: Arc::from([]),
            context_providers: Arc::from([]),
            middleware: Arc::from([]),
            store,
            observers: Arc::from([]),
        }
    }
}

/// One immutable agent-resolution request.
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    /// Namespaced source of this construction request.
    pub source: ComponentId,
    /// Selected primary components.
    pub selection: AgentComponentSelection,
    pub(super) configurations: BTreeMap<ComponentId, RawJson>,
}

impl ResolveRequest {
    /// Construct a request with no component configuration.
    #[must_use]
    pub fn new(source: ComponentId, selection: AgentComponentSelection) -> Self {
        Self {
            source,
            selection,
            configurations: BTreeMap::new(),
        }
    }

    /// Attach canonical configuration for one selected component.
    ///
    /// # Errors
    ///
    /// Returns a duplicate-selection error when a component is configured more
    /// than once or the selected-component ceiling would be exceeded.
    pub fn with_configuration(
        mut self,
        component: ComponentId,
        configuration: RawJson,
    ) -> Result<Self, AgentBuildError> {
        if self.configurations.len() >= MAX_SELECTED_COMPONENTS
            || self
                .configurations
                .insert(component.clone(), configuration)
                .is_some()
        {
            return Err(AgentBuildError::DuplicateSelection {
                request_source: self.source,
                component,
            });
        }
        Ok(self)
    }
}
