//! Trusted JS / native host implementation of the Middleware port.

use std::sync::Arc;

use finstack_ai::runtime::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRole, OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
};
use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ComponentRef, Digest, ErrorCategory, InvocationRecovery,
    Metadata, Stage,
};
use serde::Deserialize;

use crate::host::{HostFailure, host_component_version, parse_host_json};

#[cfg(not(target_arch = "wasm32"))]
use crate::host::NativeHostResult;

/// Constructor options for a JS middleware wrapper.
#[derive(Debug, Clone, Deserialize)]
pub struct HostMiddlewareOptions {
    /// Exact component identity.
    pub component: String,
    /// Stage names (`before_run`, `before_model`, ...).
    pub stages: Vec<String>,
    /// Standard-tier priority.
    #[serde(default)]
    pub priority: i32,
}

/// Host-backed middleware.
pub struct HostMiddleware {
    descriptor: MiddlewareDescriptor,
    #[cfg(not(target_arch = "wasm32"))]
    callback: crate::host::NativeJsonCallback,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    invoke: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
}

fn middleware_failure(failure: HostFailure) -> MiddlewareError {
    MiddlewareError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Middleware),
        failure.message(),
        Metadata::empty(),
    )
    .expect("frozen JS middleware host error is valid")
}

fn parse_stage(value: &str) -> Result<Stage, HostFailure> {
    match value {
        "before_run" => Ok(Stage::BeforeRun),
        "prepare_context" => Ok(Stage::PrepareContext),
        "before_model" => Ok(Stage::BeforeModel),
        "after_model" => Ok(Stage::AfterModel),
        "before_tool_batch" => Ok(Stage::BeforeToolBatch),
        "after_tool_batch" => Ok(Stage::AfterToolBatch),
        "before_finalize" => Ok(Stage::BeforeFinalize),
        _ => Err(HostFailure::InvalidResult),
    }
}

impl HostMiddleware {
    fn from_parts(
        options: &HostMiddlewareOptions,
        #[cfg(not(target_arch = "wasm32"))] callback: crate::host::NativeJsonCallback,
        #[cfg(target_arch = "wasm32")] adapter: wasm_bindgen::JsValue,
        #[cfg(target_arch = "wasm32")] invoke: js_sys::Function,
    ) -> Result<Self, MiddlewareError> {
        let stages = options
            .stages
            .iter()
            .map(|stage| parse_stage(stage))
            .collect::<Result<Vec<_>, _>>()
            .map_err(middleware_failure)?;
        if stages.is_empty() {
            return Err(middleware_failure(HostFailure::InvalidResult));
        }
        let component = ComponentRef::new(
            ComponentId::parse(&options.component)
                .map_err(|_| middleware_failure(HostFailure::InvalidResult))?,
            Some(host_component_version()),
        );
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: component.id().clone(),
                    version: host_component_version(),
                    configuration_digest: Digest::raw_json(b"{}"),
                    recovery: InvocationRecovery::NonRepeatable,
                },
                stages: StageMask::from_stages(stages),
                order: MiddlewareOrder {
                    tier: OrderTier::Standard,
                    priority: options.priority,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            #[cfg(not(target_arch = "wasm32"))]
            callback,
            #[cfg(target_arch = "wasm32")]
            adapter,
            #[cfg(target_arch = "wasm32")]
            invoke: std::rc::Rc::new(std::cell::RefCell::new(invoke)),
        })
    }

    /// Construct a native host middleware.
    ///
    /// # Errors
    ///
    /// Returns [`MiddlewareError`] when stages or the component identity are invalid.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        options: &HostMiddlewareOptions,
        callback: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
    ) -> Result<Self, MiddlewareError> {
        Self::from_parts(options, Arc::new(callback))
    }

    /// Construct a wasm32 host middleware.
    ///
    /// # Errors
    ///
    /// Returns [`MiddlewareError`] when the adapter or options are invalid.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(
        adapter: wasm_bindgen::JsValue,
        options: HostMiddlewareOptions,
    ) -> Result<Self, MiddlewareError> {
        let invoke = crate::host::extract_method(&adapter, "invoke").map_err(middleware_failure)?;
        Self::from_parts(&options, adapter, invoke)
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

impl Middleware for HostMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        if ctx.run.cancellation.is_cancelled() {
            return Box::pin(async { Err(middleware_failure(HostFailure::Cancelled)) });
        }
        let Ok(encoded) = serde_json::to_string(&input) else {
            return Box::pin(async { Err(middleware_failure(HostFailure::InvalidResult)) });
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let callback = Arc::clone(&self.callback);
            Box::pin(async move {
                let NativeHostResult::Object(body) =
                    callback(&encoded).map_err(middleware_failure)?
                else {
                    return Err(middleware_failure(HostFailure::InvalidResult));
                };
                let outcome: StageOutcome = parse_host_json(&body).map_err(middleware_failure)?;
                outcome.to_raw_json()?;
                Ok(outcome)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let adapter = self.adapter.clone();
            let method = self.invoke.borrow().clone();
            Box::pin(async move {
                let result = crate::host::invoke_host(
                    &adapter,
                    &method,
                    &[crate::host::json_string_value(&encoded)],
                    None,
                )
                .await
                .map_err(middleware_failure)?;
                let crate::host::HostJsResult::Value(value) = result else {
                    return Err(middleware_failure(HostFailure::InvalidResult));
                };
                let body = crate::host::stringify_js(&value).map_err(middleware_failure)?;
                let outcome: StageOutcome = parse_host_json(&body).map_err(middleware_failure)?;
                outcome.to_raw_json()?;
                Ok(outcome)
            })
        }
    }
}
