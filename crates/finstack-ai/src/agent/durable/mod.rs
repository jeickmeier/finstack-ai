//! Embedded durable hosting for registered application definitions.
//!
//! The host owns `SQLite` journal and adapter stores. Registering an agent
//! resolves its composition against that journal; credentials and artifact
//! stores remain supplied by the application. `start` durably admits work,
//! and `tick` advances it under worker leases. Shutdown joins local work and
//! leaves accepted runs recoverable by a fresh host with the same definition.

pub(in crate::agent) mod capability;
mod definition;
mod descriptor;
mod host;

pub use finstack_ai_workflow_hitl::{InteractionRow, ResolutionInput};
pub use finstack_ai_workflow_worker::TickReport;
pub use host::{DurableHost, DurableHostBuilder, DurableInspection};

use std::sync::Arc;
use thiserror::Error;

/// Stable durable-host failure. Error messages contain no serialized settings.
#[derive(Debug, Clone, Error)]
#[error("durable host: {code}")]
pub struct DurableHostError {
    /// Stable classification suitable for applications and bindings.
    pub code: Arc<str>,
}

impl DurableHostError {
    fn new(code: impl Into<Arc<str>>) -> Self {
        Self { code: code.into() }
    }
}

impl From<finstack_ai_workflow_worker::WorkerError> for DurableHostError {
    fn from(error: finstack_ai_workflow_worker::WorkerError) -> Self {
        Self::new(error.code())
    }
}
impl From<finstack_ai_workflow_hitl::HitlError> for DurableHostError {
    fn from(error: finstack_ai_workflow_hitl::HitlError) -> Self {
        Self::new(error.code())
    }
}
impl From<super::AgentRunError> for DurableHostError {
    fn from(error: super::AgentRunError) -> Self {
        Self::new(error.code())
    }
}
impl From<finstack_ai_runtime::workflow::WorkflowDriverError> for DurableHostError {
    fn from(error: finstack_ai_runtime::workflow::WorkflowDriverError) -> Self {
        Self::new(error.code())
    }
}
