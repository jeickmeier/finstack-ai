use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_RUNTIME_FAILURE,
    AGENT_RUN_TIMEOUT, AGENT_RUN_UNSUPPORTED_PLAN, AgentRunError, OperationLocator,
};
use wasm_bindgen::prelude::*;

pub(super) fn configuration_error(message: impl Into<String>) -> AgentRunError {
    AgentRunError::Configuration {
        code: AGENT_RUN_INVALID_CONFIGURATION,
        message: message.into(),
    }
}

pub(super) fn session_error(error: &finstack_ai::SessionError) -> JsValue {
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

pub(super) fn agent_error(error: &AgentRunError, locator: Option<&OperationLocator>) -> JsValue {
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
