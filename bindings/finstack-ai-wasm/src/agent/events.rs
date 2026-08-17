use std::sync::{Arc, OnceLock};

use finstack_ai::runtime::{EventBatch as RuntimeEventBatch, RunEventClass, RunEventKind};
use finstack_ai_kernel::RunEvent;
use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;

/// Immutable runtime event handle.
#[wasm_bindgen(js_name = Event)]
pub struct Event {
    pub(super) inner: RunEvent,
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
    pub(super) inner: RuntimeEventBatch,
    pub(super) serialized: OnceLock<Result<Arc<[u8]>, ()>>,
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
