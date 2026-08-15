//! Typed extension registration and one-time resolved-agent construction.

use core::fmt;
use core::future::{Future, poll_fn};
use core::sync::atomic::{AtomicBool, Ordering};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_runtime::{
    CancellationSignal, ComponentId, ComponentRef, ContextProvider, Digest, JournalStore, Metadata,
    Middleware, MiddlewareRegistration, Model, ModelWarmupContext, Observer, PortFuture,
    PortObject, RawJson, ResolvedMiddlewareChain, SchemaRef, Timestamp, Toolset, Version,
};
use thiserror::Error;

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

const REGISTRATION_SOURCE_REQUIRED: &str = "registration_source_required";
const REGISTRATION_SOURCE_DUPLICATE: &str = "registration_source_duplicate";
const REGISTRATION_DUPLICATE: &str = "registration_duplicate";
const REGISTRATION_REPLACEMENT_REJECTED: &str = "registration_replacement_rejected";
const REGISTRATION_ALIAS_CONFLICT: &str = "registration_alias_conflict";
const REGISTRATION_ALIAS_LIMIT: &str = "registration_alias_limit";
const REGISTRATION_COMPONENT_LIMIT: &str = "registration_component_limit";
const REGISTRATION_REENTRANT: &str = "registration_reentrant";
const REGISTRATION_INVALID_DESCRIPTOR: &str = "registration_invalid_descriptor";
const COMPONENT_FACTORY_FAILED: &str = "component_factory_failed";
const COMPONENT_LIFECYCLE_FAILED: &str = "component_lifecycle_failed";

/// Maximum components retained by one registry.
pub const MAX_REGISTERED_COMPONENTS: usize = 1_024;
/// Maximum aliases retained for one component.
pub const MAX_COMPONENT_ALIASES: usize = 16;
/// Maximum selected components of one repeated kind.
pub const MAX_SELECTED_COMPONENTS: usize = 1_024;
/// Maximum diagnostic source or alias bytes.
const LABEL_MAX_BYTES: usize = 128;

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
                code: REGISTRATION_INVALID_DESCRIPTOR,
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
    id: ComponentId,
    version: Version,
    aliases: Vec<ComponentAlias>,
    configuration_schema: Option<SchemaRef>,
    duplicate_policy: DuplicatePolicy,
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
                code: REGISTRATION_ALIAS_LIMIT,
                component: self.id,
                limit: MAX_COMPONENT_ALIASES,
            });
        }
        if self.aliases.contains(&alias) {
            return Err(RegistrationError::InvalidDescriptor {
                code: REGISTRATION_INVALID_DESCRIPTOR,
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

/// Explicit trust assumption for an extension source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionTrust {
    /// The component runs in-process with the full authority of its host.
    TrustedInProcess,
}

/// Identity and trust metadata for one extension registration source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDescriptor {
    /// Namespaced extension identity.
    pub id: ComponentId,
    /// Extension semantic version.
    pub version: Version,
    /// Explicit trust assumption.
    pub trust: ExtensionTrust,
}

impl ExtensionDescriptor {
    /// Describe a trusted in-process extension source.
    #[must_use]
    pub const fn trusted_in_process(id: ComponentId, version: Version) -> Self {
        Self {
            id,
            version,
            trust: ExtensionTrust::TrustedInProcess,
        }
    }
}

/// Self-contained native or host extension that contributes typed registrations.
pub trait Extension: PortObject {
    /// Return the immutable source/trust descriptor.
    fn descriptor(&self) -> ExtensionDescriptor;

    /// Add typed registrations to the active source transaction.
    ///
    /// # Errors
    ///
    /// Returns a stable registration error. The registrar rolls back every
    /// registration made by this extension when the method fails.
    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError>;
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
    hooks: Arc<dyn ComponentLifecycle>,
    ownership: ShutdownOwnership,
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
    handle: Arc<T>,
    lifecycle: Option<LifecycleBinding>,
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

/// One successful or replacement registration event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationEvent {
    /// Registered component.
    pub component: ComponentRef,
    /// Primary port kind.
    pub kind: ComponentKind,
    /// Registration source.
    pub source: ComponentId,
    /// Replaced source when explicit replacement occurred.
    pub replaced_source: Option<ComponentId>,
}

/// Immutable metadata retained with every registered and resolved component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredComponentDescriptor {
    /// Exact registered component identity/version.
    pub component: ComponentRef,
    /// Primary port kind.
    pub kind: ComponentKind,
    /// Extension source descriptor.
    pub source: ExtensionDescriptor,
    /// Local aliases expanding to the component identity.
    pub aliases: Arc<[ComponentAlias]>,
    /// Optional canonical configuration schema reference.
    pub configuration_schema: Option<SchemaRef>,
}

/// Typed registration failure with source-aware duplicate diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistrationError {
    /// Registration method was called outside an extension source transaction.
    #[error("{code}: registration requires an active extension source")]
    SourceRequired {
        /// Stable code.
        code: &'static str,
    },
    /// An extension source identity was reused.
    #[error("{code}: extension source {extension_source} is already registered")]
    SourceDuplicate {
        /// Stable code.
        code: &'static str,
        /// Duplicate source.
        extension_source: ComponentId,
    },
    /// Component identity was reused without explicit replacement.
    #[error(
        "{code}: {component} from {attempted_source} duplicates {kind} registered by {existing_source}"
    )]
    Duplicate {
        /// Stable code.
        code: &'static str,
        /// Duplicate component.
        component: ComponentId,
        /// Existing kind.
        kind: ComponentKind,
        /// Existing source.
        existing_source: ComponentId,
        /// Attempted source.
        attempted_source: ComponentId,
    },
    /// Explicit replacement did not match the existing source or kind.
    #[error("{code}: replacement of {component} by {attempted_source} was rejected")]
    ReplacementRejected {
        /// Stable code.
        code: &'static str,
        /// Component being replaced.
        component: ComponentId,
        /// Expected existing source.
        expected_source: ComponentId,
        /// Actual existing source, when present.
        actual_source: Option<ComponentId>,
        /// Attempted source.
        attempted_source: ComponentId,
        /// Expected new kind.
        attempted_kind: ComponentKind,
        /// Actual existing kind, when present.
        actual_kind: Option<ComponentKind>,
    },
    /// Alias is already owned by a different component.
    #[error("{code}: alias {alias} for {component} is already owned by {existing_component}")]
    AliasConflict {
        /// Stable code.
        code: &'static str,
        /// Conflicting alias.
        alias: ComponentAlias,
        /// Attempted component.
        component: ComponentId,
        /// Existing component.
        existing_component: ComponentId,
        /// Existing source.
        existing_source: ComponentId,
        /// Attempted source.
        attempted_source: ComponentId,
    },
    /// Per-component alias limit was exceeded.
    #[error("{code}: component {component} exceeds alias limit {limit}")]
    AliasLimit {
        /// Stable code.
        code: &'static str,
        /// Component.
        component: ComponentId,
        /// Limit.
        limit: usize,
    },
    /// Registry component limit was exceeded.
    #[error("{code}: registry exceeds component limit {limit}")]
    ComponentLimit {
        /// Stable code.
        code: &'static str,
        /// Limit.
        limit: usize,
    },
    /// Nested source registration was attempted.
    #[error("{code}: extension registration cannot be nested")]
    Reentrant {
        /// Stable code.
        code: &'static str,
    },
    /// Descriptor or alias metadata was invalid.
    #[error("{code}: {message}")]
    InvalidDescriptor {
        /// Stable code.
        code: &'static str,
        /// Safe diagnostic.
        message: Arc<str>,
    },
}

impl RegistrationError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SourceRequired { code }
            | Self::SourceDuplicate { code, .. }
            | Self::Duplicate { code, .. }
            | Self::ReplacementRejected { code, .. }
            | Self::AliasConflict { code, .. }
            | Self::AliasLimit { code, .. }
            | Self::ComponentLimit { code, .. }
            | Self::Reentrant { code }
            | Self::InvalidDescriptor { code, .. } => code,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FactoryConfiguration {
    Empty,
    Digest(Digest),
}

impl FactoryConfiguration {
    fn from_raw(configuration: Option<&RawJson>) -> Self {
        configuration.map_or(Self::Empty, |value| Self::Digest(value.digest()))
    }
}

enum RegistrationSlot<T: ?Sized> {
    Ready {
        component: ReadyComponent<T>,
        factory_configuration: Option<FactoryConfiguration>,
        model_warmed: bool,
    },
    Factory(Arc<dyn ComponentFactory<T>>),
}

impl<T: ?Sized> Clone for RegistrationSlot<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Ready {
                component,
                factory_configuration,
                model_warmed,
            } => Self::Ready {
                component: component.clone(),
                factory_configuration: *factory_configuration,
                model_warmed: *model_warmed,
            },
            Self::Factory(factory) => Self::Factory(Arc::clone(factory)),
        }
    }
}

struct TypedRegistration<T: ?Sized> {
    descriptor: RegisteredComponentDescriptor,
    slot: RegistrationSlot<T>,
}

impl<T: ?Sized> Clone for TypedRegistration<T> {
    fn clone(&self) -> Self {
        Self {
            descriptor: self.descriptor.clone(),
            slot: self.slot.clone(),
        }
    }
}

#[derive(Clone)]
enum RegisteredEntry {
    Model(TypedRegistration<dyn Model>),
    Toolset(TypedRegistration<dyn Toolset>),
    ContextProvider(TypedRegistration<dyn ContextProvider>),
    Middleware(TypedRegistration<dyn Middleware>),
    Store(TypedRegistration<dyn JournalStore>),
    Observer(TypedRegistration<dyn Observer>),
}

impl RegisteredEntry {
    fn descriptor(&self) -> &RegisteredComponentDescriptor {
        match self {
            Self::Model(value) => &value.descriptor,
            Self::Toolset(value) => &value.descriptor,
            Self::ContextProvider(value) => &value.descriptor,
            Self::Middleware(value) => &value.descriptor,
            Self::Store(value) => &value.descriptor,
            Self::Observer(value) => &value.descriptor,
        }
    }
}

/// Mutable transaction builder for one deterministic extension registry.
#[derive(Default)]
pub struct Registrar {
    sources: BTreeMap<ComponentId, ExtensionDescriptor>,
    entries: BTreeMap<ComponentId, RegisteredEntry>,
    aliases: BTreeMap<ComponentAlias, ComponentId>,
    events: Vec<RegistrationEvent>,
    active_source: Option<ExtensionDescriptor>,
}

impl Registrar {
    /// Construct an empty registrar.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sources: BTreeMap::new(),
            entries: BTreeMap::new(),
            aliases: BTreeMap::new(),
            events: Vec::new(),
            active_source: None,
        }
    }

    /// Register one extension atomically under its declared source.
    ///
    /// # Errors
    ///
    /// Rejects duplicate sources, nested extension transactions, and any typed
    /// registration error. Failed extension transactions leave no partial state.
    pub fn register_extension(
        &mut self,
        extension: &dyn Extension,
    ) -> Result<(), RegistrationError> {
        if self.active_source.is_some() {
            return Err(RegistrationError::Reentrant {
                code: REGISTRATION_REENTRANT,
            });
        }
        let source = extension.descriptor();
        if self.sources.contains_key(&source.id) {
            return Err(RegistrationError::SourceDuplicate {
                code: REGISTRATION_SOURCE_DUPLICATE,
                extension_source: source.id,
            });
        }
        let entries = self.entries.clone();
        let aliases = self.aliases.clone();
        let event_len = self.events.len();
        self.active_source = Some(source.clone());
        let result = extension.register(self);
        self.active_source = None;
        match result {
            Ok(()) => {
                self.sources.insert(source.id.clone(), source);
                Ok(())
            }
            Err(error) => {
                self.entries = entries;
                self.aliases = aliases;
                self.events.truncate(event_len);
                Err(error)
            }
        }
    }

    /// Register one ready `Model` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn model(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Model>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Model, |descriptor| {
            RegisteredEntry::Model(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one `Model` factory.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn model_factory(
        &mut self,
        metadata: RegistrationMetadata,
        factory: Arc<dyn ComponentFactory<dyn Model>>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Model, |descriptor| {
            RegisteredEntry::Model(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Factory(factory),
            })
        })
    }

    /// Register one ready `Toolset` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn toolset(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Toolset>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Toolset, |descriptor| {
            RegisteredEntry::Toolset(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one `Toolset` factory.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn toolset_factory(
        &mut self,
        metadata: RegistrationMetadata,
        factory: Arc<dyn ComponentFactory<dyn Toolset>>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Toolset, |descriptor| {
            RegisteredEntry::Toolset(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Factory(factory),
            })
        })
    }

    /// Register one ready `ContextProvider` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn context_provider(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn ContextProvider>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::ContextProvider, |descriptor| {
            RegisteredEntry::ContextProvider(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one `ContextProvider` factory.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn context_provider_factory(
        &mut self,
        metadata: RegistrationMetadata,
        factory: Arc<dyn ComponentFactory<dyn ContextProvider>>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::ContextProvider, |descriptor| {
            RegisteredEntry::ContextProvider(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Factory(factory),
            })
        })
    }

    /// Register one ready `Middleware` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn middleware(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Middleware>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Middleware, |descriptor| {
            RegisteredEntry::Middleware(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one `Middleware` factory.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn middleware_factory(
        &mut self,
        metadata: RegistrationMetadata,
        factory: Arc<dyn ComponentFactory<dyn Middleware>>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Middleware, |descriptor| {
            RegisteredEntry::Middleware(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Factory(factory),
            })
        })
    }

    /// Register one ready `JournalStore` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn store(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn JournalStore>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Store, |descriptor| {
            RegisteredEntry::Store(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one `JournalStore` factory.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn store_factory(
        &mut self,
        metadata: RegistrationMetadata,
        factory: Arc<dyn ComponentFactory<dyn JournalStore>>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Store, |descriptor| {
            RegisteredEntry::Store(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Factory(factory),
            })
        })
    }

    /// Register one ready `Observer` handle.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn observer(
        &mut self,
        metadata: RegistrationMetadata,
        component: ReadyComponent<dyn Observer>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Observer, |descriptor| {
            RegisteredEntry::Observer(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Ready {
                    component,
                    factory_configuration: None,
                    model_warmed: false,
                },
            })
        })
    }

    /// Register one `Observer` factory.
    ///
    /// # Errors
    ///
    /// Returns a source-aware registration error for invalid, duplicate, or
    /// over-limit metadata.
    pub fn observer_factory(
        &mut self,
        metadata: RegistrationMetadata,
        factory: Arc<dyn ComponentFactory<dyn Observer>>,
    ) -> Result<(), RegistrationError> {
        self.insert(metadata, ComponentKind::Observer, |descriptor| {
            RegisteredEntry::Observer(TypedRegistration {
                descriptor,
                slot: RegistrationSlot::Factory(factory),
            })
        })
    }

    /// Freeze the registrar into a reusable registry.
    ///
    /// Selected factories are cached as ready handles by [`Registry::resolve`].
    #[must_use]
    pub fn into_registry(self) -> Registry {
        Registry {
            entries: self.entries,
            aliases: self.aliases,
            events: self.events.into(),
            #[cfg(test)]
            lookup_count: 0,
        }
    }

    fn insert(
        &mut self,
        metadata: RegistrationMetadata,
        kind: ComponentKind,
        build: impl FnOnce(RegisteredComponentDescriptor) -> RegisteredEntry,
    ) -> Result<(), RegistrationError> {
        let source = self
            .active_source
            .clone()
            .ok_or(RegistrationError::SourceRequired {
                code: REGISTRATION_SOURCE_REQUIRED,
            })?;
        let existing = self.entries.get(&metadata.id);
        let replaced_source = match (&metadata.duplicate_policy, existing) {
            (DuplicatePolicy::Reject, Some(existing)) => {
                return Err(RegistrationError::Duplicate {
                    code: REGISTRATION_DUPLICATE,
                    component: metadata.id,
                    kind: existing.descriptor().kind,
                    existing_source: existing.descriptor().source.id.clone(),
                    attempted_source: source.id,
                });
            }
            (DuplicatePolicy::Replace { expected_source }, Some(existing))
                if &existing.descriptor().source.id == expected_source
                    && existing.descriptor().kind == kind =>
            {
                Some(existing.descriptor().source.id.clone())
            }
            (DuplicatePolicy::Replace { expected_source }, existing) => {
                return Err(RegistrationError::ReplacementRejected {
                    code: REGISTRATION_REPLACEMENT_REJECTED,
                    component: metadata.id,
                    expected_source: expected_source.clone(),
                    actual_source: existing.map(|entry| entry.descriptor().source.id.clone()),
                    attempted_source: source.id,
                    attempted_kind: kind,
                    actual_kind: existing.map(|entry| entry.descriptor().kind),
                });
            }
            (DuplicatePolicy::Reject, None) => None,
        };
        if existing.is_none() && self.entries.len() >= MAX_REGISTERED_COMPONENTS {
            return Err(RegistrationError::ComponentLimit {
                code: REGISTRATION_COMPONENT_LIMIT,
                limit: MAX_REGISTERED_COMPONENTS,
            });
        }
        for alias in &metadata.aliases {
            if let Some(existing_component) = self.aliases.get(alias)
                && existing_component != &metadata.id
            {
                let existing_source = self.entries.get(existing_component).map_or_else(
                    || source.id.clone(),
                    |entry| entry.descriptor().source.id.clone(),
                );
                return Err(RegistrationError::AliasConflict {
                    code: REGISTRATION_ALIAS_CONFLICT,
                    alias: alias.clone(),
                    component: metadata.id,
                    existing_component: existing_component.clone(),
                    existing_source,
                    attempted_source: source.id,
                });
            }
        }
        if let Some(existing) = self.entries.get(&metadata.id) {
            for alias in existing.descriptor().aliases.iter() {
                self.aliases.remove(alias);
            }
        }
        let component = ComponentRef::new(metadata.id.clone(), Some(metadata.version));
        let aliases: Arc<[ComponentAlias]> = metadata.aliases.into();
        let descriptor = RegisteredComponentDescriptor {
            component: component.clone(),
            kind,
            source: source.clone(),
            aliases: Arc::clone(&aliases),
            configuration_schema: metadata.configuration_schema,
        };
        for alias in aliases.iter() {
            self.aliases.insert(alias.clone(), metadata.id.clone());
        }
        self.entries.insert(metadata.id, build(descriptor));
        self.events.push(RegistrationEvent {
            component,
            kind,
            source: source.id,
            replaced_source,
        });
        Ok(())
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
    fn display_name(&self) -> Arc<str> {
        match self {
            Self::Component(component) => Arc::from(component.id().as_str()),
            Self::Alias { alias, .. } => Arc::from(alias.as_str()),
        }
    }

    fn required_version(&self) -> Option<Version> {
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
    configurations: BTreeMap<ComponentId, RawJson>,
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
                code: AGENT_BUILD_DUPLICATE_SELECTION,
                request_source: self.source,
                component,
            });
        }
        Ok(self)
    }
}

/// Non-secret resolution event emitted in deterministic construction order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionDiagnosticKind {
    /// A local alias was expanded to a namespaced component.
    AliasExpanded,
    /// A ready registration was selected.
    ReadySelected,
    /// A selected factory constructed and cached a ready handle.
    FactoryConstructed,
    /// A prior factory result was reused.
    CachedFactoryReused,
    /// Retained for 1.0 compatibility. Resolution now fails closed instead
    /// of emitting this diagnostic.
    ReadyConfigurationIgnored,
    /// Configuration named no selected component.
    UnusedConfiguration,
}

/// One bounded source-aware resolution diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionDiagnostic {
    /// Diagnostic class.
    pub kind: ResolutionDiagnosticKind,
    /// Request source.
    pub request_source: ComponentId,
    /// Component when known.
    pub component: Option<ComponentRef>,
    /// Registration source when known.
    pub registration_source: Option<ComponentId>,
    /// Local alias when alias expansion occurred.
    pub alias: Option<ComponentAlias>,
}

/// Immutable non-secret report for one successful resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionReport {
    /// Request source.
    pub request_source: ComponentId,
    /// Deterministically ordered diagnostics.
    pub diagnostics: Arc<[ResolutionDiagnostic]>,
}

/// Typed pre-run construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentBuildError {
    /// A selected component or alias was absent.
    #[error("{code}: {request_source} could not resolve {selector} as {expected_kind}")]
    MissingComponent {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Requested id or alias.
        selector: Arc<str>,
        /// Required primary kind.
        expected_kind: ComponentKind,
    },
    /// A selected identity was registered under another primary kind.
    #[error("{code}: {component} from {registration_source} is {actual_kind}, not {expected_kind}")]
    KindMismatch {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
        /// Registration source.
        registration_source: ComponentId,
        /// Required primary kind.
        expected_kind: ComponentKind,
        /// Registered primary kind.
        actual_kind: ComponentKind,
    },
    /// An exact selected version did not match the registration.
    #[error("{code}: {component} has version {actual:?}, not {required:?}")]
    VersionMismatch {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
        /// Registration source.
        registration_source: ComponentId,
        /// Required version.
        required: Version,
        /// Registered version.
        actual: Version,
    },
    /// One exact component was selected more than once.
    #[error("{code}: {request_source} selected {component} more than once")]
    DuplicateSelection {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Duplicate component.
        component: ComponentId,
    },
    /// A typed factory or model warmup failed before the first run.
    #[error(
        "{code}: construction of {component} from {registration_source} failed ({failure_code}): {message}"
    )]
    FactoryFailed {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
        /// Registration source.
        registration_source: ComponentId,
        /// Component-provided stable failure code.
        failure_code: Arc<str>,
        /// Safe source-free failure text.
        message: Arc<str>,
    },
    /// Construction cancellation was observed.
    #[error("{code}: construction of {component} for {request_source} was cancelled")]
    Cancelled {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
    },
    /// A ready handle or cached factory was requested with conflicting configuration.
    #[error("{code}: construction configuration conflicts for {component}")]
    ConfigurationConflict {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
        /// Registration source.
        registration_source: ComponentId,
    },
    /// A ready port's immutable descriptor disagreed with its registration.
    #[error("{code}: descriptor for {component} from {registration_source} is invalid: {message}")]
    InvalidDescriptor {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
        /// Registration source.
        registration_source: ComponentId,
        /// Safe diagnostic.
        message: Arc<str>,
    },
    /// Middleware dependency/order resolution failed.
    #[error("{code}: middleware resolution failed ({failure_code}): {message}")]
    MiddlewareInvalid {
        /// Stable code.
        code: &'static str,
        /// Request source.
        request_source: ComponentId,
        /// Runtime failure code.
        failure_code: Arc<str>,
        /// Safe source-free message.
        message: Arc<str>,
    },
}

impl AgentBuildError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingComponent { code, .. }
            | Self::KindMismatch { code, .. }
            | Self::VersionMismatch { code, .. }
            | Self::DuplicateSelection { code, .. }
            | Self::FactoryFailed { code, .. }
            | Self::Cancelled { code, .. }
            | Self::ConfigurationConflict { code, .. }
            | Self::InvalidDescriptor { code, .. }
            | Self::MiddlewareInvalid { code, .. } => code,
        }
    }
}

/// Frozen registration metadata plus one direct primary-port handle.
pub struct ResolvedComponent<T: ?Sized> {
    descriptor: RegisteredComponentDescriptor,
    handle: Arc<T>,
    lifecycle: Option<LifecycleBinding>,
}

impl<T: ?Sized> ResolvedComponent<T> {
    /// Frozen registration metadata.
    #[must_use]
    pub const fn descriptor(&self) -> &RegisteredComponentDescriptor {
        &self.descriptor
    }

    /// Borrow the direct trait-object handle.
    #[must_use]
    pub const fn handle(&self) -> &Arc<T> {
        &self.handle
    }

    /// Declared lifecycle shutdown owner, when lifecycle hooks exist.
    #[must_use]
    pub fn shutdown_ownership(&self) -> Option<ShutdownOwnership> {
        self.lifecycle.as_ref().map(LifecycleBinding::ownership)
    }
}

impl<T: ?Sized> Clone for ResolvedComponent<T> {
    fn clone(&self) -> Self {
        Self {
            descriptor: self.descriptor.clone(),
            handle: Arc::clone(&self.handle),
            lifecycle: self.lifecycle.clone(),
        }
    }
}

struct ResolvedHandles {
    model: ResolvedComponent<dyn Model>,
    toolsets: Arc<[ResolvedComponent<dyn Toolset>]>,
    context_providers: Arc<[ResolvedComponent<dyn ContextProvider>]>,
    middleware: Arc<[ResolvedComponent<dyn Middleware>]>,
    middleware_chain: Arc<ResolvedMiddlewareChain>,
    store: ResolvedComponent<dyn JournalStore>,
    observers: Arc<[ResolvedComponent<dyn Observer>]>,
}

/// Immutable per-run direct-handle plan. It performs no registry lookup.
#[derive(Clone)]
pub struct ResolvedRunPlan {
    handles: Arc<ResolvedHandles>,
}

impl ResolvedRunPlan {
    /// Resolved model.
    #[must_use]
    pub fn model(&self) -> &ResolvedComponent<dyn Model> {
        &self.handles.model
    }

    /// Resolved ordered toolsets.
    #[must_use]
    pub fn toolsets(&self) -> &[ResolvedComponent<dyn Toolset>] {
        &self.handles.toolsets
    }

    /// Resolved ordered context providers.
    #[must_use]
    pub fn context_providers(&self) -> &[ResolvedComponent<dyn ContextProvider>] {
        &self.handles.context_providers
    }

    /// Resolved middleware in configuration order.
    #[must_use]
    pub fn middleware(&self) -> &[ResolvedComponent<dyn Middleware>] {
        &self.handles.middleware
    }

    /// Dependency-resolved middleware chain.
    #[must_use]
    pub fn middleware_chain(&self) -> &Arc<ResolvedMiddlewareChain> {
        &self.handles.middleware_chain
    }

    /// Resolved journal store.
    #[must_use]
    pub fn store(&self) -> &ResolvedComponent<dyn JournalStore> {
        &self.handles.store
    }

    /// Resolved ordered observers.
    #[must_use]
    pub fn observers(&self) -> &[ResolvedComponent<dyn Observer>] {
        &self.handles.observers
    }
}

struct ResolvedLifecycle {
    descriptor: RegisteredComponentDescriptor,
    binding: LifecycleBinding,
    shutdown_started: AtomicBool,
}

/// One component health result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentHealthReport {
    /// Component.
    pub component: ComponentRef,
    /// Declared shutdown owner.
    pub ownership: ShutdownOwnership,
    /// Health result.
    pub result: Result<ComponentHealth, LifecycleError>,
}

/// At-most-once lifecycle shutdown result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentShutdownOutcome {
    /// Resolved-agent-owned hook completed.
    Completed,
    /// A prior shutdown attempt already owns the hook.
    AlreadyCompleted,
    /// The embedding host owns this hook.
    HostOwned,
    /// The sole shutdown attempt failed.
    Failed(LifecycleError),
}

/// One component shutdown result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentShutdownReport {
    /// Component.
    pub component: ComponentRef,
    /// Shutdown outcome.
    pub outcome: ComponentShutdownOutcome,
}

/// Immutable resolved agent with ready direct handles and lifecycle ownership.
pub struct ResolvedAgent {
    run_plan: ResolvedRunPlan,
    report: ResolutionReport,
    lifecycles: Arc<[ResolvedLifecycle]>,
    spec: Option<Arc<crate::AgentSpec>>,
    lock: Option<Arc<crate::ResolvedAgentLock>>,
    composition: Option<Arc<crate::bundle::CompositionRecipe>>,
}

impl ResolvedAgent {
    /// Declarative agent specification when constructed through a bundle resolver.
    #[must_use]
    pub fn spec(&self) -> Option<&Arc<crate::AgentSpec>> {
        self.spec.as_ref()
    }

    /// Exact credential-free resolution lock when constructed through a bundle resolver.
    #[must_use]
    pub fn lock(&self) -> Option<&Arc<crate::ResolvedAgentLock>> {
        self.lock.as_ref()
    }

    /// Direct non-primary runtime services validated during composition.
    #[must_use]
    pub fn services(&self) -> Option<&crate::RuntimeServices> {
        self.composition.as_ref().map(|recipe| &recipe.services)
    }

    pub(crate) fn attach_composition(
        mut self,
        spec: Arc<crate::AgentSpec>,
        lock: Arc<crate::ResolvedAgentLock>,
        composition: Arc<crate::bundle::CompositionRecipe>,
    ) -> Self {
        self.spec = Some(spec);
        self.lock = Some(lock);
        self.composition = Some(composition);
        self
    }

    pub(crate) fn composition(&self) -> Option<&Arc<crate::bundle::CompositionRecipe>> {
        self.composition.as_ref()
    }
    /// Clone the immutable no-lookup run plan.
    #[must_use]
    pub fn run_plan(&self) -> ResolvedRunPlan {
        self.run_plan.clone()
    }

    /// Successful non-secret construction report.
    #[must_use]
    pub const fn resolution_report(&self) -> &ResolutionReport {
        &self.report
    }

    /// Query all registered lifecycle hooks in selected-component order.
    pub async fn health(&self) -> Arc<[ComponentHealthReport]> {
        let mut reports = Vec::with_capacity(self.lifecycles.len());
        for lifecycle in self.lifecycles.iter() {
            reports.push(ComponentHealthReport {
                component: lifecycle.descriptor.component.clone(),
                ownership: lifecycle.binding.ownership,
                result: lifecycle.binding.hooks.health().await,
            });
        }
        reports.into()
    }

    /// Shut down resolved-agent-owned hooks at most once in reverse selection order.
    pub async fn shutdown(
        &self,
        context: AgentConstructionContext,
    ) -> Arc<[ComponentShutdownReport]> {
        let mut reports = Vec::with_capacity(self.lifecycles.len());
        for lifecycle in self.lifecycles.iter().rev() {
            let outcome = if lifecycle.binding.ownership == ShutdownOwnership::Host {
                ComponentShutdownOutcome::HostOwned
            } else if lifecycle.shutdown_started.swap(true, Ordering::AcqRel) {
                ComponentShutdownOutcome::AlreadyCompleted
            } else {
                match lifecycle.binding.hooks.shutdown(context.clone()).await {
                    Ok(()) => ComponentShutdownOutcome::Completed,
                    Err(error) => ComponentShutdownOutcome::Failed(error),
                }
            };
            reports.push(ComponentShutdownReport {
                component: lifecycle.descriptor.component.clone(),
                outcome,
            });
        }
        reports.into()
    }
}

/// Reusable deterministic registry. Successful factory outputs are cached.
pub struct Registry {
    entries: BTreeMap<ComponentId, RegisteredEntry>,
    aliases: BTreeMap<ComponentAlias, ComponentId>,
    events: Arc<[RegistrationEvent]>,
    #[cfg(test)]
    lookup_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadyOutcome {
    ReadySelected,
    FactoryConstructed,
    CachedFactoryReused,
}

impl Registry {
    /// Look up one exact registered component descriptor without constructing it.
    #[must_use]
    pub fn registered_component(&self, id: &ComponentId) -> Option<&RegisteredComponentDescriptor> {
        self.entries.get(id).map(RegisteredEntry::descriptor)
    }

    /// Successful and replacement registration events in transaction order.
    #[must_use]
    pub const fn registration_events(&self) -> &Arc<[RegistrationEvent]> {
        &self.events
    }

    /// Resolve every selected component and factory before any run is accepted.
    ///
    /// Factories and model warmup are cached after their first successful
    /// construction. The host communicates a deadline and must cancel the
    /// context signal when that deadline fires.
    ///
    /// # Errors
    ///
    /// Returns a source-aware build error for missing, mismatched, duplicate,
    /// cancelled, invalid, or failed construction.
    #[expect(
        clippy::too_many_lines,
        reason = "fixed six-port resolution order is kept visible as one pre-run transaction"
    )]
    pub async fn resolve(
        &mut self,
        request: ResolveRequest,
        context: AgentConstructionContext,
    ) -> Result<ResolvedAgent, AgentBuildError> {
        let mut diagnostics = Vec::new();
        let mut selected = BTreeSet::new();
        let mut configured = BTreeSet::new();
        Self::validate_repeated_bounds(&request)?;

        let model_id = self.resolve_target(
            &request.selection.model,
            ComponentKind::Model,
            &request.source,
            &mut selected,
            &mut diagnostics,
        )?;
        let model_configuration = request.configurations.get(&model_id).cloned();
        if model_configuration.is_some() {
            configured.insert(model_id.clone());
        }
        let model = self
            .resolve_model(
                &model_id,
                model_configuration,
                &request.source,
                &context,
                &mut diagnostics,
            )
            .await?;

        let mut toolsets = Vec::with_capacity(request.selection.toolsets.len());
        for selector in request.selection.toolsets.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::Toolset,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            if configuration.is_some() {
                configured.insert(id.clone());
            }
            toolsets.push(
                self.resolve_toolset(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        let mut context_providers = Vec::with_capacity(request.selection.context_providers.len());
        for selector in request.selection.context_providers.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::ContextProvider,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            if configuration.is_some() {
                configured.insert(id.clone());
            }
            context_providers.push(
                self.resolve_context_provider(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        let mut middleware = Vec::with_capacity(request.selection.middleware.len());
        for selector in request.selection.middleware.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::Middleware,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            if configuration.is_some() {
                configured.insert(id.clone());
            }
            middleware.push(
                self.resolve_middleware(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        let store_id = self.resolve_target(
            &request.selection.store,
            ComponentKind::Store,
            &request.source,
            &mut selected,
            &mut diagnostics,
        )?;
        let store_configuration = request.configurations.get(&store_id).cloned();
        if store_configuration.is_some() {
            configured.insert(store_id.clone());
        }
        let store = self
            .resolve_store(
                &store_id,
                store_configuration,
                &request.source,
                &context,
                &mut diagnostics,
            )
            .await?;

        let mut observers = Vec::with_capacity(request.selection.observers.len());
        for selector in request.selection.observers.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::Observer,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            if configuration.is_some() {
                configured.insert(id.clone());
            }
            observers.push(
                self.resolve_observer(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        for component in request
            .configurations
            .keys()
            .filter(|component| !configured.contains(*component))
        {
            diagnostics.push(ResolutionDiagnostic {
                kind: ResolutionDiagnosticKind::UnusedConfiguration,
                request_source: request.source.clone(),
                component: Some(ComponentRef::new(component.clone(), None)),
                registration_source: None,
                alias: None,
            });
        }

        let chain = ResolvedMiddlewareChain::try_new(
            middleware
                .iter()
                .map(|component| MiddlewareRegistration {
                    middleware: Arc::clone(component.handle()),
                })
                .collect(),
        )
        .map_err(|error| AgentBuildError::MiddlewareInvalid {
            code: AGENT_BUILD_MIDDLEWARE_INVALID,
            request_source: request.source.clone(),
            failure_code: Arc::from(error.code()),
            message: error.descriptor().message,
        })?;

        let mut lifecycles = Vec::new();
        push_lifecycle(&mut lifecycles, &model);
        for component in &toolsets {
            push_lifecycle(&mut lifecycles, component);
        }
        for component in &context_providers {
            push_lifecycle(&mut lifecycles, component);
        }
        for component in &middleware {
            push_lifecycle(&mut lifecycles, component);
        }
        push_lifecycle(&mut lifecycles, &store);
        for component in &observers {
            push_lifecycle(&mut lifecycles, component);
        }

        let handles = Arc::new(ResolvedHandles {
            model,
            toolsets: toolsets.into(),
            context_providers: context_providers.into(),
            middleware: middleware.into(),
            middleware_chain: Arc::new(chain),
            store,
            observers: observers.into(),
        });
        Ok(ResolvedAgent {
            run_plan: ResolvedRunPlan { handles },
            report: ResolutionReport {
                request_source: request.source,
                diagnostics: diagnostics.into(),
            },
            lifecycles: lifecycles.into(),
            spec: None,
            lock: None,
            composition: None,
        })
    }

    fn validate_repeated_bounds(request: &ResolveRequest) -> Result<(), AgentBuildError> {
        for selectors in [
            &request.selection.toolsets,
            &request.selection.context_providers,
            &request.selection.middleware,
            &request.selection.observers,
        ] {
            if selectors.len() > MAX_SELECTED_COMPONENTS {
                return Err(AgentBuildError::DuplicateSelection {
                    code: AGENT_BUILD_DUPLICATE_SELECTION,
                    request_source: request.source.clone(),
                    component: request.source.clone(),
                });
            }
        }
        Ok(())
    }

    fn resolve_target(
        &mut self,
        selector: &ComponentSelector,
        expected_kind: ComponentKind,
        request_source: &ComponentId,
        selected: &mut BTreeSet<ComponentId>,
        diagnostics: &mut Vec<ResolutionDiagnostic>,
    ) -> Result<ComponentId, AgentBuildError> {
        #[cfg(test)]
        {
            self.lookup_count += 1;
        }
        let (component, alias) = match selector {
            ComponentSelector::Component(component) => (component.id().clone(), None),
            ComponentSelector::Alias { alias, .. } => {
                let component = self.aliases.get(alias).cloned().ok_or_else(|| {
                    AgentBuildError::MissingComponent {
                        code: AGENT_BUILD_MISSING_COMPONENT,
                        request_source: request_source.clone(),
                        selector: selector.display_name(),
                        expected_kind,
                    }
                })?;
                (component, Some(alias.clone()))
            }
        };
        let descriptor = self
            .entries
            .get(&component)
            .map(RegisteredEntry::descriptor)
            .ok_or_else(|| AgentBuildError::MissingComponent {
                code: AGENT_BUILD_MISSING_COMPONENT,
                request_source: request_source.clone(),
                selector: selector.display_name(),
                expected_kind,
            })?;
        if descriptor.kind != expected_kind {
            return Err(AgentBuildError::KindMismatch {
                code: AGENT_BUILD_KIND_MISMATCH,
                request_source: request_source.clone(),
                component,
                registration_source: descriptor.source.id.clone(),
                expected_kind,
                actual_kind: descriptor.kind,
            });
        }
        let actual = descriptor
            .component
            .version()
            .expect("registrations always retain an exact version");
        if let Some(required) = selector.required_version()
            && required != actual
        {
            return Err(AgentBuildError::VersionMismatch {
                code: AGENT_BUILD_VERSION_MISMATCH,
                request_source: request_source.clone(),
                component,
                registration_source: descriptor.source.id.clone(),
                required,
                actual,
            });
        }
        if !selected.insert(component.clone()) {
            return Err(AgentBuildError::DuplicateSelection {
                code: AGENT_BUILD_DUPLICATE_SELECTION,
                request_source: request_source.clone(),
                component,
            });
        }
        if let Some(alias) = alias {
            diagnostics.push(ResolutionDiagnostic {
                kind: ResolutionDiagnosticKind::AliasExpanded,
                request_source: request_source.clone(),
                component: Some(descriptor.component.clone()),
                registration_source: Some(descriptor.source.id.clone()),
                alias: Some(alias),
            });
        }
        Ok(component)
    }

    async fn resolve_model(
        &mut self,
        id: &ComponentId,
        configuration: Option<RawJson>,
        source: &ComponentId,
        context: &AgentConstructionContext,
        diagnostics: &mut Vec<ResolutionDiagnostic>,
    ) -> Result<ResolvedComponent<dyn Model>, AgentBuildError> {
        let Some(RegisteredEntry::Model(registration)) = self.entries.get_mut(id) else {
            unreachable!("kind checked before typed resolution");
        };
        let outcome = ensure_ready(
            &registration.descriptor,
            &mut registration.slot,
            configuration,
            source,
            context,
        )
        .await?;
        let RegistrationSlot::Ready {
            component,
            model_warmed,
            ..
        } = &registration.slot
        else {
            unreachable!("factory cached before success");
        };
        let (ready, warmed) = (component.clone(), *model_warmed);
        validate_model_descriptor(&registration.descriptor, ready.handle(), source)?;
        if !warmed {
            let warmup = ready.handle().warmup(ModelWarmupContext {
                cancellation: context.cancellation.child(),
                deadline: context.deadline,
                metadata: context.metadata.clone(),
            });
            match await_or_cancel(warmup, context.cancellation.clone()).await {
                Ok(Ok(())) => {
                    if let RegistrationSlot::Ready { model_warmed, .. } = &mut registration.slot {
                        *model_warmed = true;
                    }
                }
                Ok(Err(error)) => {
                    return Err(AgentBuildError::FactoryFailed {
                        code: AGENT_BUILD_FACTORY_FAILED,
                        request_source: source.clone(),
                        component: id.clone(),
                        registration_source: registration.descriptor.source.id.clone(),
                        failure_code: Arc::from(error.code()),
                        message: Arc::from(error.message()),
                    });
                }
                Err(()) => {
                    return Err(AgentBuildError::Cancelled {
                        code: AGENT_BUILD_CANCELLED,
                        request_source: source.clone(),
                        component: id.clone(),
                    });
                }
            }
        }
        push_resolution_diagnostic(diagnostics, source, &registration.descriptor, outcome);
        Ok(resolved(&registration.descriptor, ready))
    }
}

macro_rules! resolve_port {
    ($method:ident, $variant:ident, $trait:path, $validate:ident) => {
        impl Registry {
            async fn $method(
                &mut self,
                id: &ComponentId,
                configuration: Option<RawJson>,
                source: &ComponentId,
                context: &AgentConstructionContext,
                diagnostics: &mut Vec<ResolutionDiagnostic>,
            ) -> Result<ResolvedComponent<dyn $trait>, AgentBuildError> {
                let Some(RegisteredEntry::$variant(registration)) = self.entries.get_mut(id) else {
                    unreachable!("kind checked before typed resolution");
                };
                let outcome = ensure_ready(
                    &registration.descriptor,
                    &mut registration.slot,
                    configuration,
                    source,
                    context,
                )
                .await?;
                let RegistrationSlot::Ready { component, .. } = &registration.slot else {
                    unreachable!("factory cached before success");
                };
                let ready = component.clone();
                $validate(&registration.descriptor, ready.handle(), source)?;
                push_resolution_diagnostic(diagnostics, source, &registration.descriptor, outcome);
                Ok(resolved(&registration.descriptor, ready))
            }
        }
    };
}

resolve_port!(
    resolve_toolset,
    Toolset,
    Toolset,
    validate_toolset_descriptor
);
resolve_port!(
    resolve_context_provider,
    ContextProvider,
    ContextProvider,
    validate_context_descriptor
);
resolve_port!(
    resolve_middleware,
    Middleware,
    Middleware,
    validate_middleware_descriptor
);
resolve_port!(
    resolve_store,
    Store,
    JournalStore,
    validate_store_descriptor
);
resolve_port!(
    resolve_observer,
    Observer,
    Observer,
    validate_observer_descriptor
);

async fn ensure_ready<T: ?Sized + 'static>(
    descriptor: &RegisteredComponentDescriptor,
    slot: &mut RegistrationSlot<T>,
    configuration: Option<RawJson>,
    request_source: &ComponentId,
    context: &AgentConstructionContext,
) -> Result<ReadyOutcome, AgentBuildError> {
    let requested_configuration = FactoryConfiguration::from_raw(configuration.as_ref());
    match slot {
        RegistrationSlot::Ready {
            factory_configuration: Some(cached),
            ..
        } => {
            if *cached != requested_configuration {
                return Err(AgentBuildError::ConfigurationConflict {
                    code: AGENT_BUILD_CONFIGURATION_CONFLICT,
                    request_source: request_source.clone(),
                    component: descriptor.component.id().clone(),
                    registration_source: descriptor.source.id.clone(),
                });
            }
            Ok(ReadyOutcome::CachedFactoryReused)
        }
        RegistrationSlot::Ready {
            factory_configuration: None,
            ..
        } => {
            if !matches!(requested_configuration, FactoryConfiguration::Empty) {
                return Err(AgentBuildError::ConfigurationConflict {
                    code: AGENT_BUILD_CONFIGURATION_CONFLICT,
                    request_source: request_source.clone(),
                    component: descriptor.component.id().clone(),
                    registration_source: descriptor.source.id.clone(),
                });
            }
            Ok(ReadyOutcome::ReadySelected)
        }
        RegistrationSlot::Factory(factory) => {
            if context.cancellation.is_cancelled() {
                return Err(AgentBuildError::Cancelled {
                    code: AGENT_BUILD_CANCELLED,
                    request_source: request_source.clone(),
                    component: descriptor.component.id().clone(),
                });
            }
            let factory = Arc::clone(factory);
            let component_context = ComponentConstructionContext {
                component: descriptor.component.clone(),
                configuration,
                cancellation: context.cancellation.child(),
                deadline: context.deadline,
                metadata: context.metadata.clone(),
            };
            let ready = match await_or_cancel(
                factory.construct(component_context),
                context.cancellation.clone(),
            )
            .await
            {
                Ok(Ok(ready)) => ready,
                Ok(Err(error)) => {
                    return Err(AgentBuildError::FactoryFailed {
                        code: AGENT_BUILD_FACTORY_FAILED,
                        request_source: request_source.clone(),
                        component: descriptor.component.id().clone(),
                        registration_source: descriptor.source.id.clone(),
                        failure_code: Arc::from(error.code()),
                        message: Arc::from(error.message()),
                    });
                }
                Err(()) => {
                    return Err(AgentBuildError::Cancelled {
                        code: AGENT_BUILD_CANCELLED,
                        request_source: request_source.clone(),
                        component: descriptor.component.id().clone(),
                    });
                }
            };
            *slot = RegistrationSlot::Ready {
                component: ready,
                factory_configuration: Some(requested_configuration),
                model_warmed: false,
            };
            Ok(ReadyOutcome::FactoryConstructed)
        }
    }
}

async fn await_or_cancel<T>(
    future: PortFuture<T>,
    cancellation: CancellationSignal,
) -> Result<T, ()> {
    let mut future = future;
    let mut cancelled = Box::pin(cancellation.cancelled());
    poll_fn(|context| {
        if let core::task::Poll::Ready(value) = future.as_mut().poll(context) {
            return core::task::Poll::Ready(Ok(value));
        }
        if cancelled.as_mut().poll(context).is_ready() {
            return core::task::Poll::Ready(Err(()));
        }
        core::task::Poll::Pending
    })
    .await
}

fn resolved<T: ?Sized>(
    descriptor: &RegisteredComponentDescriptor,
    ready: ReadyComponent<T>,
) -> ResolvedComponent<T> {
    ResolvedComponent {
        descriptor: descriptor.clone(),
        handle: ready.handle,
        lifecycle: ready.lifecycle,
    }
}

fn push_lifecycle<T: ?Sized>(
    lifecycles: &mut Vec<ResolvedLifecycle>,
    component: &ResolvedComponent<T>,
) {
    if let Some(binding) = &component.lifecycle {
        lifecycles.push(ResolvedLifecycle {
            descriptor: component.descriptor.clone(),
            binding: binding.clone(),
            shutdown_started: AtomicBool::new(false),
        });
    }
}

fn push_resolution_diagnostic(
    diagnostics: &mut Vec<ResolutionDiagnostic>,
    request_source: &ComponentId,
    descriptor: &RegisteredComponentDescriptor,
    outcome: ReadyOutcome,
) {
    diagnostics.push(ResolutionDiagnostic {
        kind: match outcome {
            ReadyOutcome::ReadySelected => ResolutionDiagnosticKind::ReadySelected,
            ReadyOutcome::FactoryConstructed => ResolutionDiagnosticKind::FactoryConstructed,
            ReadyOutcome::CachedFactoryReused => ResolutionDiagnosticKind::CachedFactoryReused,
        },
        request_source: request_source.clone(),
        component: Some(descriptor.component.clone()),
        registration_source: Some(descriptor.source.id.clone()),
        alias: None,
    });
}

fn invalid_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    request_source: &ComponentId,
    message: impl Into<Arc<str>>,
) -> AgentBuildError {
    AgentBuildError::InvalidDescriptor {
        code: AGENT_BUILD_INVALID_DESCRIPTOR,
        request_source: request_source.clone(),
        component: descriptor.component.id().clone(),
        registration_source: descriptor.source.id.clone(),
        message: message.into(),
    }
}

fn validate_model_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Model>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    handle
        .descriptor()
        .validate()
        .map_err(|error| invalid_descriptor(descriptor, request_source, error.message()))
}

fn validate_toolset_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Toolset>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let name = handle.descriptor().name;
    if name.is_empty() || name.len() > LABEL_MAX_BYTES || name.as_bytes().contains(&0) {
        return Err(invalid_descriptor(
            descriptor,
            request_source,
            "toolset descriptor name is invalid",
        ));
    }
    Ok(())
}

fn validate_context_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn ContextProvider>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let invocation = handle.descriptor().invocation;
    validate_invocation(
        descriptor,
        &invocation.component,
        invocation.version,
        request_source,
    )
}

fn validate_middleware_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Middleware>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let invocation = handle.descriptor().invocation;
    validate_invocation(
        descriptor,
        &invocation.component,
        invocation.version,
        request_source,
    )
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the store port has no identity descriptor; the shared resolver validator remains typed"
)]
fn validate_store_descriptor(
    _descriptor: &RegisteredComponentDescriptor,
    _handle: &Arc<dyn JournalStore>,
    _request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    Ok(())
}

fn validate_observer_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Observer>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let actual = handle.descriptor().component;
    if actual != descriptor.component {
        return Err(invalid_descriptor(
            descriptor,
            request_source,
            "observer descriptor identity does not match its registration",
        ));
    }
    Ok(())
}

fn validate_invocation(
    descriptor: &RegisteredComponentDescriptor,
    component: &ComponentId,
    version: Version,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    if component != descriptor.component.id() || descriptor.component.version() != Some(version) {
        return Err(invalid_descriptor(
            descriptor,
            request_source,
            "port invocation identity does not match its registration",
        ));
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use finstack_ai_runtime::{
        Model, ModelContextProfile, ModelName, Observer, ObserverDescriptor, ObserverPayloadMode,
        TokenEstimatorRef, TokenEstimatorSource,
    };
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
    use finstack_ai_test::ScriptedModel;

    use super::*;

    const VERSION: Version = Version {
        major: 1,
        minor: 0,
        patch: 0,
    };

    fn component(value: &str) -> ComponentId {
        ComponentId::parse(value).expect("valid namespaced test component")
    }

    fn exact(value: &str) -> ComponentSelector {
        ComponentSelector::Component(ComponentRef::new(component(value), Some(VERSION)))
    }

    fn profile() -> ModelContextProfile {
        ModelContextProfile {
            provider: Arc::from("scripted"),
            model: ModelName::try_new("scripted-1").expect("valid model name"),
            hard_input_bytes: 1_000,
            context_window_tokens: 100,
            max_output_tokens: 20,
            reserved_output_tokens: 20,
            provider_overhead_tokens: 5,
            estimator: TokenEstimatorRef {
                id: Arc::from("bytes-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        }
    }

    fn model() -> Arc<ScriptedModel> {
        Arc::new(ScriptedModel::from_plans(profile(), Vec::new()))
    }

    fn store() -> Arc<MemoryJournalStore> {
        Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 16,
                records_per_session: 64,
                snapshot_bytes: 4_096,
            })
            .expect("valid memory store limits"),
        )
    }

    struct ModelExtension {
        source: ComponentId,
        id: ComponentId,
        alias: Option<ComponentAlias>,
        model: Arc<ScriptedModel>,
    }

    impl Extension for ModelExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            ExtensionDescriptor::trusted_in_process(self.source.clone(), VERSION)
        }

        fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
            let mut metadata = RegistrationMetadata::new(self.id.clone(), VERSION);
            if let Some(alias) = &self.alias {
                metadata = metadata.with_alias(alias.clone())?;
            }
            let handle: Arc<dyn Model> = self.model.clone();
            registrar.model(metadata, ReadyComponent::new(handle))
        }
    }

    struct StoreExtension {
        source: ComponentId,
        id: ComponentId,
        store: Arc<MemoryJournalStore>,
    }

    impl Extension for StoreExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            ExtensionDescriptor::trusted_in_process(self.source.clone(), VERSION)
        }

        fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
            let handle: Arc<dyn JournalStore> = self.store.clone();
            registrar.store(
                RegistrationMetadata::new(self.id.clone(), VERSION),
                ReadyComponent::new(handle),
            )
        }
    }

    fn extensions() -> (ModelExtension, StoreExtension) {
        (
            ModelExtension {
                source: component("test.extension.model"),
                id: component("test.model.scripted"),
                alias: Some(ComponentAlias::try_new("primary").expect("valid alias")),
                model: model(),
            },
            StoreExtension {
                source: component("test.extension.store"),
                id: component("test.store.memory"),
                store: store(),
            },
        )
    }

    fn request_with_model(model: ComponentSelector) -> ResolveRequest {
        ResolveRequest::new(
            component("test.agent.source"),
            AgentComponentSelection::new(model, exact("test.store.memory")),
        )
    }

    fn bundle_spec(
        agent: crate::AgentSpec,
        capability: crate::CapabilitySpec,
        requirements: Vec<crate::BundleRequirement>,
        conflicts: Vec<crate::BundleConflict>,
    ) -> crate::BundleSpec {
        crate::BundleSpec {
            schema_version: crate::BUNDLE_SCHEMA_VERSION,
            id: finstack_ai_runtime::BundleId::parse("test.bundle.composition").expect("bundle id"),
            version: VERSION,
            agents: Arc::from([agent]),
            capabilities: Arc::from([capability]),
            requirements: requirements.into(),
            conflicts: conflicts.into(),
            defaults: crate::BundleDefaults::default(),
            config_schema: None,
            compatibility: crate::CompatibilityRequirements::default(),
        }
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one vertical acceptance fixture covers resolve, activation rebuild, digest invalidation, and exact lock re-import"
    )]
    async fn bundle_resolution_locks_capabilities_and_rebuilds_application_plan() {
        let (model_extension, store_extension) = extensions();
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&model_extension)
            .expect("model extension");
        registrar
            .register_extension(&store_extension)
            .expect("store extension");
        let mut registry = registrar.into_registry();

        let capability_id = finstack_ai_runtime::CapabilityId::parse("test.capability.research")
            .expect("capability id");
        let agent = crate::AgentBuilder::new(
            finstack_ai_runtime::AgentId::parse("test.agent.research").expect("agent id"),
            ComponentRef::new(component("test.model.scripted"), Some(VERSION)),
            ComponentRef::new(component("test.store.memory"), Some(VERSION)),
        )
        .capabilities(Arc::from([crate::CapabilityRef {
            id: capability_id.clone(),
            bundle: None,
        }]))
        .build()
        .expect("agent spec");
        let capability = crate::CapabilitySpec {
            id: capability_id.clone(),
            description: Arc::from("Research instruction set"),
            instructions: Arc::from([
                crate::InstructionSpec::try_new("Cite primary sources.").expect("instruction")
            ]),
            toolsets: Arc::from([]),
            context_providers: Arc::from([]),
            middleware: Arc::from([]),
            activation: crate::CapabilityActivation::Application,
        };
        let bundle = bundle_spec(
            agent,
            capability,
            vec![
                crate::BundleRequirement::RequiredComponent {
                    component: component("test.model.scripted"),
                    version: crate::VersionRequirement::Exact { version: VERSION },
                },
                crate::BundleRequirement::RequiredComponent {
                    component: component("test.store.memory"),
                    version: crate::VersionRequirement::CompatibleMajor { major: 1 },
                },
            ],
            vec![],
        );
        let bundle_id = bundle.id.clone();
        let agent_id = bundle.agents[0].id.clone();
        let mut catalog = crate::BundleCatalog::default();
        catalog.install(bundle).expect("install bundle");
        let bundle_resolver = crate::BundleResolver::new(
            &catalog,
            Version {
                major: 0,
                minor: 0,
                patch: 1,
            },
            BTreeSet::new(),
            crate::RuntimeServices::default(),
        );
        let bundle_agent = bundle_resolver
            .resolve_agent(
                &mut registry,
                &bundle_id,
                &agent_id,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .expect("resolve bundle agent");
        let base_lock = bundle_agent.lock().expect("base lock");
        assert_eq!(base_lock.capabilities.len(), 1);
        assert!(!base_lock.capabilities[0].active);
        let base_fingerprint = base_lock.fingerprint().expect("base fingerprint");

        let activated = bundle_resolver
            .activate_application(
                &mut registry,
                &bundle_agent,
                [capability_id],
                AgentConstructionContext::new(),
            )
            .await
            .expect("activate application capability");
        let activated_lock = activated.lock().expect("activated lock");
        assert!(activated_lock.capabilities[0].active);
        assert_ne!(
            activated_lock.fingerprint().expect("activated fingerprint"),
            base_fingerprint,
            "active capability membership must invalidate plan/checkpoint digests"
        );

        let exported = activated_lock.to_json().expect("lock export");
        let imported = crate::ResolvedAgentLock::from_json(&exported).expect("lock import");

        let mut wrong_config = imported.clone();
        wrong_config.effective_config_digest = Digest::raw_json(b"wrong-config");
        assert!(
            bundle_resolver
                .resolve_lock(
                    &mut registry,
                    &wrong_config,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "effective configuration selection is exact"
        );

        let mut wrong_schema = imported.clone();
        wrong_schema.schema_digests = Arc::from([Digest::raw_json(b"wrong-schema")]);
        assert!(
            bundle_resolver
                .resolve_lock(
                    &mut registry,
                    &wrong_schema,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "schema selection is exact"
        );

        let mut wrong_version = imported.clone();
        let mut components = wrong_version.components.to_vec();
        let selected = &components[0].component;
        components[0].component = ComponentRef::new(
            selected.id().clone(),
            Some(Version {
                major: 9,
                minor: 0,
                patch: 0,
            }),
        );
        wrong_version.components = components.into();
        assert!(
            bundle_resolver
                .resolve_lock(
                    &mut registry,
                    &wrong_version,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "component version selection is exact"
        );

        let reconstructed = bundle_resolver
            .resolve_lock(
                &mut registry,
                &imported,
                BTreeMap::new(),
                AgentConstructionContext::new(),
            )
            .await
            .expect("exact lock reconstruction");
        assert_eq!(reconstructed.lock().expect("lock").as_ref(), &imported);
    }

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one negative fixture proves conflict, missing service, and unresolved agent admission failures"
    )]
    async fn bundle_conflicts_unresolved_refs_and_missing_services_fail_before_start() {
        let (model_extension, store_extension) = extensions();
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&model_extension)
            .expect("model extension");
        registrar
            .register_extension(&store_extension)
            .expect("store extension");
        let mut registry = registrar.into_registry();
        let capability = crate::CapabilitySpec {
            id: finstack_ai_runtime::CapabilityId::parse("test.capability.required")
                .expect("capability"),
            description: Arc::from("Required capability"),
            instructions: Arc::from([]),
            toolsets: Arc::from([]),
            context_providers: Arc::from([]),
            middleware: Arc::from([]),
            activation: crate::CapabilityActivation::Always,
        };
        let agent = crate::AgentBuilder::new(
            finstack_ai_runtime::AgentId::parse("test.agent.required").expect("agent"),
            ComponentRef::new(component("test.model.scripted"), Some(VERSION)),
            ComponentRef::new(component("test.store.memory"), Some(VERSION)),
        )
        .build()
        .expect("agent spec");

        let conflicting = bundle_spec(
            agent.clone(),
            capability.clone(),
            vec![],
            vec![crate::BundleConflict::Component {
                component: component("test.model.scripted"),
            }],
        );
        let conflict_id = conflicting.id.clone();
        let conflict_agent = conflicting.agents[0].id.clone();
        let mut conflict_catalog = crate::BundleCatalog::default();
        conflict_catalog
            .install(conflicting)
            .expect("conflict bundle installs");
        let conflict_resolver = crate::BundleResolver::new(
            &conflict_catalog,
            VERSION,
            BTreeSet::new(),
            crate::RuntimeServices::default(),
        );
        assert!(
            conflict_resolver
                .resolve_agent(
                    &mut registry,
                    &conflict_id,
                    &conflict_agent,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err()
        );

        let incompatible_bundle = bundle_spec(
            agent.clone(),
            capability.clone(),
            vec![crate::BundleRequirement::RequiredComponent {
                component: component("test.model.scripted"),
                version: crate::VersionRequirement::Exact {
                    version: Version {
                        major: 2,
                        minor: 0,
                        patch: 0,
                    },
                },
            }],
            vec![],
        );
        let incompatible_id = incompatible_bundle.id.clone();
        let incompatible_agent = incompatible_bundle.agents[0].id.clone();
        let mut incompatible_catalog = crate::BundleCatalog::default();
        incompatible_catalog
            .install(incompatible_bundle)
            .expect("incompatible bundle installs");
        let incompatible_resolver = crate::BundleResolver::new(
            &incompatible_catalog,
            VERSION,
            BTreeSet::new(),
            crate::RuntimeServices::default(),
        );
        assert!(
            incompatible_resolver
                .resolve_agent(
                    &mut registry,
                    &incompatible_id,
                    &incompatible_agent,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "incompatible component versions must fail before start"
        );

        let mut unresolved_capability_agent = agent.clone();
        unresolved_capability_agent.capabilities = Arc::from([crate::CapabilityRef {
            id: finstack_ai_runtime::CapabilityId::parse("test.capability.missing")
                .expect("missing capability"),
            bundle: None,
        }]);
        let unresolved_capability_bundle = bundle_spec(
            unresolved_capability_agent,
            capability.clone(),
            vec![],
            vec![],
        );
        let unresolved_capability_id = unresolved_capability_bundle.id.clone();
        let unresolved_capability_agent_id = unresolved_capability_bundle.agents[0].id.clone();
        let mut unresolved_capability_catalog = crate::BundleCatalog::default();
        unresolved_capability_catalog
            .install(unresolved_capability_bundle)
            .expect("unresolved capability bundle installs");
        let unresolved_capability_resolver = crate::BundleResolver::new(
            &unresolved_capability_catalog,
            VERSION,
            BTreeSet::new(),
            crate::RuntimeServices::default(),
        );
        assert!(
            unresolved_capability_resolver
                .resolve_agent(
                    &mut registry,
                    &unresolved_capability_id,
                    &unresolved_capability_agent_id,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "explicit capability refs must resolve to a locked definition"
        );

        let mut missing_store_agent = agent.clone();
        missing_store_agent.store = None;
        let missing_store_bundle =
            bundle_spec(missing_store_agent, capability.clone(), vec![], vec![]);
        let missing_store_id = missing_store_bundle.id.clone();
        let missing_store_agent_id = missing_store_bundle.agents[0].id.clone();
        let mut missing_store_catalog = crate::BundleCatalog::default();
        missing_store_catalog
            .install(missing_store_bundle)
            .expect("declarative bundle installs");
        let missing_store_resolver = crate::BundleResolver::new(
            &missing_store_catalog,
            VERSION,
            BTreeSet::new(),
            crate::RuntimeServices::default(),
        );
        assert!(
            missing_store_resolver
                .resolve_agent(
                    &mut registry,
                    &missing_store_id,
                    &missing_store_agent_id,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "an executable resolved agent requires a journal store"
        );

        let service_bundle = bundle_spec(
            agent.clone(),
            capability.clone(),
            vec![crate::BundleRequirement::RequiredHostFeature {
                feature: crate::HostFeature::BudgetLedger,
            }],
            vec![],
        );
        let service_id = service_bundle.id.clone();
        let service_agent = service_bundle.agents[0].id.clone();
        let mut service_catalog = crate::BundleCatalog::default();
        service_catalog
            .install(service_bundle)
            .expect("service bundle installs");
        let service_resolver = crate::BundleResolver::new(
            &service_catalog,
            VERSION,
            BTreeSet::from([crate::HostFeature::BudgetLedger]),
            crate::RuntimeServices::default(),
        );
        assert!(
            service_resolver
                .resolve_agent(
                    &mut registry,
                    &service_id,
                    &service_agent,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "required ledger must not be treated as optional"
        );
        assert!(
            service_resolver
                .resolve_agent(
                    &mut registry,
                    &service_id,
                    &finstack_ai_runtime::AgentId::parse("test.agent.missing")
                        .expect("missing agent id"),
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "unresolved agent refs must fail before start"
        );

        let artifact_bundle = bundle_spec(
            agent,
            capability,
            vec![crate::BundleRequirement::RequiredHostFeature {
                feature: crate::HostFeature::ArtifactStore,
            }],
            vec![],
        );
        let artifact_id = artifact_bundle.id.clone();
        let artifact_agent = artifact_bundle.agents[0].id.clone();
        let mut artifact_catalog = crate::BundleCatalog::default();
        artifact_catalog
            .install(artifact_bundle)
            .expect("artifact bundle installs");
        let artifact_resolver = crate::BundleResolver::new(
            &artifact_catalog,
            VERSION,
            BTreeSet::from([crate::HostFeature::ArtifactStore]),
            crate::RuntimeServices::default(),
        );
        assert!(
            artifact_resolver
                .resolve_agent(
                    &mut registry,
                    &artifact_id,
                    &artifact_agent,
                    BTreeMap::new(),
                    AgentConstructionContext::new(),
                )
                .await
                .is_err(),
            "required artifact storage must not be treated as optional"
        );
    }

    #[tokio::test]
    async fn duplicate_registration_and_missing_resolution_are_source_aware() {
        let (first, store_extension) = extensions();
        let duplicate = ModelExtension {
            source: component("test.extension.duplicate"),
            id: first.id.clone(),
            alias: None,
            model: model(),
        };
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&first)
            .expect("first extension registers");
        let error = registrar
            .register_extension(&duplicate)
            .expect_err("duplicate component must fail");
        match error {
            RegistrationError::Duplicate {
                existing_source,
                attempted_source,
                ..
            } => {
                assert_eq!(existing_source, component("test.extension.model"));
                assert_eq!(attempted_source, component("test.extension.duplicate"));
            }
            other => panic!("unexpected duplicate error: {other:?}"),
        }
        registrar
            .register_extension(&store_extension)
            .expect("store extension registers");
        let mut registry = registrar.into_registry();
        let missing = request_with_model(exact("test.model.missing"));
        let Err(error) = registry
            .resolve(missing, AgentConstructionContext::new())
            .await
        else {
            panic!("missing model must fail");
        };
        match error {
            AgentBuildError::MissingComponent {
                request_source,
                selector,
                expected_kind,
                ..
            } => {
                assert_eq!(request_source, component("test.agent.source"));
                assert_eq!(&*selector, "test.model.missing");
                assert_eq!(expected_kind, ComponentKind::Model);
            }
            other => panic!("unexpected build error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolution_is_stable_across_registration_order_and_expands_aliases() {
        let (model_extension, store_extension) = extensions();
        let mut forward = Registrar::new();
        forward
            .register_extension(&model_extension)
            .expect("model registers");
        forward
            .register_extension(&store_extension)
            .expect("store registers");

        let mut reverse = Registrar::new();
        reverse
            .register_extension(&store_extension)
            .expect("store registers");
        reverse
            .register_extension(&model_extension)
            .expect("model registers");

        let alias = ComponentSelector::Alias {
            alias: ComponentAlias::try_new("primary").expect("valid alias"),
            version: Some(VERSION),
        };
        let first = forward
            .into_registry()
            .resolve(
                request_with_model(alias.clone()),
                AgentConstructionContext::new(),
            )
            .await
            .expect("forward resolution succeeds");
        let second = reverse
            .into_registry()
            .resolve(request_with_model(alias), AgentConstructionContext::new())
            .await
            .expect("reverse resolution succeeds");
        assert_eq!(first.resolution_report(), second.resolution_report());
        assert_eq!(
            first.resolution_report().diagnostics[0].kind,
            ResolutionDiagnosticKind::AliasExpanded
        );
    }

    #[tokio::test]
    async fn explicit_replacement_checks_and_records_the_existing_source() {
        struct ReplacementExtension {
            model: Arc<ScriptedModel>,
        }

        impl Extension for ReplacementExtension {
            fn descriptor(&self) -> ExtensionDescriptor {
                ExtensionDescriptor::trusted_in_process(
                    component("test.extension.replacement"),
                    VERSION,
                )
            }

            fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
                let model: Arc<dyn Model> = self.model.clone();
                registrar.model(
                    RegistrationMetadata::new(component("test.model.scripted"), VERSION)
                        .replacing(component("test.extension.model")),
                    ReadyComponent::new(model),
                )
            }
        }

        let (original, store_extension) = extensions();
        let replacement_model = model();
        let replacement = ReplacementExtension {
            model: Arc::clone(&replacement_model),
        };
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&original)
            .expect("original model registers");
        registrar
            .register_extension(&replacement)
            .expect("source-checked replacement registers");
        registrar
            .register_extension(&store_extension)
            .expect("store registers");
        assert_eq!(
            registrar.events[1].replaced_source,
            Some(component("test.extension.model"))
        );

        let agent = registrar
            .into_registry()
            .resolve(
                request_with_model(exact("test.model.scripted")),
                AgentConstructionContext::new(),
            )
            .await
            .expect("replacement resolves");
        let expected: Arc<dyn Model> = replacement_model;
        assert!(Arc::ptr_eq(agent.run_plan().model().handle(), &expected));
    }

    #[tokio::test]
    async fn resolved_plan_retains_direct_handles_without_registry_lookup() {
        let (model_extension, store_extension) = extensions();
        let expected_model: Arc<dyn Model> = model_extension.model.clone();
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&model_extension)
            .expect("model registers");
        registrar
            .register_extension(&store_extension)
            .expect("store registers");
        let mut registry = registrar.into_registry();
        let agent = registry
            .resolve(
                request_with_model(exact("test.model.scripted")),
                AgentConstructionContext::new(),
            )
            .await
            .expect("resolution succeeds");
        assert_eq!(registry.lookup_count, 2);
        let plan = agent.run_plan();
        assert!(Arc::ptr_eq(plan.model().handle(), &expected_model));
        let _ = plan.model().handle().descriptor();
        let _ = plan.store().handle().health().await;
        let _ = agent.run_plan();
        assert_eq!(registry.lookup_count, 2);
    }

    #[tokio::test]
    async fn ready_model_rejects_per_request_configuration() {
        let (model_extension, store_extension) = extensions();
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&model_extension)
            .expect("model registers");
        registrar
            .register_extension(&store_extension)
            .expect("store registers");
        let Err(error) = registrar
            .into_registry()
            .resolve(
                request_with_model(exact("test.model.scripted"))
                    .with_configuration(
                        component("test.model.scripted"),
                        RawJson::parse(br#"{"temperature":0}"#).expect("config"),
                    )
                    .expect("request"),
                AgentConstructionContext::new(),
            )
            .await
        else {
            panic!("ready handle must fail closed");
        };
        assert_eq!(error.code(), AGENT_BUILD_CONFIGURATION_CONFLICT);
    }

    struct FactoryExtension {
        model: Arc<ScriptedModel>,
        constructions: Arc<AtomicUsize>,
    }

    impl Extension for FactoryExtension {
        fn descriptor(&self) -> ExtensionDescriptor {
            ExtensionDescriptor::trusted_in_process(component("test.extension.factory"), VERSION)
        }

        fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
            let model = Arc::clone(&self.model);
            let constructions = Arc::clone(&self.constructions);
            let factory: Arc<dyn ComponentFactory<dyn Model>> = Arc::new(move |_context| {
                let model = Arc::clone(&model);
                let constructions = Arc::clone(&constructions);
                async move {
                    constructions.fetch_add(1, Ordering::AcqRel);
                    let handle: Arc<dyn Model> = model;
                    Ok(ReadyComponent::new(handle))
                }
            });
            registrar.model_factory(
                RegistrationMetadata::new(component("test.model.factory"), VERSION),
                factory,
            )
        }
    }

    #[tokio::test]
    async fn selected_factory_and_model_warmup_execute_once() {
        let scripted = model();
        let constructions = Arc::new(AtomicUsize::new(0));
        let factory_extension = FactoryExtension {
            model: Arc::clone(&scripted),
            constructions: Arc::clone(&constructions),
        };
        let (_, store_extension) = extensions();
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&factory_extension)
            .expect("factory registers");
        registrar
            .register_extension(&store_extension)
            .expect("store registers");
        let mut registry = registrar.into_registry();

        for expected in [
            ResolutionDiagnosticKind::FactoryConstructed,
            ResolutionDiagnosticKind::CachedFactoryReused,
        ] {
            let agent = registry
                .resolve(
                    request_with_model(exact("test.model.factory")),
                    AgentConstructionContext::new(),
                )
                .await
                .expect("factory resolution succeeds");
            assert_eq!(agent.resolution_report().diagnostics[0].kind, expected);
            let _ = agent.run_plan().model().handle().descriptor();
        }
        assert_eq!(constructions.load(Ordering::Acquire), 1);
        assert_eq!(scripted.warmup_count(), 1);
    }

    struct CountingLifecycle {
        shutdowns: AtomicUsize,
    }

    impl ComponentLifecycle for CountingLifecycle {
        fn health(&self) -> PortFuture<Result<ComponentHealth, LifecycleError>> {
            Box::pin(async {
                Ok(ComponentHealth {
                    ready: true,
                    detail: Arc::from("ready"),
                })
            })
        }

        fn shutdown(
            &self,
            _context: AgentConstructionContext,
        ) -> PortFuture<Result<(), LifecycleError>> {
            self.shutdowns.fetch_add(1, Ordering::AcqRel);
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn resolved_agent_owns_lifecycle_shutdown_at_most_once() {
        struct LifecycleExtension {
            model: Arc<ScriptedModel>,
            lifecycle: Arc<CountingLifecycle>,
        }

        impl Extension for LifecycleExtension {
            fn descriptor(&self) -> ExtensionDescriptor {
                ExtensionDescriptor::trusted_in_process(
                    component("test.extension.lifecycle"),
                    VERSION,
                )
            }

            fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
                let model: Arc<dyn Model> = self.model.clone();
                let lifecycle: Arc<dyn ComponentLifecycle> = self.lifecycle.clone();
                registrar.model(
                    RegistrationMetadata::new(component("test.model.lifecycle"), VERSION),
                    ReadyComponent::new(model)
                        .with_lifecycle(LifecycleBinding::resolved_agent(lifecycle)),
                )
            }
        }

        let lifecycle = Arc::new(CountingLifecycle {
            shutdowns: AtomicUsize::new(0),
        });
        let extension = LifecycleExtension {
            model: model(),
            lifecycle: Arc::clone(&lifecycle),
        };
        let (_, store_extension) = extensions();
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&extension)
            .expect("lifecycle model registers");
        registrar
            .register_extension(&store_extension)
            .expect("store registers");
        let agent = registrar
            .into_registry()
            .resolve(
                request_with_model(exact("test.model.lifecycle")),
                AgentConstructionContext::new(),
            )
            .await
            .expect("resolution succeeds");

        let health = agent.health().await;
        assert_eq!(health.len(), 1);
        assert!(health[0].result.as_ref().expect("health succeeds").ready);
        assert_eq!(
            agent.shutdown(AgentConstructionContext::new()).await[0].outcome,
            ComponentShutdownOutcome::Completed
        );
        assert_eq!(
            agent.shutdown(AgentConstructionContext::new()).await[0].outcome,
            ComponentShutdownOutcome::AlreadyCompleted
        );
        assert_eq!(lifecycle.shutdowns.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn observer_descriptor_must_match_its_registration() {
        struct ObserverExtension;

        impl Extension for ObserverExtension {
            fn descriptor(&self) -> ExtensionDescriptor {
                ExtensionDescriptor::trusted_in_process(
                    component("test.extension.observer"),
                    VERSION,
                )
            }

            fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError> {
                let observer: Arc<dyn Observer> =
                    Arc::new(finstack_ai_runtime::NoopObserver::new(ObserverDescriptor {
                        component: ComponentRef::new(
                            component("test.observer.wrong"),
                            Some(VERSION),
                        ),
                        payload_mode: ObserverPayloadMode::MetadataOnly,
                        metadata: Metadata::empty(),
                    }));
                registrar.observer(
                    RegistrationMetadata::new(component("test.observer.noop"), VERSION),
                    ReadyComponent::new(observer),
                )
            }
        }

        let (model_extension, store_extension) = extensions();
        let mut registrar = Registrar::new();
        registrar
            .register_extension(&model_extension)
            .expect("model registers");
        registrar
            .register_extension(&store_extension)
            .expect("store registers");
        registrar
            .register_extension(&ObserverExtension)
            .expect("observer registers");
        let mut selection =
            AgentComponentSelection::new(exact("test.model.scripted"), exact("test.store.memory"));
        selection.observers = Arc::from([exact("test.observer.noop")]);
        let Err(error) = registrar
            .into_registry()
            .resolve(
                ResolveRequest::new(component("test.agent.source"), selection),
                AgentConstructionContext::new(),
            )
            .await
        else {
            panic!("descriptor mismatch must fail");
        };
        assert_eq!(error.code(), AGENT_BUILD_INVALID_DESCRIPTOR);
    }
}
