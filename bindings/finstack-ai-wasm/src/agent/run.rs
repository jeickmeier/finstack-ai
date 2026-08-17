use std::sync::OnceLock;

use finstack_ai::AgentRun;
use wasm_bindgen::prelude::*;

use crate::executor;

use super::errors::agent_error;
use super::events::EventBatch;
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

    /// Submit idempotent durable cancellation.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when cancellation cannot be committed.
    pub fn cancel(&self, _reason: Option<String>) -> js_sys::Promise {
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

    /// Close event delivery without cancelling the run.
    #[wasm_bindgen(js_name = closeEvents)]
    pub fn close_events(&self) -> js_sys::Promise {
        self.inner.close_events();
        js_sys::Promise::resolve(&JsValue::UNDEFINED)
    }
}
