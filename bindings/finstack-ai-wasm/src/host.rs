//! Shared JavaScript host invoke, `AbortSignal`, and stable error mapping.

use std::sync::Arc;

use finstack_ai::runtime::{ModelError, ModelResponse, ModelToolCall};
use finstack_ai_kernel::{
    ArtifactRef, ContentBlock, ErrorCategory, JsonBlock, Metadata, ProviderIds, RawJson, TextBlock,
    Usage,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Stable cancellation code for a JS host adapter.
pub const JS_HOST_CANCELLED: &str = "js_host_cancelled";
/// Stable failure code for a JS host adapter.
pub const JS_HOST_FAILED: &str = "js_host_failed";
/// Stable validation code for a malformed JS host result.
pub const JS_HOST_RESULT_INVALID: &str = "js_host_result_invalid";

const HOST_VERSION: finstack_ai_kernel::Version = finstack_ai_kernel::Version {
    major: 0,
    minor: 0,
    patch: 1,
};
const JS_ESTIMATOR_ID: &str = "finstack.js.bytes-upper-bound";

/// Classified host-adapter failure. Durable diagnostics never include raw JS text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailure {
    /// The host promise or stream was cancelled.
    Cancelled,
    /// The host threw or rejected without a usable result.
    Failed,
    /// The host returned a value that failed DTO validation.
    InvalidResult,
}

impl HostFailure {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cancelled => JS_HOST_CANCELLED,
            Self::Failed => JS_HOST_FAILED,
            Self::InvalidResult => JS_HOST_RESULT_INVALID,
        }
    }

    /// Category for this failure, using `component` for generic host failures.
    #[must_use]
    pub const fn category(self, component: ErrorCategory) -> ErrorCategory {
        match self {
            Self::Cancelled => ErrorCategory::Cancellation,
            Self::Failed => component,
            Self::InvalidResult => ErrorCategory::Validation,
        }
    }

    /// Stable non-secret diagnostic.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "JavaScript host was cancelled",
            Self::Failed => "JavaScript host failed",
            Self::InvalidResult => "JavaScript host returned an invalid normalized result",
        }
    }
}

/// Coarse completed model object accepted from a trusted JS host.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostModelOutput {
    /// Assistant text when the host returns plain text.
    #[serde(default)]
    pub text: String,
    /// Structured JSON when the host returns a JSON completion.
    #[serde(default)]
    pub json: Option<serde_json::Value>,
    /// Non-empty provider completion identity.
    pub completion_id: String,
    /// Optional model-visible tool calls.
    #[serde(default)]
    pub tool_calls: Vec<HostModelToolCall>,
}

/// One model-visible tool call in a coarse host result.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostModelToolCall {
    name: String,
    arguments: serde_json::Value,
    #[serde(default)]
    provider_call_id: Option<String>,
}

/// Incremental stream item accepted from a `ReadableStream` or async iterable.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostModelStreamItem {
    /// Incremental or completed assistant text.
    #[serde(default)]
    pub text: Option<String>,
    /// Terminal completion identity when this item completes the stream.
    #[serde(default)]
    pub completion_id: Option<String>,
    /// Terminal tool calls when this item completes the stream.
    #[serde(default)]
    pub tool_calls: Option<Vec<HostModelToolCall>>,
    /// Terminal JSON payload when this item completes the stream.
    #[serde(default)]
    pub json: Option<serde_json::Value>,
    /// Tool-call index for an incremental tool-call delta.
    #[serde(default)]
    pub index: Option<u32>,
    /// Tool-call name for an incremental tool-call delta.
    #[serde(default)]
    pub name: Option<String>,
    /// Incremental tool-call argument fragment.
    #[serde(default)]
    pub arguments_delta: Option<String>,
}

/// Coarse completed tool object accepted from a trusted JS host.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostToolOutput {
    output: serde_json::Value,
    #[serde(default)]
    is_error: bool,
    /// Exact references staged by the trusted host before completion.
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
}

/// Constructor options shared by JS model wrappers.
#[derive(Debug, Clone, Deserialize)]
pub struct HostModelOptions {
    /// Exact component identity.
    pub component: String,
    /// Provider label.
    pub provider: String,
    /// Model name.
    pub model: String,
    /// Canonical request byte ceiling.
    #[serde(default = "default_hard_input_bytes", alias = "hardInputBytes")]
    pub hard_input_bytes: u64,
    /// Context-window token ceiling.
    #[serde(
        default = "default_context_window_tokens",
        alias = "contextWindowTokens"
    )]
    pub context_window_tokens: u64,
    /// Maximum output tokens.
    #[serde(default = "default_max_output_tokens", alias = "maxOutputTokens")]
    pub max_output_tokens: u64,
}

const fn default_hard_input_bytes() -> u64 {
    1_048_576
}

const fn default_context_window_tokens() -> u64 {
    8_192
}

const fn default_max_output_tokens() -> u64 {
    1_024
}

/// Parse a JSON DTO and reject unknown fields.
///
/// # Errors
///
/// Returns [`HostFailure::InvalidResult`] when the payload is not the expected DTO.
pub fn parse_host_json<T: DeserializeOwned>(encoded: &str) -> Result<T, HostFailure> {
    serde_json::from_str(encoded).map_err(|_| HostFailure::InvalidResult)
}

/// Convert a coarse host model object into a runtime response.
///
/// # Errors
///
/// Returns [`HostFailure::InvalidResult`] when required fields are empty or contradictory.
pub fn model_response(output: HostModelOutput) -> Result<ModelResponse, HostFailure> {
    if output.completion_id.is_empty() || output.completion_id.as_bytes().contains(&0) {
        return Err(HostFailure::InvalidResult);
    }
    if !output.text.is_empty() && output.json.is_some() {
        return Err(HostFailure::InvalidResult);
    }
    let assistant_content = if let Some(value) = output.json {
        let bytes = serde_json::to_vec(&value).map_err(|_| HostFailure::InvalidResult)?;
        Arc::from([ContentBlock::Json(JsonBlock::new(
            RawJson::parse(bytes).map_err(|_| HostFailure::InvalidResult)?,
        ))])
    } else if !output.text.is_empty() {
        Arc::from([ContentBlock::Text(
            TextBlock::try_new(&output.text).map_err(|_| HostFailure::InvalidResult)?,
        )])
    } else {
        Arc::from([])
    };
    let tool_calls = output
        .tool_calls
        .into_iter()
        .map(|call| {
            let arguments =
                serde_json::to_vec(&call.arguments).map_err(|_| HostFailure::InvalidResult)?;
            Ok(ModelToolCall {
                name: Arc::from(call.name),
                arguments: RawJson::parse(arguments).map_err(|_| HostFailure::InvalidResult)?,
                provider_call_id: call.provider_call_id.map(Arc::from),
            })
        })
        .collect::<Result<Vec<_>, HostFailure>>()?;
    Ok(ModelResponse {
        assistant_content,
        tool_calls: tool_calls.into(),
        usage: Usage::empty(),
        provider_ids: ProviderIds::empty(),
        completion_id: Arc::from(output.completion_id),
        continuation_state: None,
    })
}

/// Convert a stream item that carries a completion identity into a coarse object.
///
/// # Errors
///
/// Returns [`HostFailure::InvalidResult`] when the item cannot form a completed object.
pub fn completed_from_stream_item(
    item: HostModelStreamItem,
) -> Result<HostModelOutput, HostFailure> {
    let completion_id = item
        .completion_id
        .filter(|value| !value.is_empty())
        .ok_or(HostFailure::InvalidResult)?;
    Ok(HostModelOutput {
        text: item.text.unwrap_or_default(),
        json: item.json,
        completion_id,
        tool_calls: item.tool_calls.unwrap_or_default(),
    })
}

/// Convert a coarse tool object into canonical bytes plus the error flag.
///
/// # Errors
///
/// Returns [`HostFailure::InvalidResult`] when `output` is not JSON-serializable.
pub fn tool_output_bytes(output: &HostToolOutput) -> Result<(RawJson, bool), HostFailure> {
    if !output.output.is_object() && !output.output.is_array() {
        return Err(HostFailure::InvalidResult);
    }
    let bytes = serde_json::to_vec(&output.output).map_err(|_| HostFailure::InvalidResult)?;
    Ok((
        RawJson::parse(bytes).map_err(|_| HostFailure::InvalidResult)?,
        output.is_error,
    ))
}

/// Frozen callback-component version used by JS wrappers.
#[must_use]
pub const fn host_component_version() -> finstack_ai_kernel::Version {
    HOST_VERSION
}

/// Conservative byte-upper-bound estimator identity for JS hosts.
#[must_use]
pub fn js_estimator() -> finstack_ai::runtime::TokenEstimatorRef {
    finstack_ai::runtime::TokenEstimatorRef {
        id: Arc::from(JS_ESTIMATOR_ID),
        version: Arc::from("1"),
        source: finstack_ai::runtime::TokenEstimatorSource::ConservativeUpperBound,
    }
}

/// Construct a model adapter error from a host failure.
#[must_use]
pub fn model_failure(failure: HostFailure) -> ModelError {
    ModelError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Model),
        false,
        failure.message(),
        Metadata::empty(),
    )
    .unwrap_or_else(ModelError::from)
}

/// Parse constructor options from a JSON object.
///
/// # Errors
///
/// Returns [`HostFailure::InvalidResult`] when required fields are missing.
#[allow(dead_code)]
pub fn parse_model_options(encoded: &str) -> Result<HostModelOptions, HostFailure> {
    parse_host_json(encoded)
}

#[cfg(target_arch = "wasm32")]
mod wasm_invoke {
    use super::HostFailure;
    use core::cell::{Cell, RefCell};
    use core::future::Future;
    use core::pin::Pin;
    use core::task::{Context, Poll, Waker};
    use std::rc::Rc;

    use futures_util::future::{Either, select};
    use js_sys::{Function, Promise, Reflect, Symbol, Uint8Array};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    /// Create a host `AbortController` and its `AbortSignal`.
    pub fn create_abort_controller() -> Result<(JsValue, JsValue), HostFailure> {
        let global = js_sys::global();
        let ctor = Reflect::get(&global, &JsValue::from_str("AbortController"))
            .map_err(|_| HostFailure::Failed)?;
        let ctor = ctor
            .dyn_into::<Function>()
            .map_err(|_| HostFailure::Failed)?;
        let controller =
            Reflect::construct(&ctor, &js_sys::Array::new()).map_err(|_| HostFailure::Failed)?;
        let signal = Reflect::get(&controller, &JsValue::from_str("signal"))
            .map_err(|_| HostFailure::Failed)?;
        Ok((controller, signal))
    }

    /// Abort a host `AbortController` created by [`create_abort_controller`].
    pub fn abort_controller(controller: &JsValue) {
        if let Ok(abort) = Reflect::get(controller, &JsValue::from_str("abort"))
            && abort.is_function()
        {
            let _ = Function::from(abort).call0(controller);
        }
    }

    /// Abort an owned controller if the host invoke is dropped before settle.
    pub struct AbortOnDrop {
        pub controller: Option<JsValue>,
    }

    impl AbortOnDrop {
        pub fn disarm(&mut self) {
            self.controller = None;
        }
    }

    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            if let Some(controller) = self.controller.take() {
                abort_controller(&controller);
            }
        }
    }

    /// Invoke a host method with JSON-string arguments and an optional AbortSignal.
    pub async fn invoke_host(
        this: &JsValue,
        method: &Function,
        positional: &[JsValue],
        signal: Option<&JsValue>,
    ) -> Result<HostJsResult, HostFailure> {
        if let Some(signal) = signal
            && is_aborted(signal)
        {
            return Err(HostFailure::Cancelled);
        }
        let options = js_sys::Object::new();
        if let Some(signal) = signal {
            Reflect::set(&options, &JsValue::from_str("signal"), signal)
                .map_err(|_| HostFailure::Failed)?;
        }
        let args = js_sys::Array::new();
        for value in positional {
            args.push(value);
        }
        args.push(&options);
        let result = method.apply(this, &args).map_err(|_| HostFailure::Failed)?;
        let awaited = await_with_abort(result, signal).await?;
        normalize_js_result(awaited, signal).await
    }

    /// Invoke a host method and preserve the raw JavaScript rejection value.
    pub async fn invoke_host_raw(
        this: &JsValue,
        method: &Function,
        positional: &[JsValue],
        signal: Option<&JsValue>,
    ) -> Result<JsValue, JsValue> {
        let options = js_sys::Object::new();
        if let Some(signal) = signal {
            Reflect::set(&options, &JsValue::from_str("signal"), signal)?;
        }
        let args = js_sys::Array::new();
        for value in positional {
            args.push(value);
        }
        args.push(&options);
        let result = method.apply(this, &args)?;
        if is_promise(&result) {
            JsFuture::from(Promise::from(result)).await
        } else {
            Ok(result)
        }
    }

    /// Return a host method when present and callable.
    pub fn extract_optional_method(adapter: &JsValue, name: &str) -> Option<Function> {
        extract_method(adapter, name).ok()
    }

    /// Normalized host return: one object or collected stream items.
    pub enum HostJsResult {
        Value(JsValue),
        Items(Vec<JsValue>),
    }

    pub async fn normalize_js_result(
        value: JsValue,
        signal: Option<&JsValue>,
    ) -> Result<HostJsResult, HostFailure> {
        if value.is_undefined() || value.is_null() {
            return Err(HostFailure::InvalidResult);
        }
        if is_readable_stream(&value) {
            return Ok(HostJsResult::Items(
                read_readable_stream(value, signal).await?,
            ));
        }
        if is_async_iterable(&value) {
            return Ok(HostJsResult::Items(
                read_async_iterable(value, signal).await?,
            ));
        }
        Ok(HostJsResult::Value(value))
    }

    pub fn stringify_js(value: &JsValue) -> Result<String, HostFailure> {
        js_sys::JSON::stringify(value)
            .ok()
            .and_then(|encoded| encoded.as_string())
            .ok_or(HostFailure::InvalidResult)
    }

    pub fn json_string_value(encoded: &str) -> JsValue {
        JsValue::from_str(encoded)
    }

    pub fn extract_method(adapter: &JsValue, name: &str) -> Result<Function, HostFailure> {
        let value =
            Reflect::get(adapter, &JsValue::from_str(name)).map_err(|_| HostFailure::Failed)?;
        value
            .dyn_into::<Function>()
            .map_err(|_| HostFailure::Failed)
    }

    pub fn uint8_array_from_bytes(bytes: &[u8]) -> Uint8Array {
        let array = Uint8Array::new_with_length(u32::try_from(bytes.len()).unwrap_or(u32::MAX));
        array.copy_from(bytes);
        array
    }

    pub fn bytes_from_uint8_array(value: &JsValue) -> Result<Vec<u8>, HostFailure> {
        let array = Uint8Array::new(value);
        let mut bytes = vec![0_u8; array.length() as usize];
        array.copy_to(&mut bytes);
        Ok(bytes)
    }

    fn is_aborted(signal: &JsValue) -> bool {
        Reflect::get(signal, &JsValue::from_str("aborted"))
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    fn is_promise(value: &JsValue) -> bool {
        value.has_type::<Promise>()
            || Reflect::get(value, &JsValue::from_str("then"))
                .ok()
                .is_some_and(|then| then.is_function())
    }

    fn is_readable_stream(value: &JsValue) -> bool {
        Reflect::get(value, &JsValue::from_str("getReader"))
            .ok()
            .is_some_and(|reader| reader.is_function())
    }

    fn is_async_iterable(value: &JsValue) -> bool {
        Reflect::get(value, &Symbol::async_iterator())
            .ok()
            .is_some_and(|iter| iter.is_function())
    }

    fn map_js_error(error: JsValue) -> HostFailure {
        let name = Reflect::get(&error, &JsValue::from_str("name"))
            .ok()
            .and_then(|value| value.as_string())
            .unwrap_or_default();
        if name == "AbortError" {
            HostFailure::Cancelled
        } else {
            HostFailure::Failed
        }
    }

    async fn await_with_abort(
        value: JsValue,
        signal: Option<&JsValue>,
    ) -> Result<JsValue, HostFailure> {
        if !is_promise(&value) {
            return Ok(value);
        }
        let promise = Promise::from(value);
        let future = JsFuture::from(promise);
        if let Some(signal) = signal {
            if is_aborted(signal) {
                drop(future);
                return Err(HostFailure::Cancelled);
            }
            let abort = watch_abort(signal.clone());
            match select(future, abort).await {
                Either::Left((Ok(value), _)) => Ok(value),
                Either::Left((Err(error), _)) => Err(map_js_error(error)),
                Either::Right(((), future)) => {
                    drop(future);
                    Err(HostFailure::Cancelled)
                }
            }
        } else {
            future.await.map_err(map_js_error)
        }
    }

    async fn read_readable_stream(
        value: JsValue,
        signal: Option<&JsValue>,
    ) -> Result<Vec<JsValue>, HostFailure> {
        let get_reader = Reflect::get(&value, &JsValue::from_str("getReader"))
            .map_err(|_| HostFailure::InvalidResult)?;
        let get_reader: Function = get_reader
            .dyn_into()
            .map_err(|_| HostFailure::InvalidResult)?;
        let reader = get_reader.call0(&value).map_err(|_| HostFailure::Failed)?;
        let read = Reflect::get(&reader, &JsValue::from_str("read"))
            .map_err(|_| HostFailure::InvalidResult)?;
        let read: Function = read.dyn_into().map_err(|_| HostFailure::InvalidResult)?;
        let mut items = Vec::new();
        loop {
            if let Some(signal) = signal
                && is_aborted(signal)
            {
                cancel_reader(&reader);
                return Err(HostFailure::Cancelled);
            }
            let next = read.call0(&reader).map_err(|_| HostFailure::Failed)?;
            let chunk = await_with_abort(next, signal).await?;
            let done = Reflect::get(&chunk, &JsValue::from_str("done"))
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if done {
                break;
            }
            items.push(
                Reflect::get(&chunk, &JsValue::from_str("value"))
                    .map_err(|_| HostFailure::InvalidResult)?,
            );
        }
        Ok(items)
    }

    fn cancel_reader(reader: &JsValue) {
        if let Ok(cancel) = Reflect::get(reader, &JsValue::from_str("cancel"))
            && cancel.is_function()
        {
            let _ = Function::from(cancel).call0(reader);
        }
    }

    async fn read_async_iterable(
        value: JsValue,
        signal: Option<&JsValue>,
    ) -> Result<Vec<JsValue>, HostFailure> {
        let factory = Reflect::get(&value, &Symbol::async_iterator())
            .map_err(|_| HostFailure::InvalidResult)?;
        let factory: Function = factory.dyn_into().map_err(|_| HostFailure::InvalidResult)?;
        let iterator = factory.call0(&value).map_err(|_| HostFailure::Failed)?;
        let next = Reflect::get(&iterator, &JsValue::from_str("next"))
            .map_err(|_| HostFailure::InvalidResult)?;
        let next: Function = next.dyn_into().map_err(|_| HostFailure::InvalidResult)?;
        let mut items = Vec::new();
        loop {
            if let Some(signal) = signal
                && is_aborted(signal)
            {
                return Err(HostFailure::Cancelled);
            }
            let step = next.call0(&iterator).map_err(|_| HostFailure::Failed)?;
            let chunk = await_with_abort(step, signal).await?;
            let done = Reflect::get(&chunk, &JsValue::from_str("done"))
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if done {
                break;
            }
            items.push(
                Reflect::get(&chunk, &JsValue::from_str("value"))
                    .map_err(|_| HostFailure::InvalidResult)?,
            );
        }
        Ok(items)
    }

    fn watch_abort(signal: JsValue) -> AbortReady {
        let done = Rc::new(Cell::new(is_aborted(&signal)));
        let waker = Rc::new(RefCell::new(None::<Waker>));
        let done_cb = Rc::clone(&done);
        let waker_cb = Rc::clone(&waker);
        let closure = Closure::wrap(Box::new(move || {
            done_cb.set(true);
            if let Some(waker) = waker_cb.borrow_mut().take() {
                waker.wake();
            }
        }) as Box<dyn FnMut()>);
        if !done.get()
            && let Ok(add) = Reflect::get(&signal, &JsValue::from_str("addEventListener"))
            && add.is_function()
        {
            let _ = Function::from(add).call2(
                &signal,
                &JsValue::from_str("abort"),
                closure.as_ref().unchecked_ref(),
            );
        }
        AbortReady {
            signal,
            done,
            waker,
            _closure: closure,
        }
    }

    struct AbortReady {
        signal: JsValue,
        done: Rc<Cell<bool>>,
        waker: Rc<RefCell<Option<Waker>>>,
        _closure: Closure<dyn FnMut()>,
    }

    impl Future for AbortReady {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.done.get() || is_aborted(&self.signal) {
                Poll::Ready(())
            } else {
                *self.waker.borrow_mut() = Some(cx.waker().clone());
                if is_aborted(&self.signal) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_invoke::{
    AbortOnDrop, HostJsResult, bytes_from_uint8_array, create_abort_controller, extract_method,
    extract_optional_method, invoke_host, invoke_host_raw, json_string_value, stringify_js,
    uint8_array_from_bytes,
};

/// Native callback result used to prove DTO and stream-item paths without JS.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub enum NativeHostResult {
    /// One completed JSON object.
    Object(String),
    /// Collected stream items as JSON objects.
    #[allow(dead_code)]
    Items(Vec<String>),
}

/// Native JSON callback used by host adapters in native tests.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type NativeJsonCallback =
    std::sync::Arc<dyn Fn(&str) -> Result<NativeHostResult, HostFailure> + Send + Sync>;

/// Native two-argument JSON callback used by the toolset adapter.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type NativePairCallback =
    std::sync::Arc<dyn Fn(&str, &str) -> Result<NativeHostResult, HostFailure> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::{
        HostFailure, HostModelOutput, HostToolOutput, JS_HOST_CANCELLED, JS_HOST_FAILED,
        JS_HOST_RESULT_INVALID, model_failure, model_response, parse_host_json, tool_output_bytes,
    };
    use finstack_ai_kernel::ErrorCategory;

    #[test]
    fn host_failure_codes_are_stable() {
        assert_eq!(HostFailure::Cancelled.code(), JS_HOST_CANCELLED);
        assert_eq!(HostFailure::Failed.code(), JS_HOST_FAILED);
        assert_eq!(HostFailure::InvalidResult.code(), JS_HOST_RESULT_INVALID);
        assert_eq!(
            HostFailure::Cancelled.category(ErrorCategory::Model),
            ErrorCategory::Cancellation
        );
        assert_eq!(
            HostFailure::InvalidResult.category(ErrorCategory::Tool),
            ErrorCategory::Validation
        );
        let error = model_failure(HostFailure::Cancelled);
        assert_eq!(error.code(), JS_HOST_CANCELLED);
        assert_eq!(error.category(), ErrorCategory::Cancellation);
        assert!(!error.to_string().contains("TypeError"));
    }

    #[test]
    fn model_output_requires_completion_id_and_rejects_extras() {
        let valid: HostModelOutput =
            parse_host_json(r#"{"text":"hello","completion_id":"js-1"}"#).expect("valid");
        let response = model_response(valid).expect("response");
        assert_eq!(response.completion_id.as_ref(), "js-1");
        assert!(parse_host_json::<HostModelOutput>(r#"{"text":"hello"}"#).is_err());
        assert!(
            parse_host_json::<HostModelOutput>(
                r#"{"text":"hello","completion_id":"js-1","unexpected":true}"#
            )
            .is_err()
        );
    }

    #[test]
    fn tool_output_rejects_non_object_payloads() {
        let valid: HostToolOutput =
            parse_host_json(r#"{"output":{"value":"hi"},"is_error":false}"#).expect("valid");
        let (raw, is_error) = tool_output_bytes(&valid).expect("bytes");
        assert!(!is_error);
        assert!(raw.as_str().contains("hi"));
        let scalar: HostToolOutput = parse_host_json(r#"{"output":"nope"}"#).expect("scalar");
        assert!(tool_output_bytes(&scalar).is_err());
    }
}
