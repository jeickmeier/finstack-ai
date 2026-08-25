use std::sync::OnceLock;

use finstack_ai::{AgentRun, RemoteChildRouteSpec};
use finstack_ai_kernel::ChildPlacement;
use wasm_bindgen::prelude::*;

use crate::executor;

use super::agent::Agent;
use super::errors::{agent_error, configuration_error};
use super::events::EventBatch;
use super::request::run_request;
use super::results::RunResult;
use super::session::{Locator, Session};

/// Detached run control handle. Drop detaches observation and does not cancel.
///
/// `list_interactions` / `resolve_interaction` remain native-only
/// (`native-tokio`). Browser WASM uses the host session/inbox path.
#[wasm_bindgen(js_name = Run)]
pub struct Run {
    pub(super) inner: AgentRun,
}

#[wasm_bindgen(js_class = Run)]
impl Run {
    /// Live session handle for this run.
    #[wasm_bindgen(getter)]
    pub fn session(&self) -> Session {
        Session {
            inner: self.inner.session(),
        }
    }

    /// Immutable operation locator snapshot.
    #[wasm_bindgen(getter)]
    pub fn locator(&self) -> Locator {
        Locator {
            locator: self.inner.locator().clone(),
        }
    }

    /// Wait for the retained terminal result.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the run fails, times out, or is cancelled.
    pub fn result(&self) -> js_sys::Promise {
        let run = self.inner.clone();
        executor::drive(async move {
            let locator = run.locator().clone();
            run.result()
                .await
                .map(|inner| JsValue::from(RunResult { inner }))
                .map_err(|error| agent_error(&error, Some(&locator)))
        })
    }

    /// Read the latest confirmed semantic and lifecycle snapshot.
    #[wasm_bindgen(js_name = liveState)]
    pub fn live_state(&self) -> js_sys::Promise {
        let run = self.inner.clone();
        executor::drive(async move {
            let locator = run.locator().clone();
            let state = run
                .live_state()
                .await
                .map_err(|error| agent_error(&error, Some(&locator)))?;
            live_state_object(&state)
        })
    }

    /// Wait until the latest-only view advances beyond `revision`.
    #[wasm_bindgen(js_name = waitForLiveState)]
    pub fn wait_for_live_state(&self, revision: u64) -> js_sys::Promise {
        let run = self.inner.clone();
        executor::drive(async move {
            let locator = run.locator().clone();
            let state = run
                .wait_for_live_state(revision)
                .await
                .map_err(|error| agent_error(&error, Some(&locator)))?;
            live_state_object(&state)
        })
    }

    /// Snapshot bounded, redacted observer-delivery diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when run startup failed before a runtime
    /// handle was published.
    #[wasm_bindgen(js_name = observerDiagnostics)]
    pub fn observer_diagnostics(&self) -> js_sys::Promise {
        let run = self.inner.clone();
        executor::drive(async move {
            let locator = run.locator().clone();
            let diagnostics = run
                .observer_diagnostics()
                .await
                .map_err(|error| agent_error(&error, Some(&locator)))?;
            observer_diagnostics_object(&diagnostics)
        })
    }

    /// Submit idempotent durable cancellation.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when cancellation cannot be committed.
    pub fn cancel(&self) -> js_sys::Promise {
        let run = self.inner.clone();
        executor::drive(async move {
            let locator = run.locator().clone();
            run.cancel()
                .await
                .map(|()| JsValue::UNDEFINED)
                .map_err(|error| agent_error(&error, Some(&locator)))
        })
    }

    /// Receive the next transport batch, or `undefined` after close/terminal.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when event delivery fails to start.
    #[wasm_bindgen(js_name = nextEventBatch)]
    pub fn next_event_batch(&self) -> js_sys::Promise {
        let run = self.inner.clone();
        executor::drive(async move {
            match run.next_event_batch().await {
                Ok(Some(inner)) => Ok(JsValue::from(EventBatch {
                    inner,
                    serialized: OnceLock::new(),
                })),
                Ok(None) => Ok(JsValue::UNDEFINED),
                Err(error) => Err(agent_error(&error, Some(run.locator()))),
            }
        })
    }

    /// Prepare and accept one child through the Rust router.
    ///
    /// wasm-host fails closed with `agent_run_unsupported_plan`.
    #[wasm_bindgen(js_name = startChild)]
    #[expect(
        clippy::too_many_arguments,
        reason = "wasm-bindgen start_child forwards run bounds, placement, and remote route args"
    )]
    pub fn start_child(
        &self,
        child: &Agent,
        input: String,
        placement: String,
        timeout_seconds: Option<f64>,
        max_cycles: Option<f64>,
        max_output_retries: Option<f64>,
        capability: Option<String>,
        route_endpoint: Option<String>,
        route_service: Option<String>,
        route_id: Option<String>,
        route_token: Option<String>,
    ) -> js_sys::Promise {
        let parent = self.inner.clone();
        let child_agent = std::sync::Arc::clone(&child.inner);
        let model = child.model.clone();
        executor::drive(async move {
            let placement = parse_child_placement(&placement)?;
            let remote = remote_route(route_endpoint, route_service, route_id, route_token)?;
            let request = run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
                Vec::new(),
            )?;
            parent
                .start_child(child_agent.as_ref(), request, placement, remote)
                .await
                .map(|inner| JsValue::from(Run { inner }))
                .map_err(|error| agent_error(&error, Some(parent.locator())))
        })
    }

    /// Close event delivery without cancelling the run.
    #[wasm_bindgen(js_name = closeEvents)]
    pub fn close_events(&self) -> js_sys::Promise {
        self.inner.close_events();
        js_sys::Promise::resolve(&JsValue::UNDEFINED)
    }
}

fn live_state_object(state: &finstack_ai::runtime::run::LiveRunState) -> Result<JsValue, JsValue> {
    super::errors::js_safe_integer(state.revision, "live-state revision")
        .map_err(|error| agent_error(&error, None))?;
    super::errors::js_safe_integer(state.journal_sequence, "journal sequence")
        .map_err(|error| agent_error(&error, None))?;
    let status = match state.status {
        finstack_ai::runtime::run::RunStatus::Running => "running",
        finstack_ai::runtime::run::RunStatus::ShuttingDown => "shutting_down",
        finstack_ai::runtime::run::RunStatus::Stopped => "stopped",
        finstack_ai::runtime::run::RunStatus::Faulted { .. } => "faulted",
    };
    let value = serde_json::json!({
        "revision": state.revision,
        "journalSequence": state.journal_sequence,
        "status": status,
        "faultCode": state.fault_code,
        "phase": state.phase,
        "cycle": state.cycle,
        "preparedContextMessages": state.prepared_context_messages,
        "committedRunMessages": state.committed_run_messages,
        "activeCapabilities": state.active_capabilities,
        "resolvedPlanDigest": state.resolved_plan_digest,
        "pendingInteraction": state.pending_interaction,
        "validationFailure": state.validation_failure,
        "retryAttempts": state.retry_attempts,
        "terminal": state.terminal,
    });
    js_sys::JSON::parse(
        &serde_json::to_string(&value)
            .map_err(|_| JsValue::from_str("live state serialization failed"))?,
    )
}

fn observer_diagnostics_object(
    diagnostics: &finstack_ai::runtime::ports::observer::ObserverDiagnostics,
) -> Result<JsValue, JsValue> {
    let recent = js_sys::Array::new();
    for diagnostic in diagnostics.recent.iter() {
        let item = js_sys::Object::new();
        js_sys::Reflect::set(
            &item,
            &JsValue::from_str("code"),
            &JsValue::from_str(diagnostic.code),
        )?;
        js_sys::Reflect::set(
            &item,
            &JsValue::from_str("detail"),
            &JsValue::from_str(diagnostic.detail),
        )?;
        recent.push(&item);
    }

    let snapshot = js_sys::Object::new();
    js_sys::Reflect::set(
        &snapshot,
        &JsValue::from_str("total"),
        &js_sys::BigInt::from(diagnostics.total),
    )?;
    js_sys::Reflect::set(
        &snapshot,
        &JsValue::from_str("dropped"),
        &js_sys::BigInt::from(diagnostics.dropped),
    )?;
    js_sys::Reflect::set(&snapshot, &JsValue::from_str("recent"), &recent)?;
    Ok(snapshot.into())
}

fn parse_child_placement(value: &str) -> Result<ChildPlacement, JsValue> {
    match value {
        "compatible_lane_in_parent_session" => Ok(ChildPlacement::CompatibleLaneInParentSession),
        "isolated_child_session" => Ok(ChildPlacement::IsolatedChildSession),
        "remote_child_session" => Ok(ChildPlacement::RemoteChildSession),
        _ => Err(agent_error(
            &configuration_error(format!("unsupported child placement: {value}")),
            None,
        )),
    }
}

fn remote_route(
    endpoint: Option<String>,
    service: Option<String>,
    route: Option<String>,
    token: Option<String>,
) -> Result<Option<RemoteChildRouteSpec>, JsValue> {
    match (endpoint, service, route) {
        (None, None, None) => Ok(None),
        (Some(endpoint), Some(service), Some(route)) => Ok(Some(RemoteChildRouteSpec {
            endpoint,
            service,
            route,
            token,
        })),
        _ => Err(agent_error(
            &configuration_error("remote child route requires endpoint, service, and route id"),
            None,
        )),
    }
}
