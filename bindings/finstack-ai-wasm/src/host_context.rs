//! Trusted JS / native host implementation of the `ContextProvider` port.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::context::{
    ContextCallContext, ContextContribution, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest,
};
use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ComponentRef, Digest, ErrorCategory, InvocationRecovery,
    Metadata,
};
use serde::Deserialize;

use crate::host::{HOST_VERSION, HostFailure, parse_host_json};

#[cfg(not(target_arch = "wasm32"))]
use crate::host::NativeHostResult;

/// Constructor options for a JS context-provider wrapper.
#[derive(Debug, Clone, Deserialize)]
pub struct HostContextOptions {
    /// Exact component identity.
    pub component: String,
    /// Whether the provider may supply trusted application instructions.
    #[serde(default, alias = "trustedApplicationInstructions")]
    pub trusted_application_instructions: bool,
}

/// Host-backed context provider.
pub struct HostContextProvider {
    descriptor: ContextProviderDescriptor,
    #[cfg(not(target_arch = "wasm32"))]
    callback: crate::host::NativeJsonCallback,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    collect: js_sys::Function,
}

fn context_failure(failure: HostFailure) -> ContextError {
    ContextError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Context),
        failure.message(),
        Metadata::empty(),
    )
    .unwrap_or_else(ContextError::from)
}

impl HostContextProvider {
    fn from_parts(
        options: &HostContextOptions,
        #[cfg(not(target_arch = "wasm32"))] callback: crate::host::NativeJsonCallback,
        #[cfg(target_arch = "wasm32")] adapter: wasm_bindgen::JsValue,
        #[cfg(target_arch = "wasm32")] collect: js_sys::Function,
    ) -> Result<Self, ContextError> {
        let component = ComponentRef::new(
            ComponentId::parse(&options.component)
                .map_err(|_| context_failure(HostFailure::InvalidResult))?,
            Some(HOST_VERSION),
        );
        Ok(Self {
            descriptor: ContextProviderDescriptor {
                invocation: ComponentInvocation {
                    component: component.id().clone(),
                    version: HOST_VERSION,
                    configuration_digest: Digest::raw_json(b"{}"),
                    recovery: InvocationRecovery::NonRepeatable,
                },
                trusted_application_instructions: options.trusted_application_instructions,
                metadata: Metadata::empty(),
            },
            #[cfg(not(target_arch = "wasm32"))]
            callback,
            #[cfg(target_arch = "wasm32")]
            adapter,
            #[cfg(target_arch = "wasm32")]
            collect,
        })
    }

    /// Construct a native host context provider.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError`] when the component identity is invalid.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        options: &HostContextOptions,
        callback: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
    ) -> Result<Self, ContextError> {
        Self::from_parts(options, Arc::new(callback))
    }

    /// Construct a wasm32 host context provider.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError`] when the adapter or options are invalid.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(
        adapter: wasm_bindgen::JsValue,
        options: HostContextOptions,
    ) -> Result<Self, ContextError> {
        let collect = crate::host::extract_method(&adapter, "collect").map_err(context_failure)?;
        Self::from_parts(&options, adapter, collect)
    }

    /// Exact registered component identity.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn component_ref(&self) -> ComponentRef {
        ComponentRef::new(
            self.descriptor.invocation.component.clone(),
            Some(self.descriptor.invocation.version),
        )
    }
}

impl ContextProvider for HostContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        if ctx.run.cancellation.is_cancelled() {
            return Box::pin(async { Err(context_failure(HostFailure::Cancelled)) });
        }
        let Ok(encoded) = serde_json::to_string(&request) else {
            return Box::pin(async { Err(context_failure(HostFailure::InvalidResult)) });
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let callback = Arc::clone(&self.callback);
            Box::pin(async move {
                let NativeHostResult::Object(body) = callback(&encoded).map_err(context_failure)?
                else {
                    return Err(context_failure(HostFailure::InvalidResult));
                };
                let contribution: ContextContribution =
                    parse_host_json(&body).map_err(context_failure)?;
                contribution.to_raw_json()?;
                Ok(contribution)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.collect.clone();
            Box::pin(async move {
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[wasm_bindgen::JsValue::from_str(&encoded)],
                    None,
                )
                .await
                .map_err(context_failure)?;
                let crate::host::HostJsResult::Value(value) = result else {
                    return Err(context_failure(HostFailure::InvalidResult));
                };
                let body = crate::host::stringify_js(&value).map_err(context_failure)?;
                let contribution: ContextContribution =
                    parse_host_json(&body).map_err(context_failure)?;
                contribution.to_raw_json()?;
                Ok(contribution)
            })
        }
    }
}
