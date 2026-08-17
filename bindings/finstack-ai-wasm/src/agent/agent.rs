use std::sync::Arc;

use finstack_ai::Agent as FacadeAgent;
use finstack_ai::runtime::ModelName;
use finstack_ai_kernel::SessionId;
use wasm_bindgen::prelude::*;

use crate::executor;
use crate::{JsJournalStore, JsModel, JsToolset};

use super::build::{build_agent, inspect_session_inner};
use super::capabilities::{catalog_array, parse_active_capabilities, parse_capabilities};
use super::errors::{agent_error, session_error};
use super::request::run_request;
use super::results::RunResult;
use super::run::Run;
use super::session::Session;

/// Rust-owned Agent handle.
#[wasm_bindgen(js_name = Agent)]
pub struct Agent {
    pub(super) inner: Arc<FacadeAgent>,
    pub(super) model: ModelName,
}

#[wasm_bindgen(js_class = Agent)]
impl Agent {
    /// Construct an Agent over a trusted JS model and optional toolsets.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when configuration is invalid.
    #[wasm_bindgen(js_name = create)]
    #[expect(
        clippy::too_many_arguments,
        reason = "wasm-bindgen create forwards each host handle list distinctly"
    )]
    pub fn create(
        model: &JsModel,
        toolsets: Vec<JsToolset>,
        instruction: Option<String>,
        store: Option<JsJournalStore>,
        capabilities_json: Option<String>,
        active_capabilities_json: Option<String>,
        context_providers: Option<Vec<crate::JsContextProvider>>,
        middleware: Option<Vec<crate::JsMiddleware>>,
        observers: Option<Vec<crate::JsObserver>>,
    ) -> js_sys::Promise {
        let model_port = model.port();
        let model_component = model.component();
        let model_name = match model.model_name() {
            Ok(name) => name,
            Err(error) => return js_sys::Promise::reject(&error),
        };
        let ports = toolsets
            .iter()
            .map(|toolset| (toolset.component(), toolset.port()))
            .collect();
        let context_providers = context_providers
            .unwrap_or_default()
            .iter()
            .map(|provider| (provider.component(), provider.port()))
            .collect();
        let middleware = middleware
            .unwrap_or_default()
            .iter()
            .map(|middleware| (middleware.component(), middleware.port()))
            .collect();
        let observers = observers
            .unwrap_or_default()
            .iter()
            .map(|observer| (observer.component(), observer.port()))
            .collect();
        let store = store.map(|store| store.port());
        let capabilities = match parse_capabilities(capabilities_json.as_deref()) {
            Ok(capabilities) => capabilities,
            Err(error) => return js_sys::Promise::reject(&error),
        };
        let active_capabilities =
            match parse_active_capabilities(active_capabilities_json.as_deref()) {
                Ok(active) => active,
                Err(error) => return js_sys::Promise::reject(&error),
            };
        executor::drive(async move {
            build_agent(
                model_name,
                model_component,
                model_port,
                ports,
                context_providers,
                middleware,
                observers,
                instruction,
                store,
                capabilities,
                active_capabilities,
            )
            .await
            .map(JsValue::from)
        })
    }

    /// Return the bounded model-activated capability catalog in identity order.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the catalog object cannot be constructed.
    #[wasm_bindgen(js_name = capabilityCatalog)]
    pub fn capability_catalog(&self) -> Result<JsValue, JsValue> {
        catalog_array(self.inner.capability_catalog())
    }

    /// Render the compact catalog supplied to model-facing integrations.
    #[wasm_bindgen(js_name = compactCapabilityCatalog)]
    pub fn compact_capability_catalog(&self) -> String {
        self.inner.compact_capability_catalog()
    }

    /// Replay one stored session into a provisional inspect snapshot.
    ///
    /// This does not continue an interrupted run or retry in-flight effects.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the session id is invalid or the
    /// stored journal cannot be replayed.
    #[wasm_bindgen(js_name = inspectSession)]
    pub fn inspect_session(store: &JsJournalStore, session_id: String) -> js_sys::Promise {
        let store = store.port();
        executor::drive(async move { inspect_session_inner(store, session_id).await })
    }

    /// Create a live session on this agent's journal store.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the session cannot be created.
    #[wasm_bindgen(js_name = createSession)]
    pub fn create_session(&self, tenant_scope: Option<String>) -> js_sys::Promise {
        let store = self.inner.journal_store();
        let tenant_scope = tenant_scope.unwrap_or_else(|| "default".into());
        executor::drive(async move {
            finstack_ai::Session::create(store, tenant_scope)
                .await
                .map(|inner| JsValue::from(Session { inner }))
                .map_err(|error| session_error(&error))
        })
    }

    /// Open an existing session without respawning parked runs.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the session id is invalid or the
    /// stored journal cannot be replayed.
    #[wasm_bindgen(js_name = openSession)]
    pub fn open_session(
        &self,
        session_id: String,
        tenant_scope: Option<String>,
    ) -> js_sys::Promise {
        let store = self.inner.journal_store();
        let tenant_scope = tenant_scope.unwrap_or_else(|| "default".into());
        executor::drive(async move {
            let session_id = SessionId::parse(&session_id)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            finstack_ai::Session::open(store, session_id, tenant_scope)
                .await
                .map(|inner| JsValue::from(Session { inner }))
                .map_err(|error| session_error(&error))
        })
    }

    /// Start one run and return its detached control handle.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the request is invalid.
    pub fn start(
        &self,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: Option<f64>,
        max_output_retries: Option<f64>,
        capability: Option<String>,
    ) -> Result<Run, JsValue> {
        let request = run_request(
            &self.model,
            input,
            timeout_seconds,
            max_cycles,
            max_output_retries,
            capability,
        )?;
        self.inner
            .start(request)
            .map(|inner| Run { inner })
            .map_err(|error| agent_error(&error, None))
    }

    /// Execute one run and await its committed result.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the run fails.
    pub fn run(
        &self,
        input: String,
        timeout_seconds: Option<f64>,
        max_cycles: Option<f64>,
        max_output_retries: Option<f64>,
        capability: Option<String>,
    ) -> js_sys::Promise {
        let agent = Arc::clone(&self.inner);
        let model = self.model.clone();
        executor::drive(async move {
            let request = run_request(
                &model,
                input,
                timeout_seconds,
                max_cycles,
                max_output_retries,
                capability,
            )?;
            let run = agent
                .start(request)
                .map_err(|error| agent_error(&error, None))?;
            let locator = run.locator().clone();
            run.result()
                .await
                .map(|inner| JsValue::from(RunResult { inner }))
                .map_err(|error| agent_error(&error, Some(&locator)))
        })
    }
}
