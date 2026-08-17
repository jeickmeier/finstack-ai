use std::sync::Arc;

use finstack_ai_runtime::{ComponentId, ComponentRef, SchemaRef, Version};
use thiserror::Error;

use super::extension::ExtensionDescriptor;
use super::types::{
    AGENT_BUILD_CANCELLED, AGENT_BUILD_CONFIGURATION_CONFLICT, AGENT_BUILD_DUPLICATE_SELECTION,
    AGENT_BUILD_FACTORY_FAILED, AGENT_BUILD_INVALID_DESCRIPTOR, AGENT_BUILD_KIND_MISMATCH,
    AGENT_BUILD_MIDDLEWARE_INVALID, AGENT_BUILD_MISSING_COMPONENT, AGENT_BUILD_VERSION_MISMATCH,
    ComponentAlias, ComponentKind,
};

const REGISTRATION_SOURCE_REQUIRED: &str = "registration_source_required";
const REGISTRATION_SOURCE_DUPLICATE: &str = "registration_source_duplicate";
const REGISTRATION_DUPLICATE: &str = "registration_duplicate";
const REGISTRATION_REPLACEMENT_REJECTED: &str = "registration_replacement_rejected";
const REGISTRATION_ALIAS_CONFLICT: &str = "registration_alias_conflict";
const REGISTRATION_ALIAS_LIMIT: &str = "registration_alias_limit";
const REGISTRATION_COMPONENT_LIMIT: &str = "registration_component_limit";
const REGISTRATION_REENTRANT: &str = "registration_reentrant";
const REGISTRATION_INVALID_DESCRIPTOR: &str = "registration_invalid_descriptor";

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
    #[error(
        "{}: registration requires an active extension source",
        REGISTRATION_SOURCE_REQUIRED
    )]
    SourceRequired,
    /// An extension source identity was reused.
    #[error(
        "{}: extension source {extension_source} is already registered",
        REGISTRATION_SOURCE_DUPLICATE
    )]
    SourceDuplicate {
        /// Duplicate source.
        extension_source: ComponentId,
    },
    /// Component identity was reused without explicit replacement.
    #[error(
        "{}: {component} from {attempted_source} duplicates {kind} registered by {existing_source}",
        REGISTRATION_DUPLICATE
    )]
    Duplicate {
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
    #[error(
        "{}: replacement of {component} by {attempted_source} was rejected",
        REGISTRATION_REPLACEMENT_REJECTED
    )]
    ReplacementRejected {
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
    #[error(
        "{}: alias {alias} for {component} is already owned by {existing_component}",
        REGISTRATION_ALIAS_CONFLICT
    )]
    AliasConflict {
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
    #[error(
        "{}: component {component} exceeds alias limit {limit}",
        REGISTRATION_ALIAS_LIMIT
    )]
    AliasLimit {
        /// Component.
        component: ComponentId,
        /// Limit.
        limit: usize,
    },
    /// Registry component limit was exceeded.
    #[error(
        "{}: registry exceeds component limit {limit}",
        REGISTRATION_COMPONENT_LIMIT
    )]
    ComponentLimit {
        /// Limit.
        limit: usize,
    },
    /// Nested source registration was attempted.
    #[error("{}: extension registration cannot be nested", REGISTRATION_REENTRANT)]
    Reentrant,
    /// Descriptor or alias metadata was invalid.
    #[error("{}: {message}", REGISTRATION_INVALID_DESCRIPTOR)]
    InvalidDescriptor {
        /// Safe diagnostic.
        message: Arc<str>,
    },
}

impl RegistrationError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SourceRequired => REGISTRATION_SOURCE_REQUIRED,
            Self::SourceDuplicate { .. } => REGISTRATION_SOURCE_DUPLICATE,
            Self::Duplicate { .. } => REGISTRATION_DUPLICATE,
            Self::ReplacementRejected { .. } => REGISTRATION_REPLACEMENT_REJECTED,
            Self::AliasConflict { .. } => REGISTRATION_ALIAS_CONFLICT,
            Self::AliasLimit { .. } => REGISTRATION_ALIAS_LIMIT,
            Self::ComponentLimit { .. } => REGISTRATION_COMPONENT_LIMIT,
            Self::Reentrant => REGISTRATION_REENTRANT,
            Self::InvalidDescriptor { .. } => REGISTRATION_INVALID_DESCRIPTOR,
        }
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
    #[error(
        "{}: {request_source} could not resolve {selector} as {expected_kind}",
        AGENT_BUILD_MISSING_COMPONENT
    )]
    MissingComponent {
        /// Request source.
        request_source: ComponentId,
        /// Requested id or alias.
        selector: Arc<str>,
        /// Required primary kind.
        expected_kind: ComponentKind,
    },
    /// A selected identity was registered under another primary kind.
    #[error(
        "{}: {component} from {registration_source} is {actual_kind}, not {expected_kind}",
        AGENT_BUILD_KIND_MISMATCH
    )]
    KindMismatch {
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
    #[error(
        "{}: {component} has version {actual:?}, not {required:?}",
        AGENT_BUILD_VERSION_MISMATCH
    )]
    VersionMismatch {
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
    #[error(
        "{}: {request_source} selected {component} more than once",
        AGENT_BUILD_DUPLICATE_SELECTION
    )]
    DuplicateSelection {
        /// Request source.
        request_source: ComponentId,
        /// Duplicate component.
        component: ComponentId,
    },
    /// A typed factory or model warmup failed before the first run.
    #[error(
        "{}: construction of {component} from {registration_source} failed ({failure_code}): {message}",
        AGENT_BUILD_FACTORY_FAILED
    )]
    FactoryFailed {
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
    #[error(
        "{}: construction of {component} for {request_source} was cancelled",
        AGENT_BUILD_CANCELLED
    )]
    Cancelled {
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
    },
    /// A ready handle or cached factory was requested with conflicting configuration.
    #[error(
        "{}: construction configuration conflicts for {component}",
        AGENT_BUILD_CONFIGURATION_CONFLICT
    )]
    ConfigurationConflict {
        /// Request source.
        request_source: ComponentId,
        /// Selected component.
        component: ComponentId,
        /// Registration source.
        registration_source: ComponentId,
    },
    /// A ready port's immutable descriptor disagreed with its registration.
    #[error(
        "{}: descriptor for {component} from {registration_source} is invalid: {message}",
        AGENT_BUILD_INVALID_DESCRIPTOR
    )]
    InvalidDescriptor {
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
    #[error(
        "{}: middleware resolution failed ({failure_code}): {message}",
        AGENT_BUILD_MIDDLEWARE_INVALID
    )]
    MiddlewareInvalid {
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
            Self::MissingComponent { .. } => AGENT_BUILD_MISSING_COMPONENT,
            Self::KindMismatch { .. } => AGENT_BUILD_KIND_MISMATCH,
            Self::VersionMismatch { .. } => AGENT_BUILD_VERSION_MISMATCH,
            Self::DuplicateSelection { .. } => AGENT_BUILD_DUPLICATE_SELECTION,
            Self::FactoryFailed { .. } => AGENT_BUILD_FACTORY_FAILED,
            Self::Cancelled { .. } => AGENT_BUILD_CANCELLED,
            Self::ConfigurationConflict { .. } => AGENT_BUILD_CONFIGURATION_CONFLICT,
            Self::InvalidDescriptor { .. } => AGENT_BUILD_INVALID_DESCRIPTOR,
            Self::MiddlewareInvalid { .. } => AGENT_BUILD_MIDDLEWARE_INVALID,
        }
    }
}
