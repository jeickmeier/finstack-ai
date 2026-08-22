//! Trusted JS / native host implementation of the Model port.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::model::{
    InputCapabilities, Model, ModelCapabilities, ModelContextProfile, ModelDescriptor, ModelError,
    ModelEventStream, ModelName, ModelRequest, ModelStreamItem, ModelTokenEstimate,
    StructuredOutputCapability, TextDelta, TokenEstimatorSource, ToolCallDelta,
};
use finstack_ai_kernel::{ComponentId, ComponentRef, Metadata};
use futures_util::stream;

use crate::host::{
    HostFailure, HostModelOptions, completed_from_stream_item, host_component_version,
    js_estimator, model_failure, model_response, parse_host_json,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::host::NativeHostResult;

/// Host-backed Model port. Local (`!Send`) on wasm32; `Send + Sync` on native.
pub struct HostModel {
    #[allow(dead_code)]
    component: ComponentRef,
    descriptor: ModelDescriptor,
    capabilities: ModelCapabilities,
    #[cfg(not(target_arch = "wasm32"))]
    callback: crate::host::NativeJsonCallback,
    #[cfg(target_arch = "wasm32")]
    adapter: wasm_bindgen::JsValue,
    #[cfg(target_arch = "wasm32")]
    request: std::rc::Rc<std::cell::RefCell<js_sys::Function>>,
}

impl HostModel {
    fn from_parts(
        options: HostModelOptions,
        #[cfg(not(target_arch = "wasm32"))] callback: crate::host::NativeJsonCallback,
        #[cfg(target_arch = "wasm32")] adapter: wasm_bindgen::JsValue,
        #[cfg(target_arch = "wasm32")] request: js_sys::Function,
    ) -> Result<Self, ModelError> {
        let component = ComponentRef::new(
            ComponentId::parse(&options.component)
                .map_err(|_| model_failure(HostFailure::InvalidResult))?,
            Some(host_component_version()),
        );
        let model = ModelName::try_new(options.model)
            .map_err(|_| model_failure(HostFailure::InvalidResult))?;
        let profile = ModelContextProfile {
            provider: Arc::from(options.provider.as_str()),
            model: model.clone(),
            hard_input_bytes: options.hard_input_bytes,
            context_window_tokens: options.context_window_tokens,
            max_output_tokens: options.max_output_tokens,
            reserved_output_tokens: options.max_output_tokens,
            provider_overhead_tokens: 0,
            estimator: js_estimator(),
        };
        let capabilities = ModelCapabilities {
            input: InputCapabilities {
                text: true,
                json: true,
                images: false,
                audio: false,
                files: false,
            },
            context_profile: profile,
            native_tool_calls: true,
            parallel_tool_calls: true,
            structured_output: StructuredOutputCapability::Native,
            reasoning: false,
            prompt_cache: false,
            resumable_stream: false,
            idempotent_requests: false,
            native_capabilities: BTreeSet::new(),
        };
        let descriptor = ModelDescriptor {
            provider: Arc::from(options.provider),
            models: Arc::from([model]),
            metadata: Metadata::empty(),
        };
        descriptor
            .validate()
            .map_err(|_| model_failure(HostFailure::InvalidResult))?;
        Ok(Self {
            component,
            descriptor,
            capabilities,
            #[cfg(not(target_arch = "wasm32"))]
            callback,
            #[cfg(target_arch = "wasm32")]
            adapter,
            #[cfg(target_arch = "wasm32")]
            request: std::rc::Rc::new(std::cell::RefCell::new(request)),
        })
    }

    /// Construct a native host model over a JSON callback.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when constructor options are invalid.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_callback(
        options: HostModelOptions,
        callback: impl Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync + 'static,
    ) -> Result<Self, ModelError> {
        Self::from_parts(options, Arc::new(callback))
    }

    /// Construct a wasm32 host model around a JS `HostModel` object.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when the adapter or options are invalid.
    #[cfg(target_arch = "wasm32")]
    pub fn from_js(
        adapter: wasm_bindgen::JsValue,
        options: HostModelOptions,
    ) -> Result<Self, ModelError> {
        let request = crate::host::extract_method(&adapter, "request").map_err(model_failure)?;
        Self::from_parts(options, adapter, request)
    }

    /// Exact registered component identity.
    #[must_use]
    #[allow(dead_code)]
    pub fn component(&self) -> &ComponentRef {
        &self.component
    }
}

impl Model for HostModel {
    fn descriptor(&self) -> ModelDescriptor {
        self.descriptor.clone()
    }

    fn capabilities(&self, _model: &ModelName) -> ModelCapabilities {
        self.capabilities.clone()
    }

    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        if !self.descriptor.models.contains(model) {
            return Err(model_failure(HostFailure::InvalidResult));
        }
        Ok(ModelTokenEstimate {
            input_tokens: u64::try_from(canonical_request.len())
                .map_err(|_| model_failure(HostFailure::InvalidResult))?,
            estimator: finstack_ai::runtime::ports::model::TokenEstimatorRef {
                id: Arc::from("finstack.js.bytes-upper-bound"),
                version: Arc::from("1"),
                source: TokenEstimatorSource::ConservativeUpperBound,
            },
        })
    }

    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        if request.call.run.cancellation.is_cancelled() {
            return Box::pin(async { Err(model_failure(HostFailure::Cancelled)) });
        }
        let Ok(draft) = serde_json::to_string(&request.draft) else {
            return Box::pin(async { Err(model_failure(HostFailure::InvalidResult)) });
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let callback = Arc::clone(&self.callback);
            Box::pin(async move {
                let result = callback(&draft).map_err(model_failure)?;
                items_from_native(result)
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.request_with_js_signal(request, draft, None)
        }
    }
}

impl HostModel {
    /// Drive a model request while propagating a JS `AbortSignal`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn request_with_js_signal(
        &self,
        request: ModelRequest,
        draft: String,
        signal: Option<wasm_bindgen::JsValue>,
    ) -> PortFuture<Result<ModelEventStream, ModelError>> {
        let adapter = self.adapter.clone();
        let method = self.request.borrow().clone();
        let cancellation = request.call.run.cancellation.clone();
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
            if cancellation.is_cancelled()
                || effective.as_ref().is_some_and(|value| {
                    js_sys::Reflect::get(value, &wasm_bindgen::JsValue::from_str("aborted"))
                        .ok()
                        .and_then(|aborted| aborted.as_bool())
                        .unwrap_or(false)
                })
            {
                return Err(model_failure(HostFailure::Cancelled));
            }
            let draft_value = crate::host::json_string_value(&draft);
            let positional = [draft_value];
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
                    return Err(model_failure(HostFailure::Cancelled));
                }
            };
            abort_on_drop.disarm();
            items_from_js(result.map_err(model_failure)?)
        })
    }
}

fn items_from_response(
    response: finstack_ai::runtime::ports::model::ModelResponse,
) -> Result<ModelEventStream, ModelError> {
    let mut items = Vec::with_capacity(2);
    if let Some(text) = response
        .assistant_content
        .first()
        .and_then(|block| match block {
            finstack_ai_kernel::ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        && !text.is_empty()
    {
        items.push(ModelStreamItem::TextDelta(TextDelta {
            text: Arc::from(text),
        }));
    }
    for (index, call) in response.tool_calls.iter().enumerate() {
        items.push(ModelStreamItem::ToolCallDelta(ToolCallDelta {
            index: u32::try_from(index).map_err(|_| model_failure(HostFailure::InvalidResult))?,
            name: Some(Arc::clone(&call.name)),
            arguments_delta: Arc::from(call.arguments.as_str()),
            provider_call_id: None,
        }));
    }
    items.push(ModelStreamItem::Completed(response));
    Ok(Box::pin(stream::iter(items.into_iter().map(Ok))) as ModelEventStream)
}

#[cfg(not(target_arch = "wasm32"))]
fn items_from_native(result: NativeHostResult) -> Result<ModelEventStream, ModelError> {
    match result {
        NativeHostResult::Object(encoded) => {
            let output = parse_host_json(&encoded).map_err(model_failure)?;
            items_from_response(model_response(output).map_err(model_failure)?)
        }
        NativeHostResult::Items(items) => items_from_encoded(&items),
    }
}

fn items_from_encoded(items: &[String]) -> Result<ModelEventStream, ModelError> {
    let mut stream_items = Vec::new();
    let mut completed = None;
    let mut streamed_text = String::new();
    for encoded in items {
        let item: crate::host::HostModelStreamItem =
            parse_host_json(encoded).map_err(model_failure)?;
        if item_has_completion(&item) {
            let mut output = completed_from_stream_item(item).map_err(model_failure)?;
            if output.text.is_empty() {
                output.text.clone_from(&streamed_text);
            }
            completed = Some(model_response(output).map_err(model_failure)?);
        } else if let Some(text) = item_text(&item) {
            streamed_text.push_str(&text);
            stream_items.push(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from(text),
            }));
        } else if let Some((index, name, arguments_delta)) = item_tool_delta(&item) {
            stream_items.push(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index,
                name,
                arguments_delta: Arc::from(arguments_delta),
                provider_call_id: None,
            }));
        } else {
            return Err(model_failure(HostFailure::InvalidResult));
        }
    }
    let response = completed.ok_or_else(|| model_failure(HostFailure::InvalidResult))?;
    stream_items.push(ModelStreamItem::Completed(response));
    Ok(Box::pin(stream::iter(stream_items.into_iter().map(Ok))) as ModelEventStream)
}

fn item_has_completion(item: &crate::host::HostModelStreamItem) -> bool {
    item.completion_id.as_ref().is_some_and(|id| !id.is_empty())
}

fn item_text(item: &crate::host::HostModelStreamItem) -> Option<String> {
    item.text.as_ref().filter(|text| !text.is_empty()).cloned()
}

fn item_tool_delta(
    item: &crate::host::HostModelStreamItem,
) -> Option<(u32, Option<Arc<str>>, String)> {
    Some((
        item.index?,
        item.name.as_ref().map(|name| Arc::from(name.as_str())),
        item.arguments_delta.clone().unwrap_or_default(),
    ))
}

#[cfg(target_arch = "wasm32")]
fn items_from_js(result: crate::host::HostJsResult) -> Result<ModelEventStream, ModelError> {
    match result {
        crate::host::HostJsResult::Value(value) => {
            let encoded = crate::host::stringify_js(&value).map_err(model_failure)?;
            items_from_native_object(&encoded)
        }
        crate::host::HostJsResult::Items(values) => {
            let encoded = values
                .iter()
                .map(|value| crate::host::stringify_js(value).map_err(model_failure))
                .collect::<Result<Vec<_>, _>>()?;
            items_from_encoded(&encoded)
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn items_from_native_object(encoded: &str) -> Result<ModelEventStream, ModelError> {
    let output = parse_host_json(encoded).map_err(model_failure)?;
    items_from_response(model_response(output).map_err(model_failure)?)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::HostModel;
    use crate::executor::block_on_ready;
    use crate::fixture::model_request;
    use crate::host::{HostModelOptions, NativeHostResult};
    use finstack_ai::runtime::ports::model::{CancellationSignal, Model, ModelStreamItem};

    fn options() -> HostModelOptions {
        HostModelOptions {
            component: "js.model.fixture".into(),
            provider: "js-fixture".into(),
            model: "js-fixture-model".into(),
            hard_input_bytes: 1_048_576,
            context_window_tokens: 8_192,
            max_output_tokens: 1_024,
        }
    }

    fn collect(
        model: &HostModel,
        cancellation: CancellationSignal,
    ) -> Result<Vec<ModelStreamItem>, finstack_ai::runtime::ports::model::ModelError> {
        let request = model_request(&model.descriptor().models[0], cancellation).expect("request");
        let stream = block_on_ready(model.request(request))?;
        let mut items = Vec::new();
        let mut stream = stream;
        let waker = core::task::Waker::noop();
        let mut context = core::task::Context::from_waker(waker);
        loop {
            match stream.as_mut().poll_next(&mut context) {
                core::task::Poll::Ready(Some(Ok(item))) => items.push(item),
                core::task::Poll::Ready(Some(Err(error))) => return Err(error),
                core::task::Poll::Ready(None) => break,
                core::task::Poll::Pending => panic!("native host stream was pending"),
            }
        }
        Ok(items)
    }

    #[test]
    fn native_host_model_completes_coarse_object() {
        let model = HostModel::from_callback(options(), |_| {
            Ok(NativeHostResult::Object(
                r#"{"text":"hello from JS","completion_id":"js-1"}"#.into(),
            ))
        })
        .expect("model");
        let items = collect(&model, CancellationSignal::new()).expect("items");
        let ModelStreamItem::Completed(response) = items.last().expect("terminal") else {
            panic!("expected completed");
        };
        assert_eq!(response.completion_id.as_ref(), "js-1");
        assert_eq!(
            response
                .assistant_content
                .first()
                .and_then(|block| match block {
                    finstack_ai_kernel::ContentBlock::Text(text) => Some(text.text()),
                    _ => None,
                }),
            Some("hello from JS")
        );
    }

    #[test]
    fn native_host_model_accepts_item_stream_without_per_token_hook() {
        let model = HostModel::from_callback(options(), |_| {
            Ok(NativeHostResult::Items(vec![
                r#"{"text":"hel"}"#.into(),
                r#"{"text":"lo","completion_id":"js-stream-1"}"#.into(),
            ]))
        })
        .expect("model");
        let items = collect(&model, CancellationSignal::new()).expect("items");
        assert!(matches!(items[0], ModelStreamItem::TextDelta(_)));
        let ModelStreamItem::Completed(response) = items.last().expect("terminal") else {
            panic!("expected completed");
        };
        assert_eq!(response.completion_id.as_ref(), "js-stream-1");
    }

    #[test]
    fn native_host_model_rejects_unknown_fields() {
        let model = HostModel::from_callback(options(), |_| {
            Ok(NativeHostResult::Object(
                r#"{"text":"hello","completion_id":"js-1","unexpected":true}"#.into(),
            ))
        })
        .expect("model");
        let error = collect(&model, CancellationSignal::new()).expect_err("invalid");
        assert_eq!(error.code(), crate::host::JS_HOST_RESULT_INVALID);
    }

    #[test]
    fn native_host_model_maps_pre_cancelled_signal() {
        let model = HostModel::from_callback(options(), |_| {
            panic!("callback must not run after cancel");
        })
        .expect("model");
        let cancellation = CancellationSignal::new();
        cancellation.cancel();
        let error = collect(&model, cancellation).expect_err("cancelled");
        assert_eq!(error.code(), crate::host::JS_HOST_CANCELLED);
        assert_eq!(
            error.category(),
            finstack_ai_kernel::ErrorCategory::Cancellation
        );
    }

    #[test]
    fn native_host_model_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HostModel>();
    }
}
