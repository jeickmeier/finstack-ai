//! Public native execution facade.

mod activation;
mod builder;
#[cfg(feature = "native-tokio")]
mod child;
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
pub use handle::Agent;
pub use linked::{
    AnthropicAgentSpec, GatewayAgentSpec, LinkedAgent, LinkedAgentPorts, OllamaAgentSpec,
    OpenAiAgentSpec,
};
pub use run::AgentRun;
pub use types::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, AgentRunError, AgentRunOutput, AgentRunRequest,
    CapabilityCatalogEntry,
};
