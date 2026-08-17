use finstack_ai::AgentRunOutput;
use wasm_bindgen::prelude::*;

use super::capabilities::active_capability_array;
use super::errors::locator_object;
use super::session::Locator;

/// Successful terminal result handle.
#[wasm_bindgen(js_name = RunResult)]
pub struct RunResult {
    pub(super) inner: AgentRunOutput,
}

#[wasm_bindgen(js_class = RunResult)]
impl RunResult {
    /// Concatenated final assistant text.
    #[wasm_bindgen(getter)]
    pub fn text(&self) -> String {
        self.inner.text()
    }

    /// Durable retry attempts consumed by this run.
    #[wasm_bindgen(getter, js_name = retryAttempts)]
    pub fn retry_attempts(&self) -> u32 {
        self.inner.retry_attempts()
    }

    /// Stable Rust-owned committed record-kind trace in journal order.
    #[wasm_bindgen(getter)]
    pub fn trace(&self) -> Vec<String> {
        self.inner
            .record_kinds()
            .iter()
            .map(|kind| kind.to_string())
            .collect()
    }

    /// Complete Rust-owned capability activation set for this run.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the activation objects cannot be constructed.
    #[wasm_bindgen(getter, js_name = activeCapabilities)]
    pub fn active_capabilities(&self) -> Result<JsValue, JsValue> {
        active_capability_array(self.inner.active_capabilities())
    }

    /// Operation locator for the completed run.
    #[wasm_bindgen(getter)]
    pub fn locator(&self) -> Locator {
        Locator {
            locator: self.inner.locator.clone(),
        }
    }

    /// Locator snapshot for the completed run.
    #[wasm_bindgen(getter)]
    pub fn session(&self) -> Locator {
        Locator {
            locator: self.inner.locator.clone(),
        }
    }

    /// Explicit result snapshot.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the snapshot object cannot be constructed.
    #[wasm_bindgen(js_name = toDict)]
    pub fn to_dict(&self) -> Result<JsValue, JsValue> {
        let object = locator_object(&self.inner.locator)?;
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("text"),
            &JsValue::from_str(&self.inner.text()),
        )?;
        Ok(object)
    }
}
