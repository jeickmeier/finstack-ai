//! Host-owned services that are not among the six primary ports.

pub(crate) mod agent_invoker;
pub(crate) mod artifact;
pub(crate) mod audit;
pub(crate) mod budget;
#[cfg(feature = "native-tokio")]
pub(crate) mod child_starter;
pub(crate) mod composition;
pub(crate) mod id_generation;
pub(crate) mod identity_map;
pub(crate) mod interaction;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod process_confinement;
pub(crate) mod session;
mod session_intern;
mod session_sync;
