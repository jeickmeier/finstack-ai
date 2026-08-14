//! wasm-bindgen Agent / Run handles over Rust-owned state.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use finstack_ai::runtime::{
    Clock, ComponentId, ComponentRef, EventBatch as RuntimeEventBatch, JournalStore, ModelName,
    RandomSource, RunEventClass, RunEventKind, Version,
};
use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, Agent as FacadeAgent, AgentRun, AgentRunError,
    AgentRunOutput, AgentRunRequest, OperationLocator, PrincipalRef, RunSecurityContext,
};
use finstack_ai_kernel::RunEvent;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use js_sys::Uint8Array;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::executor;
use crate::{JsModel, JsToolset};

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
    pub fn create(
        model: &JsModel,
        toolsets: Vec<JsToolset>,
        instruction: Option<String>,
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
        executor::drive(async move {
            build_agent(model_name, model_component, model_port, ports, instruction)
                .await
                .map(JsValue::from)
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
    ) -> Result<Run, JsValue> {
        let request = run_request(
            &self.model,
            input,
            timeout_seconds,
            max_cycles,
            max_output_retries,
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
#[wasm_bindgen(js_name = Run)]
pub struct Run {
    inner: AgentRun,
}

#[wasm_bindgen(js_class = Run)]
impl Run {
    /// Immutable session locator for this run.
    #[wasm_bindgen(getter)]
    pub fn session(&self) -> Session {
        Session {
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

/// Read-only session locator.
#[wasm_bindgen(js_name = Session)]
pub struct Session {
    locator: OperationLocator,
}

#[wasm_bindgen(js_class = Session)]
impl Session {
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

    /// Session locator for the completed run.
    #[wasm_bindgen(getter)]
    pub fn session(&self) -> Session {
        Session {
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

async fn build_agent(
    model_name: ModelName,
    model_component: ComponentRef,
    model: Arc<dyn finstack_ai::runtime::Model>,
    toolsets: Vec<(ComponentRef, Arc<dyn finstack_ai::runtime::Toolset>)>,
    instruction: Option<String>,
) -> Result<Agent, JsValue> {
    let store: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 64,
            batches_per_session: 256,
            records_per_session: 4_096,
            snapshot_bytes: 64 * 1_024,
        })
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
    );
    let mut builder = FacadeAgent::builder(
        finstack_ai_kernel::AgentId::parse("js.agent.host")
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        finstack_ai_kernel::BundleId::parse("js.bundle.host")
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        (model_component, model),
        (component("js.store.memory")?, store),
    );
    for (component, toolset) in toolsets {
        builder = builder.toolset(component, toolset);
    }
    if let Some(instruction) = instruction {
        builder = builder
            .try_instruction(instruction)
            .map_err(|error| agent_error(&error, None))?;
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

fn run_request(
    model: &ModelName,
    input: String,
    timeout_seconds: Option<f64>,
    max_cycles: Option<f64>,
    max_output_retries: Option<f64>,
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

fn configuration_error(message: impl Into<String>) -> AgentRunError {
    AgentRunError::Configuration {
        code: AGENT_RUN_INVALID_CONFIGURATION,
        message: message.into(),
    }
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
