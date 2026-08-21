//! Trusted JS / native host implementation of the Toolset port.

use std::sync::Arc;

use finstack_ai::runtime::{
    PortFuture, ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolSpec, ToolStreamItem,
    Toolset, ToolsetDescriptor,
};
use finstack_ai_kernel::{ComponentId, ComponentRef, ErrorCategory, Metadata, ValidatedToolCall};
use futures_util::stream;
use serde::Deserialize;

use crate::host::{HostFailure, host_component_version, parse_host_json, tool_output_bytes};

#[cfg(not(target_arch = "wasm32"))]
use crate::host::NativeHostResult;

/// Constructor options for a JS toolset wrapper.
#[derive(Debug, Clone, Deserialize)]
pub struct HostToolsetOptions {
    /// Exact component identity.
    pub component: String,
    /// Toolset name.
    pub name: String,
    /// Cached tool schemas.
    pub tools: Vec<ToolSpec>,
}

/// Host-backed Toolset port.
pub struct HostToolset {
    #[allow(dead_code)]
    component: ComponentRef,
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    #[cfg(not(target_arch = "wasm32"))]
    callback: crate::host::NativePairCallback,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    call: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
}

fn tool_failure(failure: HostFailure) -> ToolError {
    ToolError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Tool),
        false,
        failure.message(),
        Metadata::empty(),
    )
    .unwrap_or_else(ToolError::from)
}

impl HostToolset {
    fn from_parts(
        options: HostToolsetOptions,
        #[cfg(not(target_arch = "wasm32"))] callback: crate::host::NativePairCallback,
        #[cfg(target_arch = "wasm32")] adapter: wasm_bindgen::JsValue,
        #[cfg(target_arch = "wasm32")] call: js_sys::Function,
    ) -> Result<Self, ToolError> {
        if options.tools.is_empty() {
            return Err(tool_failure(HostFailure::InvalidResult));
        }
        for tool in &options.tools {
            tool.validate()
                .map_err(|_| tool_failure(HostFailure::InvalidResult))?;
        }
        let component = ComponentRef::new(
            ComponentId::parse(&options.component)
                .map_err(|_| tool_failure(HostFailure::InvalidResult))?,
            Some(host_component_version()),
        );
        Ok(Self {
            component,
            descriptor: ToolsetDescriptor {
                name: Arc::from(options.name),
                metadata: Metadata::empty(),
            },
            tools: options.tools.into(),
            #[cfg(not(target_arch = "wasm32"))]
            callback,
            #[cfg(target_arch = "wasm32")]
            adapter,
            #[cfg(target_arch = "wasm32")]
            call: std::rc::Rc::new(std::cell::RefCell::new(call)),
        })
    }

    /// Construct a native host toolset over a JSON callback.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] when tools are empty or invalid.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        options: HostToolsetOptions,
        callback: impl Fn(&str, &str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
    ) -> Result<Self, ToolError> {
        Self::from_parts(options, Arc::new(callback))
    }

    /// Construct a wasm32 host toolset around a JS `HostToolset` object.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] when the adapter or options are invalid.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(
        adapter: wasm_bindgen::JsValue,
        options: HostToolsetOptions,
    ) -> Result<Self, ToolError> {
        let call = crate::host::extract_method(&adapter, "call").map_err(tool_failure)?;
        Self::from_parts(options, adapter, call)
    }

    /// Exact registered component identity.
    #[must_use]
    #[allow(dead_code)]
    pub fn component(&self) -> &ComponentRef {
        &self.component
    }

    /// Drive a tool call while propagating a JS `AbortSignal`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn call_with_js_signal(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
        signal: Option<wasm_bindgen::JsValue>,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let adapter = self.adapter.clone();
        let method = self.call.borrow().clone();
        let cancellation = ctx.run.cancellation.clone();
        Box::pin(async move {
            let owned = if signal.is_none() {
                crate::host::create_abort_controller().ok()
            } else {
                None
            };
            let effective = signal.or_else(|| owned.as_ref().map(|(_, value)| value.clone()));
            let mut abort_on_drop = crate::host::AbortOnDrop {
                controller: owned.as_ref().map(|(controller, _)| controller.clone()),
            };
            if cancellation.is_cancelled() {
                return Err(tool_failure(HostFailure::Cancelled));
            }
            let context_json = serde_json::to_string(&ctx.run.locator)
                .map_err(|_| tool_failure(HostFailure::InvalidResult))?;
            let call_json = serde_json::to_string(&call)
                .map_err(|_| tool_failure(HostFailure::InvalidResult))?;
            let positional = [
                crate::host::json_string_value(&context_json),
                crate::host::json_string_value(&call_json),
            ];
            let invoke =
                crate::host::invoke_host(&adapter, &method, &positional, effective.as_ref());
            let result = match futures_util::future::select(
                std::pin::pin!(invoke),
                std::pin::pin!(cancellation.cancelled()),
            )
            .await
            {
                futures_util::future::Either::Left((result, _)) => result,
                futures_util::future::Either::Right(((), invoke)) => {
                    drop(invoke);
                    return Err(tool_failure(HostFailure::Cancelled));
                }
            };
            abort_on_drop.disarm();
            tool_items_from_js(result.map_err(tool_failure)?)
        })
    }
}

impl Toolset for HostToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        if ctx.run.cancellation.is_cancelled() {
            return Box::pin(async { Err(tool_failure(HostFailure::Cancelled)) });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Ok(context_json) = serde_json::to_string(&ctx.run.locator) else {
                return Box::pin(async { Err(tool_failure(HostFailure::InvalidResult)) });
            };
            let Ok(call_json) = serde_json::to_string(&call) else {
                return Box::pin(async { Err(tool_failure(HostFailure::InvalidResult)) });
            };
            let callback = Arc::clone(&self.callback);
            Box::pin(async move {
                let result = callback(&context_json, &call_json).map_err(tool_failure)?;
                tool_items_from_native(result)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.call_with_js_signal(ctx, call, None)
        }
    }
}

fn completed_stream(
    result: ToolResult,
    artifacts: impl IntoIterator<Item = finstack_ai_kernel::ArtifactRef>,
) -> ToolEventStream {
    let items = artifacts
        .into_iter()
        .map(ToolStreamItem::Artifact)
        .chain(core::iter::once(ToolStreamItem::Completed(result)))
        .map(Ok)
        .collect::<Vec<_>>();
    Box::pin(stream::iter(items))
}

#[cfg(not(target_arch = "wasm32"))]
fn tool_items_from_native(result: NativeHostResult) -> Result<ToolEventStream, ToolError> {
    let encoded = match result {
        NativeHostResult::Object(encoded) => encoded,
        NativeHostResult::Items(items) => items
            .last()
            .cloned()
            .ok_or_else(|| tool_failure(HostFailure::InvalidResult))?,
    };
    let output = parse_host_json(&encoded).map_err(tool_failure)?;
    let (raw, is_error) = tool_output_bytes(&output).map_err(tool_failure)?;
    Ok(completed_stream(
        ToolResult {
            output: raw,
            is_error,
        },
        output.artifacts,
    ))
}

#[cfg(target_arch = "wasm32")]
fn tool_items_from_js(result: crate::host::HostJsResult) -> Result<ToolEventStream, ToolError> {
    let value = match result {
        crate::host::HostJsResult::Value(value) => value,
        crate::host::HostJsResult::Items(items) => items
            .into_iter()
            .last()
            .ok_or_else(|| tool_failure(HostFailure::InvalidResult))?,
    };
    let encoded = crate::host::stringify_js(&value).map_err(tool_failure)?;
    let output = parse_host_json(&encoded).map_err(tool_failure)?;
    let (raw, is_error) = tool_output_bytes(&output).map_err(tool_failure)?;
    Ok(completed_stream(
        ToolResult {
            output: raw,
            is_error,
        },
        output.artifacts,
    ))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{HostToolset, HostToolsetOptions};
    use crate::executor::block_on_ready;
    use crate::fixture::{echo_tool_json, tool_call};
    use crate::host::{JS_HOST_RESULT_INVALID, NativeHostResult};
    use finstack_ai::runtime::{CancellationSignal, ToolStreamItem, Toolset};
    use futures_util::Stream;

    fn options() -> HostToolsetOptions {
        HostToolsetOptions {
            component: "js.toolset.fixture".into(),
            name: "js-fixture-tools".into(),
            tools: vec![serde_json::from_str(echo_tool_json()).expect("tool")],
        }
    }

    #[test]
    fn native_host_toolset_completes_object_output() {
        let toolset = HostToolset::from_callback(options(), |_, _| {
            Ok(NativeHostResult::Object(
                r#"{"output":{"value":"hi"},"is_error":false}"#.into(),
            ))
        })
        .expect("toolset");
        let (ctx, call) = tool_call(CancellationSignal::new()).expect("call");
        let stream = block_on_ready(toolset.call(ctx, call)).expect("stream");
        let mut stream = stream;
        let waker = core::task::Waker::noop();
        let mut context = core::task::Context::from_waker(waker);
        let item = match Stream::poll_next(stream.as_mut(), &mut context) {
            core::task::Poll::Ready(Some(Ok(item))) => item,
            other => panic!("unexpected {other:?}"),
        };
        let ToolStreamItem::Completed(result) = item else {
            panic!("expected completed");
        };
        assert!(!result.is_error);
        assert!(result.output.as_str().contains("hi"));
    }

    #[test]
    fn native_host_toolset_rejects_non_object_output() {
        let toolset = HostToolset::from_callback(options(), |_, _| {
            Ok(NativeHostResult::Object(r#"{"output":"nope"}"#.into()))
        })
        .expect("toolset");
        let (ctx, call) = tool_call(CancellationSignal::new()).expect("call");
        let Err(error) = block_on_ready(toolset.call(ctx, call)) else {
            panic!("expected invalid tool output");
        };
        assert_eq!(error.code(), JS_HOST_RESULT_INVALID);
    }

    #[test]
    fn native_host_toolset_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HostToolset>();
    }
}
