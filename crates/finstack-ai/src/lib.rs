//! Public SDK/facade for the `finstack-ai` agent engine.
//!
//! Owns registration, one-time extension resolution, and ergonomic adapters
//! over the runtime. The default `native-tokio` feature selects the runtime
//! Tokio driver; browser WASM consumers disable defaults and enable
//! `wasm-host`.
//!
//! Start at [`Agent::builder`]. Kernel, runtime, and protocol crates are not a
//! second constructor path.
//!
//! # Module map
//!
//! - `spec` — declarative `AgentSpec` / `AgentBuilder` (data only)
//! - `agent` — live `Agent` / `NativeAgentBuilder` / `AgentRun`
//! - `session` — journaled `Session` / `Lane` handles
//! - `bundle` — catalog, exact lock, resolver
//! - `registry` — registration and one-time resolution
//! - `result` — typed decode of a committed structured result

#![warn(missing_docs)]

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
mod agent;
mod bundle;
mod registry;
mod result;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
mod session;
mod spec;

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use agent::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, ActivationHostError, Agent, AgentRun,
    AgentRunError, AgentRunOutput, AgentRunRequest, AnthropicAgentSpec,
    CAPABILITY_ACTIVATION_BOUND, CAPABILITY_ACTIVATION_FAILED, CapabilityCatalogEntry,
    E2bSandboxAgentSpec, GatewayAgentSpec, LinkedAgent, LinkedAgentPorts,
    MAX_CONCURRENT_CAPABILITY_ACTIVATIONS, NativeAgentBuilder, NativeCapabilityHost,
    OllamaAgentSpec, OpenAiAgentSpec, RemoteChildRouteSpec,
};
#[cfg(feature = "native-tokio")]
pub use agent::{
    CHILD_RUN_BRIDGE_FAILED, CHILD_RUN_BRIDGE_PLANNER_REJECTED,
    CHILD_RUN_BRIDGE_PLANNER_UNAVAILABLE, ChildEventContext, ChildEventSink, ChildPlanContext,
    ChildRunBridge, ChildRunBridgeError, ChildRunResolver, DeferredChildPlanner, DeferredPlanError,
    OutstandingDeferral,
};
pub use bundle::{
    BUNDLE_RESOLUTION_CONFLICT, BUNDLE_RESOLUTION_INVALID, BUNDLE_RESOLUTION_LOCK_MISMATCH,
    BUNDLE_RESOLUTION_MISSING, BUNDLE_SCHEMA_VERSION, BundleCatalog, BundleConflict,
    BundleDefaults, BundleRequirement, BundleResolutionError, BundleResolver, BundleSpec,
    CompatibilityRequirements, HostFeature, LockedBundle, LockedCapability, LockedComponent,
    LockedComponentKind, RequiredServices, ResolvedAgentLock, RuntimeServices, VersionRequirement,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use finstack_ai_kernel::{
    ActiveCapability, CapabilityActivationSource, InteractionRequest, InteractionResolution,
    OperationLocator, PrincipalRef, RunSecurityContext, SessionId,
};
pub use finstack_ai_runtime as runtime;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use finstack_ai_runtime::{
    ExternalIdentityKey, ExternalIdentityMap, IdentityMapError, LaneInspect,
    MemoryExternalIdentityMap, SessionError,
};
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
pub use result::{
    RESULT_DECODE_INVALID_VALUE, RESULT_DECODE_SCHEMA_MISMATCH, ResultDecodeError, RunResult,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use session::{Lane, Session};
pub use spec::{
    AGENT_SPEC_SCHEMA_VERSION, AgentBuilder, AgentSpec, AgentSpecError, CapabilityActivation,
    CapabilityRef, CapabilitySpec, ChildRunPolicy, InstructionSpec, RunPolicy,
};
