//! Public native execution facade.

mod builder;
mod drive;
mod handle;
mod lane;
mod prepare;
mod run;
mod types;

#[cfg(all(test, feature = "native-tokio"))]
mod tests;

pub use builder::NativeAgentBuilder;
pub use handle::Agent;
pub use run::AgentRun;
pub use types::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, AgentRunError, AgentRunOutput, AgentRunRequest,
    CapabilityCatalogEntry,
};
