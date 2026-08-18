//! wasm-host fail-closed child APIs. Bindings must not omit these methods.

use finstack_ai_kernel::{ChildPlacement, ChildRunLocator, ChildRunPrepared};

use super::child_route::RemoteChildRouteSpec;
use super::handle::Agent;
use super::run::AgentRun;
use super::types::{AGENT_RUN_UNSUPPORTED_PLAN, AgentRunError, AgentRunRequest};

impl AgentRun {
    /// wasm-host stub for [`AgentRun::prepare_child`](AgentRun).
    ///
    /// # Errors
    ///
    /// Always returns [`AGENT_RUN_UNSUPPORTED_PLAN`].
    #[expect(
        clippy::unused_async,
        reason = "wasm-host keeps the same async signature as native-tokio"
    )]
    pub async fn prepare_child(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
    ) -> Result<ChildRunPrepared, AgentRunError> {
        let _ = (child, request, placement);
        unsupported("prepare_child")
    }

    /// wasm-host stub for remote-routed prepare.
    ///
    /// # Errors
    ///
    /// Always returns [`AGENT_RUN_UNSUPPORTED_PLAN`].
    #[expect(
        clippy::unused_async,
        reason = "wasm-host keeps the same async signature as native-tokio"
    )]
    pub async fn prepare_child_routed(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
        remote: Option<RemoteChildRouteSpec>,
    ) -> Result<ChildRunPrepared, AgentRunError> {
        let _ = (child, request, placement, remote);
        unsupported("prepare_child")
    }

    /// wasm-host stub for [`AgentRun::accept_child`](AgentRun).
    ///
    /// # Errors
    ///
    /// Always returns [`AGENT_RUN_UNSUPPORTED_PLAN`].
    #[expect(
        clippy::unused_async,
        reason = "wasm-host keeps the same async signature as native-tokio"
    )]
    pub async fn accept_child(
        &self,
        prepared: &ChildRunPrepared,
        child: &Agent,
        request: AgentRunRequest,
    ) -> Result<Self, AgentRunError> {
        let _ = (prepared, child, request);
        unsupported("accept_child")
    }

    /// wasm-host stub for [`AgentRun::start_child`](AgentRun).
    ///
    /// # Errors
    ///
    /// Always returns [`AGENT_RUN_UNSUPPORTED_PLAN`].
    #[expect(
        clippy::unused_async,
        reason = "wasm-host keeps the same async signature as native-tokio"
    )]
    pub async fn start_child(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
    ) -> Result<Self, AgentRunError> {
        let _ = (child, request, placement);
        unsupported("start_child")
    }

    /// wasm-host stub for remote-routed start.
    ///
    /// # Errors
    ///
    /// Always returns [`AGENT_RUN_UNSUPPORTED_PLAN`].
    #[expect(
        clippy::unused_async,
        reason = "wasm-host keeps the same async signature as native-tokio"
    )]
    pub async fn start_child_routed(
        &self,
        child: &Agent,
        request: AgentRunRequest,
        placement: ChildPlacement,
        remote: Option<RemoteChildRouteSpec>,
    ) -> Result<Self, AgentRunError> {
        let _ = (child, request, placement, remote);
        unsupported("start_child")
    }

    /// wasm-host stub for [`AgentRun::cancel_child_locator`](AgentRun).
    ///
    /// # Errors
    ///
    /// Always returns [`AGENT_RUN_UNSUPPORTED_PLAN`].
    #[expect(
        clippy::unused_async,
        reason = "wasm-host keeps the same async signature as native-tokio"
    )]
    pub async fn cancel_child_locator(
        &self,
        locator: &ChildRunLocator,
    ) -> Result<(), AgentRunError> {
        let _ = locator;
        unsupported("cancel_child_locator")
    }
}

fn unsupported<T>(name: &str) -> Result<T, AgentRunError> {
    Err(AgentRunError::configuration(
        AGENT_RUN_UNSUPPORTED_PLAN,
        format!("AgentRun::{name} is not supported on wasm-host"),
    ))
}
