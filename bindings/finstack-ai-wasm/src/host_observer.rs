//! Trusted JS / native host implementation of the Observer port.

use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::observer::{
    Observer, ObserverDescriptor, ObserverError, ObserverPayloadMode,
};
use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata, RunEvent, Sensitivity};
use serde::Deserialize;

use crate::host::{HOST_VERSION, HostFailure};

#[cfg(not(target_arch = "wasm32"))]
use crate::host::NativeHostResult;

/// Constructor options for a JS observer wrapper.
#[derive(Debug, Clone, Deserialize)]
pub struct HostObserverOptions {
    /// Exact component identity.
    pub component: String,
    /// Payload projection mode.
    #[serde(default = "default_payload_mode", alias = "payloadMode")]
    pub payload_mode: String,
}

fn default_payload_mode() -> String {
    "metadata_only".into()
}

fn parse_payload_mode(value: &str) -> Result<ObserverPayloadMode, HostFailure> {
    match value {
        "metadata_only" => Ok(ObserverPayloadMode::MetadataOnly),
        "redacted" => Ok(ObserverPayloadMode::Redacted),
        "full" => Ok(ObserverPayloadMode::Full),
        _ => Err(HostFailure::InvalidResult),
    }
}

/// Host-backed observer.
pub struct HostObserver {
    descriptor: ObserverDescriptor,
    #[cfg(not(target_arch = "wasm32"))]
    callback: crate::host::NativeJsonCallback,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    observe: js_sys::Function,
}

impl HostObserver {
    fn from_parts(
        options: &HostObserverOptions,
        #[cfg(not(target_arch = "wasm32"))] callback: crate::host::NativeJsonCallback,
        #[cfg(target_arch = "wasm32")] adapter: wasm_bindgen::JsValue,
        #[cfg(target_arch = "wasm32")] observe: js_sys::Function,
    ) -> Result<Self, ObserverError> {
        let component = ComponentRef::new(
            ComponentId::parse(&options.component)
                .map_err(|_| ObserverError::ConfigurationInvalid)?,
            Some(HOST_VERSION),
        );
        let payload_mode = parse_payload_mode(&options.payload_mode)
            .map_err(|_| ObserverError::ConfigurationInvalid)?;
        Ok(Self {
            descriptor: ObserverDescriptor {
                component,
                payload_mode,
                metadata: Metadata::empty(),
            },
            #[cfg(not(target_arch = "wasm32"))]
            callback,
            #[cfg(target_arch = "wasm32")]
            adapter,
            #[cfg(target_arch = "wasm32")]
            observe,
        })
    }

    /// Construct a native host observer.
    ///
    /// # Errors
    ///
    /// Returns [`ObserverError::ConfigurationInvalid`] when options are invalid.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        options: &HostObserverOptions,
        callback: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
    ) -> Result<Self, ObserverError> {
        Self::from_parts(options, Arc::new(callback))
    }

    /// Construct a wasm32 host observer.
    ///
    /// # Errors
    ///
    /// Returns [`ObserverError`] when the adapter or options are invalid.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(
        adapter: wasm_bindgen::JsValue,
        options: HostObserverOptions,
    ) -> Result<Self, ObserverError> {
        let observe = crate::host::extract_method(&adapter, "observe")
            .map_err(|_| ObserverError::Unavailable)?;
        Self::from_parts(&options, adapter, observe)
    }

    /// Exact registered component identity.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn component_ref(&self) -> ComponentRef {
        self.descriptor.component.clone()
    }
}

impl Observer for HostObserver {
    fn descriptor(&self) -> ObserverDescriptor {
        self.descriptor.clone()
    }

    fn observe(&self, batch: Arc<[RunEvent]>) -> PortFuture<Result<(), ObserverError>> {
        let mode = self.descriptor.payload_mode;
        let Ok(projected) = batch
            .iter()
            .map(|event| observer_event(event, mode))
            .collect::<Result<Vec<_>, _>>()
        else {
            return Box::pin(async { Err(ObserverError::Unavailable) });
        };
        let Ok(encoded) = serde_json::to_string(&projected) else {
            return Box::pin(async { Err(ObserverError::Unavailable) });
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let callback = Arc::clone(&self.callback);
            Box::pin(async move {
                callback(&encoded).map_err(|_| ObserverError::Unavailable)?;
                Ok(())
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.observe.clone();
            Box::pin(async move {
                crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[wasm_bindgen::JsValue::from_str(&encoded)],
                    None,
                )
                .await
                .map_err(|_| ObserverError::Unavailable)?;
                Ok(())
            })
        }
    }
}

fn observer_event(event: &RunEvent, mode: ObserverPayloadMode) -> Result<serde_json::Value, ()> {
    let mut value = serde_json::to_value(event).map_err(|_| ())?;
    let include_body = match mode {
        ObserverPayloadMode::MetadataOnly => false,
        ObserverPayloadMode::Redacted => event.sensitivity() == Sensitivity::Public,
        ObserverPayloadMode::Full => event.sensitivity() != Sensitivity::Credential,
    };
    if !include_body {
        value.as_object_mut().ok_or(())?.remove("body");
    }
    Ok(value)
}
