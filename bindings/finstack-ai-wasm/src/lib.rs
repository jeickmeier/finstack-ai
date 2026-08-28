//! Browser wasm-bindgen package for `finstack-ai`.
//!
//! Depends on the facade with default features disabled and only the
//! `wasm-host` pass-through feature enabled. The runtime feature selects
//! local port aliases; this crate owns wasm-bindgen, JS promise proxies, and
//! the host-driven local executor.
//!
//! Public JavaScript is the hand-authored `@finstack/ai` facade. Generated
//! glue stays under `js/generated/` and is not the published API.
//! `DeferredBindingAdapter::wasm()` remains `Unavailable` in `cargo test`.
//! Browser Playwright Agent-run goldens are the WASM parity evidence path.

#![warn(missing_docs)]
// wasm-bindgen generates FFI glue that requires `unsafe`.
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
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

#[cfg(target_arch = "wasm32")]
mod agent;
// Compiled for wasm32 builds, and additionally for native `cargo test` so
// `DocumentArtifactStore`'s FIFO/byte-budget eviction has native unit-test
// coverage (see `document_store::tests`) without needing a wasm32 test
// target. `build_artifact` (from `host_artifact`, unconditionally
// compiled) is available on both.
#[cfg(any(target_arch = "wasm32", test))]
mod document_store;
mod executor;
#[cfg(any(not(target_arch = "wasm32"), feature = "scripted-trace"))]
mod fixture;
mod health;
mod host;
mod host_artifact;
mod host_clock;
mod host_context;
#[cfg(not(target_arch = "wasm32"))]
mod host_memory;
mod host_middleware;
mod host_model;
mod host_observer;
mod host_store;
mod host_toolset;
#[cfg(feature = "scripted-trace")]
mod noop_trace;
#[cfg(any(test, feature = "scripted-trace"))]
mod port_proxies;
mod prebeta;
#[cfg(all(target_arch = "wasm32", feature = "scripted-trace"))]
mod scripted;

#[cfg(target_arch = "wasm32")]
use std::sync::Arc;

use wasm_bindgen::prelude::*;

/// In-process, non-persistent [`MemoryStore`](finstack_ai_memory::store::MemoryStore)
/// for wasm consumers that skip host-backed persistence entirely.
pub use finstack_ai_memory::store::InProcessMemoryStore;

/// Install the host driver when the generated module loads.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn wasm_start() {
    crate::agent::install_host_driver();
}

/// Copy one byte payload through the generated WASM boundary for benchmark calibration.
#[cfg(all(target_arch = "wasm32", feature = "scripted-trace"))]
#[wasm_bindgen(js_name = benchmarkRoundTrip)]
#[must_use]
pub fn benchmark_round_trip(payload: &[u8]) -> Vec<u8> {
    payload.to_vec()
}

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
///
/// Test-only: compiled for `cargo test` and the `scripted-trace` wasm harness.
#[cfg(any(test, feature = "scripted-trace"))]
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

/// Compute journal known-answer hex through the one Rust engine.
///
/// # Errors
///
/// Returns a TypeError-equivalent when `kind` or the diagnostic JSON is invalid.
#[wasm_bindgen(js_name = journalKnownAnswer)]
pub fn journal_known_answer(kind: &str, encoded: &str) -> Result<String, JsValue> {
    let answer = finstack_ai_protocol::journal_known_answer(kind, encoded)
        .map_err(|error| js_sys::TypeError::new(&error.to_string()))?;
    serde_json::to_string(&answer)
        .map_err(|_| js_sys::TypeError::new("journal known-answer serialization failed").into())
}

/// Normalize a pre-beta lineage or authenticated external-command shape.
///
/// # Errors
///
/// Returns a TypeError-equivalent when `kind` is unsupported or `value` is invalid.
#[wasm_bindgen(js_name = normalizePrebetaShape)]
pub fn normalize_prebeta_shape(kind: &str, encoded: &str) -> Result<String, JsValue> {
    finstack_ai_protocol::normalize_prebeta_shape(kind, encoded)
        .map_err(|error| js_sys::TypeError::new(error.reason()).into())
}

/// Apply normalized coordinator commands and return identity traces.
///
/// This export is test-only and does not submit a live Agent.
///
/// # Errors
///
/// Returns a TypeError-equivalent when any command fails DTO validation.
#[wasm_bindgen(js_name = applyScriptedCoordinatorCommands)]
pub fn apply_scripted_coordinator_commands(encoded: &str) -> Result<String, JsValue> {
    let commands: Vec<ScriptedCommand> = serde_json::from_str(encoded)
        .map_err(|_| js_sys::TypeError::new("invalid pre-beta shape"))?;
    let mut traces = Vec::new();
    for command in commands {
        let value = serde_json::to_string(&command.value)
            .map_err(|_| js_sys::TypeError::new("invalid pre-beta shape"))?;
        traces.push(
            prebeta::apply_prebeta_command(&command.kind, &value)
                .map_err(|error| js_sys::TypeError::new(error.reason()))?,
        );
    }
    serde_json::to_string(&serde_json::json!({ "commands": traces }))
        .map_err(|_| js_sys::TypeError::new("invalid pre-beta shape").into())
}

#[derive(serde::Deserialize)]
struct ScriptedCommand {
    kind: String,
    value: serde_json::Value,
}

/// Debug helper: parse a document to Markdown and return only the Markdown.
///
/// Runs the same `finstack-ai-tools-document` parser the ingest middleware
/// uses, without an `Agent` or `Run`, so a developer can see exactly what
/// would be injected for a given file.
///
/// # Errors
///
/// Returns a TypeError-equivalent carrying the stable `document_*` parse
/// error code when the input is oversized, unsupported, or unparseable.
#[wasm_bindgen(js_name = parseDocumentMarkdown)]
pub fn parse_document_markdown(data: &[u8], media_type: &str) -> Result<String, JsValue> {
    let parsed = finstack_ai_tools_document::parser::parse(
        data,
        Some(media_type),
        &finstack_ai_tools_document::parser::DocumentLimits::default(),
    )
    .map_err(|error| js_sys::TypeError::new(&error.to_string()))?;
    Ok(parsed.markdown)
}

/// Debug helper: parse a document and return the full detailed result.
///
/// # Errors
///
/// Returns a TypeError-equivalent carrying the stable `document_*` parse
/// error code when the input is oversized, unsupported, or unparseable, or
/// when the parsed result cannot be serialized.
#[wasm_bindgen(js_name = parseDocument)]
pub fn parse_document(data: &[u8], media_type: &str) -> Result<JsValue, JsValue> {
    let parsed = finstack_ai_tools_document::parser::parse(
        data,
        Some(media_type),
        &finstack_ai_tools_document::parser::DocumentLimits::default(),
    )
    .map_err(|error| js_sys::TypeError::new(&error.to_string()))?;
    let encoded = serde_json::to_string(&parsed)
        .map_err(|_| js_sys::TypeError::new("parsed document result serialization failed"))?;
    js_sys::JSON::parse(&encoded)
        .map_err(|_| js_sys::TypeError::new("parsed document result serialization failed").into())
}

/// Trusted JS model wrapper. Not an Agent handle.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsModel)]
pub struct JsModel {
    inner: Arc<host_model::HostModel>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsModel)]
impl JsModel {
    /// Construct a model wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when options or the adapter are invalid.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue, options: JsValue) -> Result<JsModel, JsValue> {
        let options = parse_js_options(&options)?;
        Ok(Self {
            inner: Arc::new(
                host_model::HostModel::from_js(adapter, options)
                    .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
            ),
        })
    }
}

#[cfg(target_arch = "wasm32")]
impl JsModel {
    pub(crate) fn port(&self) -> Arc<dyn finstack_ai::runtime::ports::model::Model> {
        Arc::clone(&self.inner) as Arc<dyn finstack_ai::runtime::ports::model::Model>
    }

    pub(crate) fn component(&self) -> finstack_ai_kernel::ComponentRef {
        self.inner.component().clone()
    }

    pub(crate) fn model_name(
        &self,
    ) -> Result<finstack_ai::runtime::ports::model::ModelName, JsValue> {
        finstack_ai::runtime::ports::model::Model::descriptor(self.inner.as_ref())
            .models
            .first()
            .cloned()
            .ok_or_else(|| js_sys::TypeError::new("model descriptor is empty").into())
    }
}

/// Trusted JS toolset wrapper. Not an Agent handle.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsToolset)]
pub struct JsToolset {
    inner: Arc<host_toolset::HostToolset>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsToolset)]
impl JsToolset {
    /// Construct a toolset wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when options or the adapter are invalid.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue, options: JsValue) -> Result<JsToolset, JsValue> {
        let options = parse_js_options(&options)?;
        Ok(Self {
            inner: Arc::new(
                host_toolset::HostToolset::from_js(adapter, options)
                    .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
            ),
        })
    }

    /// Clone the wrapper without moving the caller's handle.
    #[wasm_bindgen(js_name = cloneHandle)]
    pub fn clone_handle(&self) -> JsToolset {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl JsToolset {
    pub(crate) fn port(&self) -> Arc<dyn finstack_ai::runtime::ports::tool::Toolset> {
        Arc::clone(&self.inner) as Arc<dyn finstack_ai::runtime::ports::tool::Toolset>
    }

    pub(crate) fn component(&self) -> finstack_ai_kernel::ComponentRef {
        self.inner.component().clone()
    }
}

/// Trusted JS context-provider wrapper.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsContextProvider)]
pub struct JsContextProvider {
    inner: Arc<host_context::HostContextProvider>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsContextProvider)]
impl JsContextProvider {
    /// Construct a context-provider wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when options or the adapter are invalid.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue, options: JsValue) -> Result<JsContextProvider, JsValue> {
        let options = parse_js_options(&options)?;
        Ok(Self {
            inner: Arc::new(
                host_context::HostContextProvider::from_js(adapter, options)
                    .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
            ),
        })
    }
}

#[cfg(target_arch = "wasm32")]
impl JsContextProvider {
    /// Borrow the trusted context-provider port.
    pub(crate) fn port(&self) -> Arc<dyn finstack_ai::runtime::ports::context::ContextProvider> {
        Arc::clone(&self.inner) as Arc<dyn finstack_ai::runtime::ports::context::ContextProvider>
    }

    /// Exact registered component identity.
    pub(crate) fn component(&self) -> finstack_ai_kernel::ComponentRef {
        self.inner.component_ref()
    }
}

/// Trusted JS middleware wrapper.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsMiddleware)]
pub struct JsMiddleware {
    inner: Arc<host_middleware::HostMiddleware>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsMiddleware)]
impl JsMiddleware {
    /// Construct a middleware wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when options or the adapter are invalid.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue, options: JsValue) -> Result<JsMiddleware, JsValue> {
        let options = parse_js_options(&options)?;
        Ok(Self {
            inner: Arc::new(
                host_middleware::HostMiddleware::from_js(adapter, options)
                    .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
            ),
        })
    }
}

#[cfg(target_arch = "wasm32")]
impl JsMiddleware {
    /// Borrow the trusted middleware port.
    pub(crate) fn port(&self) -> Arc<dyn finstack_ai::runtime::ports::middleware::Middleware> {
        Arc::clone(&self.inner) as Arc<dyn finstack_ai::runtime::ports::middleware::Middleware>
    }

    /// Exact registered component identity.
    pub(crate) fn component(&self) -> finstack_ai_kernel::ComponentRef {
        self.inner.component_ref()
    }
}

/// Trusted JS observer wrapper.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsObserver)]
pub struct JsObserver {
    inner: Arc<host_observer::HostObserver>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsObserver)]
impl JsObserver {
    /// Construct an observer wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when options or the adapter are invalid.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue, options: JsValue) -> Result<JsObserver, JsValue> {
        let options = parse_js_options(&options)?;
        Ok(Self {
            inner: Arc::new(
                host_observer::HostObserver::from_js(adapter, options)
                    .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
            ),
        })
    }
}

#[cfg(target_arch = "wasm32")]
impl JsObserver {
    /// Borrow the trusted observer port.
    pub(crate) fn port(&self) -> Arc<dyn finstack_ai::runtime::ports::observer::Observer> {
        Arc::clone(&self.inner) as Arc<dyn finstack_ai::runtime::ports::observer::Observer>
    }

    /// Exact registered component identity.
    pub(crate) fn component(&self) -> finstack_ai_kernel::ComponentRef {
        self.inner.component_ref()
    }
}

/// Trusted JS journal-store wrapper. Not crash-durable.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsJournalStore)]
pub struct JsJournalStore {
    inner: std::sync::Arc<host_store::HostJournalStore>,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsJournalStore)]
impl JsJournalStore {
    /// Construct a journal-store wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when the adapter is invalid.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue, options: JsValue) -> Result<JsJournalStore, JsValue> {
        let options = parse_js_options(&options)?;
        Ok(Self {
            inner: std::sync::Arc::new(
                host_store::HostJournalStore::from_js(adapter, options)
                    .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
            ),
        })
    }

    /// Clone the wrapper without moving the caller's handle.
    #[wasm_bindgen(js_name = cloneHandle)]
    pub fn clone_handle(&self) -> JsJournalStore {
        Self {
            inner: std::sync::Arc::clone(&self.inner),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl JsJournalStore {
    pub(crate) fn port(
        &self,
    ) -> std::sync::Arc<dyn finstack_ai::runtime::ports::journal::JournalStore> {
        std::sync::Arc::clone(&self.inner)
            as std::sync::Arc<dyn finstack_ai::runtime::ports::journal::JournalStore>
    }
}

/// Host clock wrapper. `now()` returns Unix milliseconds.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsClock)]
pub struct JsClock {
    _inner: host_clock::HostClock,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsClock)]
impl JsClock {
    /// Construct a clock wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when `now` is missing.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue) -> Result<JsClock, JsValue> {
        Ok(Self {
            _inner: host_clock::HostClock::from_js(adapter)
                .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
        })
    }
}

/// Host random-source wrapper.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsRandomSource)]
pub struct JsRandomSource {
    _inner: host_clock::HostRandomSource,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsRandomSource)]
impl JsRandomSource {
    /// Construct a random-source wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when `fillBytes` is missing.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue) -> Result<JsRandomSource, JsValue> {
        Ok(Self {
            _inner: host_clock::HostRandomSource::from_js(adapter)
                .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
        })
    }
}

/// Host artifact-store wrapper over `Uint8Array` payloads.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = JsArtifactStore)]
pub struct JsArtifactStore {
    _inner: host_artifact::HostArtifactStore,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_class = JsArtifactStore)]
impl JsArtifactStore {
    /// Construct an artifact-store wrapper around a trusted host adapter.
    ///
    /// # Errors
    ///
    /// Returns a TypeError-equivalent when `stagePut` or `get` is missing.
    #[wasm_bindgen(constructor)]
    pub fn new(adapter: JsValue) -> Result<JsArtifactStore, JsValue> {
        Ok(Self {
            _inner: host_artifact::HostArtifactStore::from_js(adapter)
                .map_err(|_| js_sys::TypeError::new("invalid host options"))?,
        })
    }
}

/// Drive one scripted `Model::request`. Test-only; not an Agent run.
///
/// # Errors
///
/// Returns a JavaScript exception when options cannot be parsed.
#[cfg(all(target_arch = "wasm32", feature = "scripted-trace"))]
#[wasm_bindgen(js_name = driveScriptedModelRequest)]
pub fn drive_scripted_model_request(
    adapter: JsValue,
    options: JsValue,
    signal: JsValue,
) -> js_sys::Promise {
    crate::executor::drive(scripted::drive_scripted_model_request(
        adapter, options, signal,
    ))
}

/// Drive one scripted `Toolset::call`. Test-only; not an Agent run.
///
/// # Errors
///
/// Returns a JavaScript exception when options cannot be parsed.
#[cfg(all(target_arch = "wasm32", feature = "scripted-trace"))]
#[wasm_bindgen(js_name = driveScriptedToolCall)]
pub fn drive_scripted_tool_call(
    adapter: JsValue,
    options: JsValue,
    signal: JsValue,
) -> js_sys::Promise {
    crate::executor::drive(scripted::drive_scripted_tool_call(adapter, options, signal))
}

/// Drive scripted journal-store health. Test-only; always non-durable.
///
/// # Errors
///
/// Returns a JavaScript exception when the adapter is invalid.
#[cfg(all(target_arch = "wasm32", feature = "scripted-trace"))]
#[wasm_bindgen(js_name = driveScriptedJournalHealth)]
pub fn drive_scripted_journal_health(adapter: JsValue, options: JsValue) -> js_sys::Promise {
    crate::executor::drive(scripted::drive_scripted_journal_health(adapter, options))
}

#[cfg(target_arch = "wasm32")]
fn parse_js_options<T: serde::de::DeserializeOwned>(value: &JsValue) -> Result<T, JsValue> {
    let encoded = js_sys::JSON::stringify(value)
        .map_err(|_| js_sys::TypeError::new("invalid host options"))?
        .as_string()
        .ok_or_else(|| js_sys::TypeError::new("invalid host options"))?;
    serde_json::from_str(&encoded)
        .map_err(|_| js_sys::TypeError::new("invalid host options").into())
}

fn set_string(object: &js_sys::Object, key: &str, value: &str) -> Result<(), JsValue> {
    js_sys::Reflect::set(object, &JsValue::from_str(key), &JsValue::from_str(value))?;
    Ok(())
}

/// Construct native host adapters so the DTO path stays in the native graph.
#[cfg(not(target_arch = "wasm32"))]
pub fn compile_native_host_adapters() {
    use crate::host::{HostFailure, NativeHostResult};
    use crate::host_artifact::HostArtifactStore;
    use crate::host_clock::{HostClock, HostRandomSource};
    use crate::host_memory::HostMemoryStore;
    use crate::host_store::{HostJournalStore, HostJournalStoreOptions};
    use finstack_ai::runtime::artifact::ArtifactStore;
    use finstack_ai::runtime::ids::{Clock, RandomSource};
    use finstack_ai::runtime::ports::journal::JournalStore;
    use finstack_ai_memory::store::MemoryStore;

    compile_native_port_adapters();
    let store = HostJournalStore::from_callback(
        HostJournalStoreOptions {
            detail: "js_memory_prebeta".into(),
        },
        || Ok(NativeHostResult::Object(r#"{"ready":true}"#.into())),
    );
    let _: std::sync::Arc<dyn JournalStore> = std::sync::Arc::new(store);
    let clock = HostClock::from_callback(|| Ok(1_704_067_200_000));
    let _ = Clock::now(&clock);
    let random = HostRandomSource::from_callback(|len| Ok(vec![0_u8; len]));
    let mut buf = [0_u8; 4];
    let _ = RandomSource::fill_bytes(&random, &mut buf);
    let artifacts = HostArtifactStore::memory();
    let _: std::sync::Arc<dyn ArtifactStore> = std::sync::Arc::new(artifacts);
    let _ = HostFailure::Failed;
    let memory_store = HostMemoryStore::from_callback_fns(
        |_| Ok(NativeHostResult::Object(r#"{"ok":"inserted"}"#.into())),
        |_| Ok(NativeHostResult::Object(r#"{"ok":null}"#.into())),
        |_| Ok(NativeHostResult::Object(r#"{"ok":[]}"#.into())),
        |_| Ok(NativeHostResult::Object(r#"{"ok":null}"#.into())),
        |_| Ok(NativeHostResult::Object(r#"{"ok":null}"#.into())),
        |_| {
            Ok(NativeHostResult::Object(
                r#"{"ok":{"records":[],"total":0}}"#.into(),
            ))
        },
    );
    let _: std::sync::Arc<dyn MemoryStore> = std::sync::Arc::new(memory_store);
}

#[cfg(not(target_arch = "wasm32"))]
fn compile_native_port_adapters() {
    use crate::host::{HostModelOptions, NativeHostResult};
    use crate::host_context::{HostContextOptions, HostContextProvider};
    use crate::host_middleware::{HostMiddleware, HostMiddlewareOptions};
    use crate::host_model::HostModel;
    use crate::host_observer::{HostObserver, HostObserverOptions};
    use crate::host_toolset::{HostToolset, HostToolsetOptions};
    use finstack_ai::runtime::ports::context::ContextProvider;
    use finstack_ai::runtime::ports::middleware::Middleware;
    use finstack_ai::runtime::ports::model::Model;
    use finstack_ai::runtime::ports::observer::Observer;
    use finstack_ai::runtime::ports::tool::Toolset;

    let Ok(model) = HostModel::from_callback(
        HostModelOptions {
            component: "js.model.fixture".into(),
            provider: "js-fixture".into(),
            model: "js-fixture-model".into(),
            hard_input_bytes: 1_024,
            context_window_tokens: 1_024,
            max_output_tokens: 128,
            idempotent_requests: false,
        },
        |_| {
            Ok(NativeHostResult::Object(
                r#"{"text":"ok","completion_id":"compile"}"#.into(),
            ))
        },
    ) else {
        return;
    };
    let _ = model.component();
    let _: std::sync::Arc<dyn Model> = std::sync::Arc::new(model);

    let Ok(tool) = serde_json::from_str(crate::fixture::echo_tool_json()) else {
        return;
    };
    let Ok(toolset) = HostToolset::from_callback(
        HostToolsetOptions {
            component: "js.toolset.fixture".into(),
            name: "js-fixture-tools".into(),
            tools: vec![tool],
        },
        |_, _| {
            Ok(NativeHostResult::Object(
                r#"{"output":{"ok":true},"is_error":false}"#.into(),
            ))
        },
    ) else {
        return;
    };
    let _ = toolset.component();
    let _: std::sync::Arc<dyn Toolset> = std::sync::Arc::new(toolset);

    let Ok(context) = HostContextProvider::from_callback(
        &HostContextOptions {
            component: "js.context.fixture".into(),
            trusted_application_instructions: false,
        },
        |_| {
            Ok(NativeHostResult::Object(
                r#"{"items":[],"estimated_tokens":0,"bytes":0,"cache_key":null}"#.into(),
            ))
        },
    ) else {
        return;
    };
    let _: std::sync::Arc<dyn ContextProvider> = std::sync::Arc::new(context);
    if let Ok(middleware) = HostMiddleware::from_callback(
        &HostMiddlewareOptions {
            component: "js.middleware.fixture".into(),
            stages: vec!["before_run".into()],
            priority: 0,
        },
        |_| Ok(NativeHostResult::Object(r#""continue""#.into())),
    ) {
        let _: std::sync::Arc<dyn Middleware> = std::sync::Arc::new(middleware);
    }
    if let Ok(observer) = HostObserver::from_callback(
        &HostObserverOptions {
            component: "js.observer.fixture".into(),
            payload_mode: "metadata_only".into(),
        },
        |_| Ok(NativeHostResult::Object("null".into())),
    ) {
        let _: std::sync::Arc<dyn Observer> = std::sync::Arc::new(observer);
    }
}

#[cfg(test)]
mod tests {

    use super::{health, parse_document_markdown};

    #[test]
    fn wasm_bindgen_health_matches_rust_token() {
        assert_eq!(health(), "ok");
    }

    const SAMPLE_CSV: &[u8] = b"quarter,revenue\nQ1,1250\nQ2,1310\n";

    // `parse_document_markdown`'s success path returns a plain `String` and
    // never touches a `js_sys` import, so it is exercisable on a native
    // (non-wasm32) test target. Its error path and `parse_document`
    // (success or error) always construct a JS value — `js_sys::TypeError`
    // or `js_sys::JSON::parse` — which panics off wasm32 ("cannot call
    // wasm-bindgen imported functions on non-wasm targets"), matching every
    // other JS-value-returning export in this module (`journal_known_answer`,
    // `normalize_prebeta_shape`, `build_metadata`); those paths are covered
    // by the Playwright suite instead (`js/src/document-parse.test.ts`).
    #[test]
    fn parse_document_markdown_returns_markdown_for_csv() {
        let markdown =
            parse_document_markdown(SAMPLE_CSV, "text/csv").expect("csv parses to markdown");
        assert!(markdown.contains("1250"));
        assert!(markdown.contains("revenue"));
    }

    #[test]
    fn document_parser_matches_the_stable_parser_crate_directly() {
        // Cross-check against `finstack_ai_tools_document::parser::parse`
        // directly, so this test also documents that `parse_document` (the
        // wasm-bindgen export) is a thin pass-through with no extra
        // normalization beyond JS serialization.
        let parsed = finstack_ai_tools_document::parser::parse(
            SAMPLE_CSV,
            Some("text/csv"),
            &finstack_ai_tools_document::parser::DocumentLimits::default(),
        )
        .expect("csv parses");
        assert_eq!(
            parsed.format,
            finstack_ai_tools_document::parser::DocumentFormat::Csv
        );
        assert!(!parsed.requires_ocr);
        assert!(!parsed.truncated);
        assert!(parsed.page_count.is_none());
        assert!(parsed.classification.is_none());
        assert!(parsed.markdown.contains("revenue"));

        let markdown = parse_document_markdown(SAMPLE_CSV, "text/csv").expect("csv parses");
        assert_eq!(markdown, parsed.markdown);
    }
}
