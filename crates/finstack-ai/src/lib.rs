//! Public SDK/facade for the `finstack-ai` agent engine.
//!
//! Owns registration, one-time extension resolution, and ergonomic adapters
//! over the runtime. The default `native-tokio` feature selects the runtime
//! Tokio driver; browser WASM consumers disable defaults and enable
//! `wasm-host`.

#![warn(missing_docs)]

mod registry;

pub use finstack_ai_runtime as runtime;
pub use registry::{
    AGENT_BUILD_CANCELLED, AGENT_BUILD_CONFIGURATION_CONFLICT, AGENT_BUILD_DUPLICATE_SELECTION,
    AGENT_BUILD_FACTORY_FAILED, AGENT_BUILD_INVALID_DESCRIPTOR, AGENT_BUILD_KIND_MISMATCH,
    AGENT_BUILD_MIDDLEWARE_INVALID, AGENT_BUILD_MISSING_COMPONENT, AGENT_BUILD_VERSION_MISMATCH,
    AgentBuildError, AgentComponentSelection, AgentConstructionContext, ComponentAlias,
    ComponentConstructionContext, ComponentFactory, ComponentHealth, ComponentHealthReport,
    ComponentKind, ComponentLifecycle, ComponentSelector, ComponentShutdownOutcome,
    ComponentShutdownReport, ConstructionError, DuplicatePolicy, Extension, ExtensionDescriptor,
    ExtensionTrust, LifecycleBinding, LifecycleError, MAX_COMPONENT_ALIASES,
    MAX_REGISTERED_COMPONENTS, MAX_SELECTED_COMPONENTS, ReadyComponent,
    RegisteredComponentDescriptor, Registrar, RegistrationError, RegistrationEvent,
    RegistrationMetadata, Registry, ResolutionDiagnostic, ResolutionDiagnosticKind,
    ResolutionReport, ResolveRequest, ResolvedAgent, ResolvedComponent, ResolvedRunPlan,
    ShutdownOwnership,
};
