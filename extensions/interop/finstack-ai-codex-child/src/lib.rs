//! Codex exec child-agent invoker and toolset.
//!
//! The crate holds no invocation authority beyond spawning the frozen
//! Codex binary. Child-run policy stays a runtime concern; policy, depth,
//! and budget failures surface as tool results.

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
#![doc(test(attr(allow(clippy::expect_used))))]

use thiserror::Error;

mod config;
mod events;
mod identity;
mod invoker;
mod state;

#[cfg(test)]
mod tests;

pub use config::{CodexExecConfig, CodexSandboxMode};
pub use events::CodexUsage;
pub use identity::{CODEX_PEER_AGENT_ID, codex_agent_ref, codex_route_ref};
pub use invoker::CodexChildInvoker;
pub use state::{CodexRunReport, CodexRunStatus};

/// Stable constructor failure code (missing binary, bad workspace, bad spec).
pub const CODEX_CONFIGURATION_INVALID: &str = "codex_configuration_invalid";
/// Tool arguments failed validation.
pub const CODEX_INVALID_ARGUMENTS: &str = "codex_invalid_arguments";
/// Named child is not in the in-process start table.
pub const CODEX_CHILD_NOT_FOUND: &str = "codex_child_not_found";

/// Construction failure for the invoker or toolset.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CodexChildError {
    /// Missing binary/workspace, or an invalid checked-in specification.
    #[error("{CODEX_CONFIGURATION_INVALID}: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}
