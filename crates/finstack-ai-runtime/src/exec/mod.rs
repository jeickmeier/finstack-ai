//! Commit-before-effect execution and run ownership.

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) mod context_driver;
pub(crate) mod coordinator;
pub(crate) mod event_hub;
pub mod middleware_driver;

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) mod run_types;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) mod settlement;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) mod stage_settlement;

#[cfg(feature = "native-tokio")]
pub(crate) mod task;

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub(crate) mod host_task;
