//! Public SDK/facade for the `finstack-ai` agent engine.
//!
//! Owns registration, one-time extension resolution, and ergonomic adapters
//! over the runtime. The default `native-tokio` feature selects the runtime
//! Tokio driver; browser WASM consumers disable defaults and enable
//! `wasm-host`.
//!
//! Start at [`Agent::builder`]. Identity and run types are re-exported here.
//! A journal store still comes from a store-leaf crate.
//!
//! # Examples
//!
//! One model, one journal store, one run.
//!
//! ```
//! use std::sync::Arc;
//!
//! use finstack_ai::runtime::ports::journal::JournalStore;
//! use finstack_ai::runtime::ports::model::{Model, ModelName};
//! use finstack_ai::{
//!     Agent, AgentId, AgentRunRequest, BundleId, ComponentId, ComponentRef, PrincipalRef,
//!     RunLimits, RunSecurityContext, Version,
//! };
//! use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
//!
//! # async fn compose(model: Arc<dyn Model>) -> Result<(), Box<dyn std::error::Error>> {
//! const VERSION: Version = Version { major: 0, minor: 1, patch: 0 };
//!
//! let store: Arc<dyn JournalStore> = Arc::new(MemoryJournalStore::try_new(MemoryStoreLimits {
//!     sessions: 4,
//!     batches_per_session: 64,
//!     records_per_session: 512,
//!     snapshot_bytes: 4_096,
//! })?);
//!
//! let agent = Agent::builder(
//!     AgentId::parse("demo.agent")?,
//!     BundleId::parse("demo.bundle")?,
//!     (ComponentRef::new(ComponentId::parse("demo.model")?, Some(VERSION)), model),
//!     (ComponentRef::new(ComponentId::parse("demo.store")?, Some(VERSION)), store),
//! )
//! .try_instruction("Answer directly.")?
//! .limits(RunLimits::empty())
//! .build()
//! .await?;
//!
//! let security = RunSecurityContext::try_new(
//!     "tenant-a",
//!     PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))?,
//!     "local",
//!     "developer",
//!     "policy-v1",
//!     "decision-v1",
//!     None,
//! )?;
//! let request = AgentRunRequest::try_new(ModelName::try_new("demo-1")?, "hello", security)?;
//! let output = agent.run(request).await?;
//! # let _ = output;
//! # Ok(())
//! # }
//! ```
//!
//! When a model port reports a structured failure, [`AgentRunError::Failed`]
//! carries the port's full [`ErrorDescriptor`]. Other runtime failures keep
//! their stable code on [`AgentRunError::Runtime`]; read either with
//! [`AgentRunError::code`].
//!
//! # Module map
//!
//! Everything below is at the crate root except [`registry`] and the
//! [`runtime`] alias.
//!
//! - **Build and run an agent** — [`Agent::builder`] then
//!   [`Agent::run`]; [`AgentRunRequest`], [`AgentRunOutput`],
//!   [`AgentRunError`]
//! - **Provider shortcuts** — [`Agent::openai`], [`Agent::anthropic`],
//!   [`Agent::gemini`], [`Agent::ollama`], [`Agent::openrouter`],
//!   [`Agent::gateway`], each taking one `*AgentSpec` plus [`LinkedCommon`]
//! - **Declarative form** — [`AgentSpec::builder`], [`AgentSpec`],
//!   [`RunPolicy`], [`CapabilitySpec`]
//! - **Journaled handles** — [`Session`], [`Lane`]
//! - **Bundles** — [`BundleCatalog`], [`BundleResolver`], [`LockedBundle`]
//! - [`registry`] — registration, factories and one-time resolution
//! - [`runtime`] — the port and driver types from `finstack-ai-runtime`

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
    CompactionAuthorization, ComponentId, ComponentRef, CostLimit, ErrorCategory, ErrorCode,
    ErrorDescriptor, InteractionRequest, InteractionResolution, OperationLocator, PrincipalRef,
    RunLimits, RunSecurityContext, SessionId, Version,
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
