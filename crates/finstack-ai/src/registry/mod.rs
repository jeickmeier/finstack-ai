//! Typed extension registration and one-time resolved-agent construction.

mod errors;
mod extension;
mod registrar;
mod resolve;
mod resolved;
mod types;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

pub use errors::{
    AgentBuildError, RegisteredComponentDescriptor, RegistrationError, RegistrationEvent,
    ResolutionDiagnostic, ResolutionDiagnosticKind, ResolutionReport,
};
pub use extension::{Extension, ExtensionDescriptor, ExtensionTrust};
pub use registrar::Registrar;
pub use resolve::Registry;
pub use resolved::{
    ComponentHealthReport, ComponentShutdownOutcome, ComponentShutdownReport, ResolvedAgent,
    ResolvedComponent, ResolvedRunPlan,
};
pub use types::{
    AGENT_BUILD_CANCELLED, AGENT_BUILD_CONFIGURATION_CONFLICT, AGENT_BUILD_DUPLICATE_SELECTION,
    AGENT_BUILD_FACTORY_FAILED, AGENT_BUILD_INVALID_DESCRIPTOR, AGENT_BUILD_KIND_MISMATCH,
    AGENT_BUILD_MIDDLEWARE_INVALID, AGENT_BUILD_MISSING_COMPONENT, AGENT_BUILD_VERSION_MISMATCH,
    AgentComponentSelection, AgentConstructionContext, ComponentAlias,
    ComponentConstructionContext, ComponentFactory, ComponentHealth, ComponentKind,
    ComponentLifecycle, ComponentSelector, ConstructionError, DuplicatePolicy, LifecycleBinding,
    LifecycleError, MAX_COMPONENT_ALIASES, MAX_REGISTERED_COMPONENTS, MAX_SELECTED_COMPONENTS,
    ReadyComponent, RegistrationMetadata, ResolveRequest, ShutdownOwnership,
};
