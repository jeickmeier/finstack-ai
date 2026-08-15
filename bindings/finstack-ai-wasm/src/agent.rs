//! wasm-bindgen Agent / Run handles over Rust-owned state.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use finstack_ai::runtime::{
    CapabilityId, Clock, CommitCoordinator, ComponentId, ComponentRef,
    EventBatch as RuntimeEventBatch, JournalStore, LoadRequest, ModelName, RandomSource,
    RunEventClass, RunEventKind, StoreError, Version,
};
use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, ActiveCapability, Agent as FacadeAgent,
    AgentRun, AgentRunError, AgentRunOutput, AgentRunRequest, CapabilityActivation,
    CapabilityActivationSource, CapabilityCatalogEntry, CapabilitySpec, InstructionSpec,
    OperationLocator, PrincipalRef, RunSecurityContext,
};
use finstack_ai_kernel::{ContentBlock, RunEvent, SessionId, TerminalState};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use js_sys::Uint8Array;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::executor;
use crate::{JsJournalStore, JsModel, JsToolset};

const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};
const DEFAULT_TIMEOUT_SECONDS: f64 = 30.0;
const MAX_TIMEOUT_SECONDS: f64 = 86_400.0;
const DEFAULT_MAX_CYCLES: u64 = 16;
const MAX_MAX_CYCLES: u64 = 1_024;
const DEFAULT_MAX_OUTPUT_RETRIES: u32 = 1;
const MAX_MAX_OUTPUT_RETRIES: u32 = 1_024;

/// Install spawn, sleep, clock, and random hooks for the host driver.
pub fn install_host_driver() {
    finstack_ai::runtime::host_driver::install_spawner(crate::executor::spawn_port_future);
    finstack_ai::runtime::host_driver::install_sleeper(|duration, done| {
        let millis =
            u32::try_from(duration.as_millis().min(u128::from(u32::MAX))).unwrap_or(u32::MAX);
        let global = js_sys::global();
        if let Ok(set_timeout) = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            && let Ok(set_timeout) = set_timeout.dyn_into::<js_sys::Function>()
        {
            let callback = Closure::once_into_js(move || done());
            let _ = set_timeout.call2(&global, &callback, &JsValue::from(millis));
            return;
        }
        done();
    });
    finstack_ai::runtime::host_driver::install_clock(Arc::new(BrowserClock));
    finstack_ai::runtime::host_driver::install_random(Arc::new(BrowserRandom));
}

struct BrowserClock;

impl Clock for BrowserClock {
    fn now(
        &self,
    ) -> Result<finstack_ai::runtime::Timestamp, finstack_ai::runtime::IdGenerationError> {
        let millis = js_sys::Date::now();
        if !millis.is_finite() || millis < 0.0 || millis > 9_007_199_254_740_991.0 {
            return Err(finstack_ai::runtime::IdGenerationError::Source(
                "host clock overflow".into(),
            ));
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "finite millis were range-checked against i64::MAX"
        )]
        let millis = millis as i64;
        Ok(finstack_ai::runtime::Timestamp::from_unix_ms(millis)?)
    }
}

struct BrowserRandom;

impl RandomSource for BrowserRandom {
    fn fill_bytes(&self, buf: &mut [u8]) -> Result<(), finstack_ai::runtime::IdGenerationError> {
        let len = u32::try_from(buf.len()).map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source(
                "host entropy request is too large".into(),
            )
        })?;
        let array = Uint8Array::new_with_length(len);
        let global = js_sys::global();
        let crypto = js_sys::Reflect::get(&global, &JsValue::from_str("crypto")).map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source("host crypto is unavailable".into())
        })?;
        let fill =
            js_sys::Reflect::get(&crypto, &JsValue::from_str("getRandomValues")).map_err(|_| {
                finstack_ai::runtime::IdGenerationError::Source("host crypto is unavailable".into())
            })?;
        let fill = fill.dyn_into::<js_sys::Function>().map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source("host crypto is unavailable".into())
        })?;
        fill.call1(&crypto, &array).map_err(|_| {
            finstack_ai::runtime::IdGenerationError::Source("host entropy fill failed".into())
        })?;
        array.copy_to(buf);
        Ok(())
    }
}

/// Rust-owned Agent handle.
#[wasm_bindgen(js_name = Agent)]
pub struct Agent {
    inner: Arc<FacadeAgent>,
    model: ModelName,
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

/// Detached run control handle. Drop detaches observation and does not cancel.
///
/// `list_interactions` / `resolve_interaction` remain native-only
/// (`native-tokio`). Browser WASM uses the host session/inbox path.
#[wasm_bindgen(js_name = Run)]
pub struct Run {
    inner: AgentRun,
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

/// Read-only operation locator.
#[wasm_bindgen(js_name = Locator)]
pub struct Locator {
    locator: OperationLocator,
}

#[wasm_bindgen(js_class = Locator)]
impl Locator {
    /// Tenant scope captured at acceptance.
    #[wasm_bindgen(getter, js_name = tenantScope)]
    pub fn tenant_scope(&self) -> String {
        self.locator.tenant_scope.to_string()
    }

    /// Session identity.
    #[wasm_bindgen(getter, js_name = sessionId)]
    pub fn session_id(&self) -> String {
        self.locator.session_id.to_string()
    }

    /// Lane identity.
    #[wasm_bindgen(getter, js_name = laneId)]
    pub fn lane_id(&self) -> String {
        self.locator.lane_id.to_string()
    }

    /// Run identity.
    #[wasm_bindgen(getter, js_name = runId)]
    pub fn run_id(&self) -> String {
        self.locator.run_id.to_string()
    }

    /// Explicit locator snapshot.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the snapshot object cannot be constructed.
    #[wasm_bindgen(js_name = toDict)]
    pub fn to_dict(&self) -> Result<JsValue, JsValue> {
        locator_object(&self.locator)
    }
}

/// Live session handle.
#[wasm_bindgen(js_name = Session)]
pub struct Session {
    inner: finstack_ai::Session,
}

#[wasm_bindgen(js_class = Session)]
impl Session {
    /// Tenant scope captured by the host.
    #[wasm_bindgen(getter, js_name = tenantScope)]
    pub fn tenant_scope(&self) -> String {
        self.inner.tenant_scope().to_string()
    }

    /// Session identity.
    #[wasm_bindgen(getter, js_name = sessionId)]
    pub fn session_id(&self) -> String {
        self.inner.session_id().to_string()
    }

    /// Create a named lane, optionally forking from an existing entry.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the lane cannot be created.
    #[wasm_bindgen(js_name = createLane)]
    pub fn create_lane(&self, name: String, fork: Option<String>) -> js_sys::Promise {
        let session = self.inner.clone();
        executor::drive(async move {
            let fork = fork
                .map(|value| finstack_ai::runtime::EntryId::parse(&value))
                .transpose()
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            session
                .create_lane(name, fork)
                .await
                .map(|inner| JsValue::from(Lane { inner }))
                .map_err(|error| session_error(&error))
        })
    }

    /// List restored lanes.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the session cannot be loaded.
    #[wasm_bindgen(js_name = listLanes)]
    pub fn list_lanes(&self) -> js_sys::Promise {
        let session = self.inner.clone();
        executor::drive(async move {
            session
                .list_lanes()
                .await
                .map(|lanes| {
                    lanes
                        .into_iter()
                        .map(|inner| JsValue::from(Lane { inner }))
                        .collect::<js_sys::Array>()
                        .into()
                })
                .map_err(|error| session_error(&error))
        })
    }

    /// Look up one lane by application name.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the lane does not exist.
    pub fn lane(&self, name: String) -> js_sys::Promise {
        let session = self.inner.clone();
        executor::drive(async move {
            session
                .lane(&name)
                .await
                .map(|inner| JsValue::from(Lane { inner }))
                .map_err(|error| session_error(&error))
        })
    }

    /// Bind a host-owned external identity to one lane.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the key is invalid, the lane is
    /// unknown, or the key is already bound to a different session lane.
    #[wasm_bindgen(js_name = bindExternalIdentity)]
    pub fn bind_external_identity(
        &self,
        map: &MemoryExternalIdentityMap,
        channel: String,
        account: String,
        thread: String,
        lane_id: String,
    ) -> Result<(), JsValue> {
        let key = finstack_ai::ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        let lane_id = finstack_ai::runtime::LaneId::parse(&lane_id)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        self.inner
            .bind_external_identity(&map.inner, key, lane_id)
            .map_err(|error| session_error(&error))
    }
}

/// Live lane handle.
#[wasm_bindgen(js_name = Lane)]
pub struct Lane {
    inner: finstack_ai::Lane,
}

#[wasm_bindgen(js_class = Lane)]
impl Lane {
    /// Durable lane identity.
    #[wasm_bindgen(getter, js_name = laneId)]
    pub fn lane_id(&self) -> String {
        self.inner.lane_id().to_string()
    }

    /// Session that owns this lane.
    #[wasm_bindgen(getter)]
    pub fn session(&self) -> Session {
        Session {
            inner: self.inner.session().clone(),
        }
    }

    /// Point this idle lane at an existing entry without copying.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the entry is unknown or the lane
    /// is busy.
    pub fn navigate(&self, entry_id: String) -> js_sys::Promise {
        let lane = self.inner.clone();
        executor::drive(async move {
            let entry_id = finstack_ai::runtime::EntryId::parse(&entry_id)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            lane.navigate(entry_id)
                .await
                .map(|()| JsValue::UNDEFINED)
                .map_err(|error| session_error(&error))
        })
    }

    /// Inspect name, leaf, and history length.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the lane cannot be inspected.
    pub fn inspect(&self) -> js_sys::Promise {
        let lane = self.inner.clone();
        executor::drive(async move {
            lane.inspect()
                .await
                .map(|inspect| {
                    let object = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("laneId"),
                        &JsValue::from_str(&inspect.lane_id.to_string()),
                    );
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("name"),
                        &JsValue::from_str(&inspect.name),
                    );
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("historyLen"),
                        &JsValue::from_f64(inspect.history.len() as f64),
                    );
                    JsValue::from(object)
                })
                .map_err(|error| session_error(&error))
        })
    }
}

/// In-process external identity map.
#[wasm_bindgen(js_name = MemoryExternalIdentityMap)]
pub struct MemoryExternalIdentityMap {
    inner: finstack_ai::MemoryExternalIdentityMap,
}

#[wasm_bindgen(js_class = MemoryExternalIdentityMap)]
impl MemoryExternalIdentityMap {
    /// Empty map.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: finstack_ai::MemoryExternalIdentityMap::new(),
        }
    }

    /// Resolve one previously bound key.
    ///
    /// # Errors
    ///
    /// Returns a structured host error when the key is invalid.
    pub fn resolve(
        &self,
        channel: String,
        account: String,
        thread: String,
    ) -> Result<JsValue, JsValue> {
        let key = finstack_ai::ExternalIdentityKey::try_new(channel, account, thread)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        Ok(
            finstack_ai::ExternalIdentityMap::resolve(&self.inner, &key).map_or(
                JsValue::UNDEFINED,
                |(session_id, lane_id)| {
                    let object = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("sessionId"),
                        &JsValue::from_str(&session_id.to_string()),
                    );
                    let _ = js_sys::Reflect::set(
                        &object,
                        &JsValue::from_str("laneId"),
                        &JsValue::from_str(&lane_id.to_string()),
                    );
                    object.into()
                },
            ),
        )
    }
}

/// Successful terminal result handle.
#[wasm_bindgen(js_name = RunResult)]
pub struct RunResult {
    inner: AgentRunOutput,
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

/// Immutable runtime event handle.
#[wasm_bindgen(js_name = Event)]
pub struct Event {
    inner: RunEvent,
}

#[wasm_bindgen(js_class = Event)]
impl Event {
    /// Event kind name.
    #[wasm_bindgen(getter)]
    pub fn kind(&self) -> String {
        event_kind_name(self.inner.kind()).to_owned()
    }

    /// Durable or transient class.
    #[wasm_bindgen(getter, js_name = eventClass)]
    pub fn event_class(&self) -> String {
        match self.inner.class() {
            RunEventClass::DurableDerived => "durable_derived".to_owned(),
            RunEventClass::Transient => "transient".to_owned(),
        }
    }

    /// Transient sequence.
    #[wasm_bindgen(getter, js_name = transientSequence)]
    pub fn transient_sequence(&self) -> u64 {
        self.inner.transient_sequence()
    }

    /// Durable sequence, when the event is durable-derived.
    #[wasm_bindgen(getter, js_name = durableSequence)]
    pub fn durable_sequence(&self) -> Option<u64> {
        self.inner.durable_sequence()
    }

    /// Explicit JSON snapshot.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the event cannot be serialized.
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner)
            .map_err(|_| js_sys::Error::new("event serialization failed").into())
    }
}

/// Bounded transport batch. Expand events only on request.
#[wasm_bindgen(js_name = EventBatch)]
pub struct EventBatch {
    inner: RuntimeEventBatch,
    serialized: OnceLock<Result<Arc<[u8]>, ()>>,
}

#[wasm_bindgen(js_class = EventBatch)]
impl EventBatch {
    /// First contained sequence.
    #[wasm_bindgen(getter, js_name = firstSequence)]
    pub fn first_sequence(&self) -> u64 {
        self.inner.first_sequence()
    }

    /// Last contained sequence.
    #[wasm_bindgen(getter, js_name = lastSequence)]
    pub fn last_sequence(&self) -> u64 {
        self.inner.last_sequence()
    }

    /// Lag-dropped transient events since the previous batch.
    #[wasm_bindgen(getter, js_name = droppedProgress)]
    pub fn dropped_progress(&self) -> u64 {
        self.inner.dropped_progress()
    }

    /// Expand contained events. This is the per-event FFI boundary.
    pub fn events(&self) -> Vec<Event> {
        self.inner
            .events()
            .iter()
            .cloned()
            .map(|inner| Event { inner })
            .collect()
    }

    /// Explicit JSON snapshot of the contained events.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the batch cannot be serialized.
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsValue> {
        let bytes = self.serialized_bytes()?;
        std::str::from_utf8(bytes)
            .map(ToOwned::to_owned)
            .map_err(|_| {
                js_sys::Error::new("event batch serialization produced invalid UTF-8").into()
            })
    }

    /// Explicit UTF-8 JSON bytes of the contained events.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript exception when the batch cannot be serialized.
    #[wasm_bindgen(js_name = toJsonBytes)]
    pub fn to_json_bytes(&self) -> Result<Uint8Array, JsValue> {
        let bytes = self.serialized_bytes()?;
        Ok(Uint8Array::from(bytes))
    }
}

impl EventBatch {
    fn serialized_bytes(&self) -> Result<&[u8], JsValue> {
        self.serialized
            .get_or_init(|| {
                serde_json::to_vec(self.inner.events())
                    .map(Arc::from)
                    .map_err(|_| ())
            })
            .as_ref()
            .map(Arc::as_ref)
            .map_err(|()| js_sys::Error::new("event batch serialization failed").into())
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "wasm-bindgen create forwards each host handle and capability list distinctly"
)]
async fn build_agent(
    model_name: ModelName,
    model_component: ComponentRef,
    model: Arc<dyn finstack_ai::runtime::Model>,
    toolsets: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Toolset>)>,
    context_providers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::ContextProvider>)>,
    middleware: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Middleware>)>,
    observers: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Observer>)>,
    instruction: Option<String>,
    store: Option<Arc<dyn JournalStore>>,
    capabilities: Vec<CapabilitySpec>,
    active_capabilities: Vec<CapabilityId>,
) -> Result<Agent, JsValue> {
    let (store_component, store) = match store {
        Some(store) => (component("js.store.host")?, store),
        None => {
            let memory = MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 64,
                batches_per_session: 256,
                records_per_session: 4_096,
                snapshot_bytes: 64 * 1_024,
            })
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
            (
                component("js.store.memory")?,
                Arc::new(memory) as Arc<dyn JournalStore>,
            )
        }
    };
    let mut builder = FacadeAgent::builder(
        finstack_ai_kernel::AgentId::parse("js.agent.host")
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        finstack_ai_kernel::BundleId::parse("js.bundle.host")
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        (model_component, model),
        (store_component, store),
    );
    for (component, toolset) in toolsets {
        builder = builder.toolset(component, toolset);
    }
    for (component, provider) in context_providers {
        builder = builder.context_provider(component, provider);
    }
    for (component, middleware) in middleware {
        builder = builder.middleware(component, middleware);
    }
    for (component, observer) in observers {
        builder = builder.observer(component, observer);
    }
    if let Some(instruction) = instruction {
        builder = builder
            .try_instruction(instruction)
            .map_err(|error| agent_error(&error, None))?;
    }
    for capability in capabilities {
        builder = builder.capability(capability);
    }
    for capability in active_capabilities {
        builder = builder.activate_application(capability);
    }
    let inner = builder
        .build()
        .await
        .map_err(|error| agent_error(&error, None))?;
    Ok(Agent {
        inner: Arc::new(inner),
        model: model_name,
    })
}

async fn inspect_session_inner(
    store: Arc<dyn JournalStore>,
    session_id: String,
) -> Result<JsValue, JsValue> {
    let session_id = SessionId::parse(&session_id)
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let loaded = store
        .load(LoadRequest { session_id })
        .await
        .map_err(store_error_js)?;
    if loaded.head_sequence == 0 && loaded.committed_batches.is_empty() {
        return inspect_object(&session_id, 0, "empty", None, None);
    }
    let recovered = CommitCoordinator::recover(Arc::clone(&store), session_id)
        .await
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let state = recovered.state();
    let phase = match state.terminal.as_ref() {
        Some(TerminalState::Completed(_)) => "completed",
        Some(TerminalState::Failed(_)) => "failed",
        Some(TerminalState::Cancelled(_)) => "cancelled",
        None if state.phase.is_none() && loaded.head_sequence == 0 => "empty",
        None => "in_progress",
    };
    let result_text = match state.terminal.as_ref() {
        Some(TerminalState::Completed(completed)) => state
            .messages
            .iter()
            .find(|message| message.id() == &completed.result_message_id)
            .map(message_text),
        _ => None,
    };
    let last_record_kind = loaded
        .committed_batches
        .iter()
        .rev()
        .flat_map(|batch| batch.records.iter().rev())
        .next()
        .map(|record| record.body().kind_name().to_owned());
    inspect_object(
        &session_id,
        loaded.head_sequence,
        phase,
        result_text.as_deref(),
        last_record_kind.as_deref(),
    )
}

fn message_text(message: &finstack_ai_kernel::Message) -> String {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect()
}

fn inspect_object(
    session_id: &SessionId,
    head_sequence: u64,
    phase: &str,
    result_text: Option<&str>,
    last_record_kind: Option<&str>,
) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("sessionId"),
        &JsValue::from_str(&session_id.to_string()),
    )?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "inspect sequences stay well below the 2^53 JS integer limit"
    )]
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("headSequence"),
        &JsValue::from(head_sequence as f64),
    )?;
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("phase"),
        &JsValue::from_str(phase),
    )?;
    if let Some(result_text) = result_text {
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("resultText"),
            &JsValue::from_str(result_text),
        )?;
    }
    if let Some(last_record_kind) = last_record_kind {
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("lastRecordKind"),
            &JsValue::from_str(last_record_kind),
        )?;
    }
    Ok(object.into())
}

fn store_error_js(error: StoreError) -> JsValue {
    let object = js_sys::Error::new(&error.to_string());
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("name"),
        &JsValue::from_str("FinstackError"),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("code"),
        &JsValue::from_str(error.code()),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("retryable"),
        &JsValue::from_bool(false),
    );
    match &error {
        StoreError::Conflict {
            expected_sequence,
            actual_next_sequence,
        } => {
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("expectedSequence"),
                &JsValue::from(*expected_sequence),
            );
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("actualNextSequence"),
                &JsValue::from(*actual_next_sequence),
            );
        }
        StoreError::Corruption { reason_code }
        | StoreError::InvalidRequest { reason_code }
        | StoreError::Unavailable { reason_code }
        | StoreError::Integrity { reason_code } => {
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("reasonCode"),
                &JsValue::from_str(reason_code),
            );
        }
        StoreError::LimitExceeded { resource, limit } => {
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("resource"),
                &JsValue::from_str(resource),
            );
            let _ = js_sys::Reflect::set(
                &object,
                &JsValue::from_str("limit"),
                &JsValue::from(u64::try_from(*limit).unwrap_or(u64::MAX)),
            );
        }
        StoreError::AmbiguousAcknowledgement => {}
    }
    object.into()
}

fn run_request(
    model: &ModelName,
    input: String,
    timeout_seconds: Option<f64>,
    max_cycles: Option<f64>,
    max_output_retries: Option<f64>,
    capability: Option<String>,
) -> Result<AgentRunRequest, JsValue> {
    let timeout_seconds = timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if !timeout_seconds.is_finite()
        || timeout_seconds <= 0.0
        || timeout_seconds > MAX_TIMEOUT_SECONDS
    {
        return Err(agent_error(
            &configuration_error("timeoutSeconds must be finite and in (0, 86400]"),
            None,
        ));
    }
    let max_cycles = optional_u64(max_cycles, DEFAULT_MAX_CYCLES, MAX_MAX_CYCLES, "maxCycles")?;
    let max_output_retries = u32::try_from(optional_u64(
        max_output_retries,
        u64::from(DEFAULT_MAX_OUTPUT_RETRIES),
        u64::from(MAX_MAX_OUTPUT_RETRIES),
        "maxOutputRetries",
    )?)
    .map_err(|_| {
        agent_error(
            &configuration_error("maxOutputRetries is out of range"),
            None,
        )
    })?;
    let security = RunSecurityContext::try_new(
        "js-local",
        PrincipalRef::try_new("finstack-ai-wasm", "local-user", Some("js-local"))
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        "local",
        "js-embedded",
        "js-policy-v1",
        "js-decision-v1",
        None,
    )
    .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let mut request = AgentRunRequest::try_new(model.clone(), input, security)
        .map_err(|error| agent_error(&error, None))?;
    request.timeout = Duration::from_secs_f64(timeout_seconds);
    request.max_cycles = max_cycles;
    request.max_output_retries = max_output_retries;
    if let Some(capability) = capability {
        request.capability = Some(
            CapabilityId::parse(&capability)
                .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        );
    }
    Ok(request)
}

fn optional_u64(value: Option<f64>, default: u64, max: u64, name: &str) -> Result<u64, JsValue> {
    let Some(value) = value else {
        return Ok(default);
    };
    if !value.is_finite() || value < 1.0 || value.fract() != 0.0 || value > max as f64 {
        return Err(agent_error(
            &configuration_error(format!("{name} must be a positive integer in 1..={max}")),
            None,
        ));
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite integer was range-checked against max"
    )]
    Ok(value as u64)
}

fn component(id: &str) -> Result<ComponentRef, JsValue> {
    Ok(ComponentRef::new(
        ComponentId::parse(id)
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        Some(PREVIEW_VERSION),
    ))
}

#[derive(Debug, Deserialize)]
struct JsCapabilityWire {
    id: String,
    description: String,
    instructions: Vec<String>,
    #[serde(default)]
    activation: Option<String>,
}

fn parse_capabilities(json: Option<&str>) -> Result<Vec<CapabilitySpec>, JsValue> {
    let Some(json) = json.filter(|value| !value.is_empty() && *value != "undefined") else {
        return Ok(Vec::new());
    };
    let wires: Vec<JsCapabilityWire> = serde_json::from_str(json)
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    wires
        .into_iter()
        .map(capability_from_wire)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_active_capabilities(json: Option<&str>) -> Result<Vec<CapabilityId>, JsValue> {
    let Some(json) = json.filter(|value| !value.is_empty() && *value != "undefined") else {
        return Ok(Vec::new());
    };
    let ids: Vec<String> = serde_json::from_str(json)
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    ids.into_iter()
        .map(|id| {
            CapabilityId::parse(id)
                .map_err(|error| agent_error(&configuration_error(error.to_string()), None))
        })
        .collect()
}

fn capability_from_wire(wire: JsCapabilityWire) -> Result<CapabilitySpec, JsValue> {
    let activation = match wire.activation.as_deref().unwrap_or("application") {
        "always" => CapabilityActivation::Always,
        "application" => CapabilityActivation::Application,
        "model" => CapabilityActivation::Model,
        "disabled" => CapabilityActivation::Disabled,
        other => {
            return Err(agent_error(
                &configuration_error(format!(
                    "activation must be always, application, model, or disabled: {other}"
                )),
                None,
            ));
        }
    };
    let instructions = wire
        .instructions
        .into_iter()
        .map(InstructionSpec::try_new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let capability = CapabilitySpec {
        id: CapabilityId::parse(wire.id)
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        description: Arc::from(wire.description),
        instructions: instructions.into(),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation,
    };
    capability
        .validate()
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    Ok(capability)
}

fn catalog_array(entries: Vec<CapabilityCatalogEntry>) -> Result<JsValue, JsValue> {
    let array = js_sys::Array::new();
    for entry in entries {
        let object = js_sys::Object::new();
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("id"),
            &JsValue::from_str(entry.id().as_str()),
        )?;
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("description"),
            &JsValue::from_str(entry.description()),
        )?;
        array.push(&object);
    }
    Ok(array.into())
}

fn active_capability_array(active: &[ActiveCapability]) -> Result<JsValue, JsValue> {
    let array = js_sys::Array::new();
    for item in active {
        let object = js_sys::Object::new();
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("id"),
            &JsValue::from_str(item.capability_id.as_str()),
        )?;
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("source"),
            &JsValue::from_str(match item.source {
                CapabilityActivationSource::Always => "always",
                CapabilityActivationSource::Application => "application",
                CapabilityActivationSource::Model => "model",
            }),
        )?;
        array.push(&object);
    }
    Ok(array.into())
}

fn configuration_error(message: impl Into<String>) -> AgentRunError {
    AgentRunError::Configuration {
        code: AGENT_RUN_INVALID_CONFIGURATION,
        message: message.into(),
    }
}

fn session_error(error: &finstack_ai::SessionError) -> JsValue {
    let object = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("name"),
        &JsValue::from_str("FinstackError"),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("code"),
        &JsValue::from_str(error.code()),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("message"),
        &JsValue::from_str(&error.to_string()),
    );
    let _ = js_sys::Reflect::set(&object, &JsValue::from_str("retryable"), &JsValue::FALSE);
    object.into()
}

fn agent_error(error: &AgentRunError, locator: Option<&OperationLocator>) -> JsValue {
    let object = js_sys::Error::new(&error.to_string());
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("name"),
        &JsValue::from_str("FinstackError"),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("code"),
        &JsValue::from_str(stable_code(error)),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("retryable"),
        &JsValue::from_bool(error.retryable()),
    );
    if let Some(locator) = locator
        && let Ok(context) = locator_object(locator)
    {
        let _ = js_sys::Reflect::set(&object, &JsValue::from_str("context"), &context);
    }
    object.into()
}

fn stable_code(error: &AgentRunError) -> &'static str {
    match error.code() {
        AGENT_RUN_INVALID_CONFIGURATION => AGENT_RUN_INVALID_CONFIGURATION,
        AGENT_RUN_TIMEOUT => AGENT_RUN_TIMEOUT,
        AGENT_RUN_CANCELLED => AGENT_RUN_CANCELLED,
        AGENT_RUN_UNSUPPORTED_PLAN => AGENT_RUN_UNSUPPORTED_PLAN,
        _ => AGENT_RUN_RUNTIME_FAILURE,
    }
}

fn locator_object(locator: &OperationLocator) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("tenantScope"),
        &JsValue::from_str(&locator.tenant_scope),
    )?;
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("sessionId"),
        &JsValue::from_str(&locator.session_id.to_string()),
    )?;
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("laneId"),
        &JsValue::from_str(&locator.lane_id.to_string()),
    )?;
    js_sys::Reflect::set(
        &object,
        &JsValue::from_str("runId"),
        &JsValue::from_str(&locator.run_id.to_string()),
    )?;
    Ok(object.into())
}

const fn event_kind_name(kind: RunEventKind) -> &'static str {
    match kind {
        RunEventKind::RunAccepted => "run_accepted",
        RunEventKind::EffectRequested => "effect_requested",
        RunEventKind::EffectDeferred => "effect_deferred",
        RunEventKind::EffectCompleted => "effect_completed",
        RunEventKind::EffectFailed => "effect_failed",
        RunEventKind::EffectCancelled => "effect_cancelled",
        RunEventKind::InteractionRequested => "interaction_requested",
        RunEventKind::InteractionResolved => "interaction_resolved",
        RunEventKind::InteractionExpired => "interaction_expired",
        RunEventKind::InteractionCancelled => "interaction_cancelled",
        RunEventKind::MessageFinalized => "message_finalized",
        RunEventKind::ToolSettled => "tool_settled",
        RunEventKind::LimitReached => "limit_reached",
        RunEventKind::RunSuspended => "run_suspended",
        RunEventKind::RunCompleted => "run_completed",
        RunEventKind::RunFailed => "run_failed",
        RunEventKind::RunCancelled => "run_cancelled",
        RunEventKind::ModelTextDelta => "model_text_delta",
        RunEventKind::ReasoningDelta => "reasoning_delta",
        RunEventKind::ToolProgress => "tool_progress",
        RunEventKind::QueueDepthWarning => "queue_depth_warning",
        RunEventKind::ProviderHeartbeat => "provider_heartbeat",
    }
}
