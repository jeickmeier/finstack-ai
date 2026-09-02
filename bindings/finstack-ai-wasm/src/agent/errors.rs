use finstack_ai::{AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, OperationLocator};
use wasm_bindgen::prelude::*;

const JS_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

pub(super) fn configuration_error(message: impl Into<String>) -> AgentRunError {
    AgentRunError::Configuration {
        code: finstack_ai_kernel::static_error_code!(AGENT_RUN_INVALID_CONFIGURATION),
        message: message.into(),
    }
}

pub(super) fn js_safe_integer(value: u64, field: &'static str) -> Result<f64, AgentRunError> {
    if value > JS_SAFE_INTEGER_MAX {
        return Err(configuration_error(format!(
            "{field} exceeds the JavaScript safe-integer range"
        )));
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "the explicit Number.MAX_SAFE_INTEGER check above makes this cast exact"
    )]
    Ok(value as f64)
}

/// Build a real `Error` carrying the stable `code`. `FinstackError.fromUnknown`
/// reads `code`/`retryable` only off values that are `instanceof Error`, and
/// falls back to `String(value)` otherwise, which renders a plain object as the
/// literal "[object Object]", losing the stable code *and* the message.
fn finstack_error(message: &str, code: &str, retryable: bool) -> js_sys::Error {
    let object = js_sys::Error::new(message);
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("name"),
        &JsValue::from_str("FinstackError"),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("code"),
        &JsValue::from_str(code),
    );
    let _ = js_sys::Reflect::set(
        &object,
        &JsValue::from_str("retryable"),
        &JsValue::from_bool(retryable),
    );
    object
}

pub(super) fn session_error(error: &finstack_ai::SessionError) -> JsValue {
    finstack_error(&error.to_string(), error.code(), false).into()
}

pub(super) fn agent_error(error: &AgentRunError, locator: Option<&OperationLocator>) -> JsValue {
    let object = finstack_error(&error.to_string(), error.code(), error.retryable());
    if let Some(locator) = locator
        && let Ok(context) = locator_object(locator)
    {
        let _ = js_sys::Reflect::set(&object, &JsValue::from_str("context"), &context);
    }
    object.into()
}

pub(super) fn locator_object(locator: &OperationLocator) -> Result<JsValue, JsValue> {
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

#[cfg(test)]
mod tests {
    use super::{JS_SAFE_INTEGER_MAX, js_safe_integer};

    #[test]
    fn javascript_integer_conversion_fails_closed_above_exact_range() {
        assert_eq!(
            js_safe_integer(JS_SAFE_INTEGER_MAX, "sequence").expect("safe"),
            9_007_199_254_740_991.0
        );
        assert!(js_safe_integer(JS_SAFE_INTEGER_MAX + 1, "sequence").is_err());
    }
}
