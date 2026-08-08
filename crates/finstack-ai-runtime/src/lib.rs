//! Runtime ports and effect execution for `finstack-ai`.
//!
//! Owns the six primary port contracts and effect execution. Target drivers
//! are selected by the non-default `native-tokio` and `wasm-host` features.
//!
//! PR-006 adds injected `UUIDv7` generation and a local [`FrameworkError`] wrapper.
//! Commit-loop and port implementations arrive in later Phase 2 pull requests.

#![warn(missing_docs)]

mod error;
mod id_generation;

pub use error::FrameworkError;
pub use id_generation::{Clock, IdGenerationError, RandomSource, UuidV7Generator};

#[cfg(feature = "native-tokio")]
pub use id_generation::{OsRandomSource, SystemClock};
