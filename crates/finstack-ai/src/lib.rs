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
//! - `spec` — declarative `AgentSpec` / [`AgentBuilder`] (data only; start at [`AgentSpec::builder`])
//! - `agent` — live `Agent` / [`NativeAgentBuilder`] / `AgentRun` (start at [`Agent::builder`])
//! - `session` — journaled `Session` / `Lane` handles
//! - `bundle` — catalog, exact lock, resolver
//! - `registry` — registration, factories, and one-time resolution
//! - `result` — typed decode of a committed structured result
//! - `runtime` — alias for `finstack-ai-runtime` port and driver types

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
mod agent;
mod bundle;
pub mod registry;
mod result;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
mod session;
mod spec;

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use agent::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, ActivationHostError, Agent, AgentRun,
    AgentRunError, AgentRunOutput, AgentRunRequest, AnthropicAgentSpec, AttachmentInput,
    CAPABILITY_ACTIVATION_BOUND, CAPABILITY_ACTIVATION_FAILED, CapabilityCatalogEntry,
    GatewayAgentSpec, GeminiAgentSpec, HistoryCachePolicy, LinkedAgent, LinkedAgentPorts,
    LinkedCommon, MAX_CONCURRENT_CAPABILITY_ACTIVATIONS, MAX_RUN_ATTACHMENTS, NativeAgentBuilder,
    NativeCapabilityHost, OllamaAgentSpec, OpenAiAgentSpec, OpenRouterAgentSpec,
    OpenRouterMediaToolsSpec, RemoteChildRouteSpec,
};
#[cfg(feature = "native-tokio")]
pub use agent::{
    CHILD_RUN_BRIDGE_FAILED, CHILD_RUN_BRIDGE_PLANNER_REJECTED,
    CHILD_RUN_BRIDGE_PLANNER_UNAVAILABLE, ChildEventContext, ChildEventSink, ChildPlanContext,
    ChildRunBridge, ChildRunBridgeError, ChildRunResolver, ChildSettleOutcome,
    DeferredChildPlanner, DeferredPlanError, OutstandingDeferral, outstanding_deferrals,
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
    ActiveCapability, AgentId, BundleId, CapabilityActivationSource, CapabilityId,
    CompactionAuthorization, ComponentId, ComponentRef, CostLimit, InteractionRequest,
    InteractionResolution, OperationLocator, PrincipalRef, RunLimits, RunSecurityContext,
    SessionId, Version,
};
/// Runtime port contracts and drivers (`finstack-ai-runtime`).
///
/// This crate re-exports the runtime crate so SDK consumers can name port
/// types without a second direct dependency. Kernel types stay on
/// `finstack-ai-kernel`.
pub use finstack_ai_runtime as runtime;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use finstack_ai_runtime::session::{
    ExternalIdentityKey, ExternalIdentityMap, IdentityMapError, LaneInspect,
    MemoryExternalIdentityMap, SessionError,
};
pub(crate) use registry::ReadyComponent;
pub use registry::{
    AGENT_BUILD_CANCELLED, AGENT_BUILD_CONFIGURATION_CONFLICT, AGENT_BUILD_DUPLICATE_SELECTION,
    AGENT_BUILD_FACTORY_FAILED, AGENT_BUILD_INVALID_DESCRIPTOR, AGENT_BUILD_KIND_MISMATCH,
    AGENT_BUILD_MIDDLEWARE_INVALID, AGENT_BUILD_MISSING_COMPONENT, AGENT_BUILD_VERSION_MISMATCH,
    AgentBuildError, AgentComponentSelection, AgentConstructionContext, ComponentAlias,
    ComponentKind, ComponentSelector, DuplicatePolicy, Extension, ExtensionDescriptor,
    ExtensionTrust, MAX_COMPONENT_ALIASES, MAX_REGISTERED_COMPONENTS, MAX_SELECTED_COMPONENTS,
    RegisteredComponentDescriptor, Registrar, RegistrationError, RegistrationEvent,
    RegistrationMetadata, Registry, ResolutionDiagnostic, ResolutionDiagnosticKind,
    ResolutionReport, ResolveRequest, ResolvedAgent, ResolvedComponent, ResolvedRunPlan,
};
pub use result::{
    RESULT_DECODE_INVALID_VALUE, RESULT_DECODE_SCHEMA_MISMATCH, ResultDecodeError, RunResult,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub use session::{Lane, Session};
pub use spec::{
    AGENT_SPEC_SCHEMA_VERSION, AgentBuilder, AgentSpec, AgentSpecError, ApprovalGrantMode,
    CapabilityActivation, CapabilityRef, CapabilitySpec, ChildRunPolicy, InstructionSpec,
    RunPolicy,
};

/// Whether this build of the facade has the `native-tokio` feature
/// compiled in.
///
/// A `wasm-host`-only consumer (`default-features = false, features =
/// ["wasm-host"]`, e.g. `finstack-ai-wasm`) cannot see this by writing its
/// own `#[cfg(feature = "native-tokio")]` — Cargo features are resolved
/// per dependency edge, and that consumer never declares or requests
/// `native-tokio` itself. But `cargo test --workspace` (and any other
/// build that also compiles a sibling depending on
/// `finstack-ai/native-tokio`, such as
/// `finstack-ai-provider-anthropic`) unifies features onto the single
/// `finstack-ai` unit built for that target, so this crate's *own*
/// `native-tokio` feature can end up enabled even for a `wasm-host`-only
/// consumer. Exposing the outcome here — where `cfg!` sees this crate's
/// real, post-unification feature set — lets such consumers write tests
/// that stay correct in both configurations rather than assuming
/// `wasm-host` alone determines which constructor bodies were compiled.
#[must_use]
pub const fn native_tokio_enabled() -> bool {
    cfg!(feature = "native-tokio")
}
