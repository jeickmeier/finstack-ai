//! Browser wasm-bindgen package for `finstack-ai`.
//!
//! Depends on the facade with default features disabled and only the
//! `wasm-host` pass-through feature enabled. The runtime feature selects
//! local port aliases; this crate owns wasm-bindgen, JS promise proxies, and
//! the host-driven local executor.
//!
//! Public JavaScript is the hand-authored `@finstack/ai` facade. Generated
//! glue stays under `js/generated/` and is not the published API.
//! `DeferredBindingAdapter::wasm()` remains unavailable.

#![warn(missing_docs)]

mod executor;
mod health;
#[cfg(feature = "scripted-trace")]
mod noop_trace;
mod port_proxies;

use wasm_bindgen::prelude::*;

/// Process-local health token. Does not create a runtime, open a store, or spawn work.
#[must_use]
#[wasm_bindgen]
pub fn health() -> String {
    health::health().to_owned()
}

/// Lockstep version metadata for the wasm package.
///
/// # Errors
///
/// Returns a JavaScript exception when the metadata object cannot be constructed.
#[wasm_bindgen(js_name = buildMetadata)]
pub fn build_metadata() -> Result<JsValue, JsValue> {
    let metadata = health::build_metadata();
    let object = js_sys::Object::new();
    set_string(&object, "version", metadata.version)?;
    set_string(&object, "engineVersion", metadata.engine_version)?;
    set_string(&object, "implementation", metadata.implementation)?;
    set_string(&object, "target", metadata.target)?;
    Ok(object.into())
}

/// Construct the six-port compile fixtures for the current target.
#[wasm_bindgen(js_name = compilePortProxies)]
pub fn compile_port_proxies() {
    #[cfg(not(target_arch = "wasm32"))]
    port_proxies::compile_native_port_proxies();
    #[cfg(target_arch = "wasm32")]
    port_proxies::compile_js_port_proxies();
}

/// Run the embedded no-op golden trace through `CommitCoordinator`.
///
/// This export is a test-only engine-load proof and is not part of the public
/// TypeScript surface.
///
/// # Errors
///
/// Returns a JavaScript exception when the fixture, store, or report fails.
#[cfg(feature = "scripted-trace")]
#[wasm_bindgen(js_name = runNoopTrace)]
pub fn run_noop_trace() -> Result<String, JsValue> {
    noop_trace::run_noop_trace_json().map_err(JsValue::from_str)
}

fn set_string(object: &js_sys::Object, key: &str, value: &str) -> Result<(), JsValue> {
    js_sys::Reflect::set(object, &JsValue::from_str(key), &JsValue::from_str(value))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::health;

    #[test]
    fn wasm_bindgen_health_matches_rust_token() {
        assert_eq!(health(), "ok");
    }
}
