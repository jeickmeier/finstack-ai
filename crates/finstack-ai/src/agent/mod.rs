//! Public native execution facade.

use finstack_ai_kernel::Version;

/// Preview lock/registration version used by the native builder and linked factories.
pub(super) const PREVIEW_ENGINE_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

mod activation;
mod builder;
#[cfg(feature = "native-tokio")]
mod child;
mod child_route;
#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
mod child_wasm;
#[cfg(feature = "native-tokio")]
pub mod deferred;
mod drive;
mod handle;
mod lane;
mod linked;
mod mask;
mod prepare;
mod run;
mod types;

#[cfg(all(test, feature = "native-tokio"))]
mod tests;

pub use activation::{
    ActivationHostError, CAPABILITY_ACTIVATION_BOUND, CAPABILITY_ACTIVATION_FAILED,
    MAX_CONCURRENT_CAPABILITY_ACTIVATIONS, NativeCapabilityHost,
};
pub use builder::NativeAgentBuilder;
pub use child_route::RemoteChildRouteSpec;
#[cfg(feature = "native-tokio")]
pub use deferred::{
    CHILD_RUN_BRIDGE_FAILED, CHILD_RUN_BRIDGE_PLANNER_REJECTED,
    CHILD_RUN_BRIDGE_PLANNER_UNAVAILABLE, ChildEventContext, ChildEventSink, ChildPlanContext,
    ChildRunBridge, ChildRunBridgeError, ChildRunResolver, ChildSettleOutcome,
    DeferredChildPlanner, DeferredPlanError, OutstandingDeferral, outstanding_deferrals,
};
pub use handle::Agent;
#[cfg(feature = "native-tokio")]
pub(crate) use lane::LaneLive;
pub use linked::{
    AnthropicAgentSpec, E2bSandboxAgentSpec, GatewayAgentSpec, LinkedAgent, LinkedAgentPorts,
    LinkedCommon, OllamaAgentSpec, OpenAiAgentSpec,
};
pub use run::AgentRun;
pub use types::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, AgentRunError, AgentRunOutput, AgentRunRequest,
    CapabilityCatalogEntry,
};
