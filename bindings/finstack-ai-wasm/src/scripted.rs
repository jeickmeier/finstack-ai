//! Test-only scripted drives for host adapters. Not Agent-run parity.

use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai::runtime::ports::model::{CancellationSignal, Model, ModelStreamItem};
use finstack_ai::runtime::ports::tool::ToolStreamItem;
use futures_util::StreamExt;
use serde::Serialize;
use wasm_bindgen::JsValue;

use crate::host::{HostFailure, HostModelOptions, parse_host_json};
use crate::host_model::HostModel;
use crate::host_store::{HostJournalStore, HostJournalStoreOptions};
use crate::host_toolset::{HostToolset, HostToolsetOptions};

#[derive(Serialize)]
struct DriveError {
    ok: bool,
    code: String,
    category: String,
}

#[derive(Serialize)]
struct ModelDriveOk {
    ok: bool,
    text: String,
    completion_id: String,
    tool_calls: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct ToolDriveOk {
    ok: bool,
    output: serde_json::Value,
    is_error: bool,
}

fn encode_error(code: &str, category: &str) -> String {
    serde_json::to_string(&DriveError {
        ok: false,
        code: code.to_owned(),
        category: category.to_owned(),
    })
    .unwrap_or_else(|_| r#"{"ok":false,"code":"encode_failed","category":"internal"}"#.to_owned())
}

fn encode_host_failure(failure: HostFailure) -> String {
    encode_error(
        failure.code(),
        failure
            .category(finstack_ai_kernel::ErrorCategory::Model)
            .as_str(),
    )
}

fn js_error(message: &str) -> JsValue {
    js_sys::TypeError::new(message).into()
}

fn stringify_js(value: &JsValue) -> Result<String, JsValue> {
    js_sys::JSON::stringify(value)
        .map_err(|_| js_error("invalid host options"))?
        .as_string()
        .ok_or_else(|| js_error("invalid host options"))
}

/// Drive one `Model::request` against a JS host adapter.
///
/// # Errors
///
/// Returns a JavaScript exception when options cannot be parsed.
pub async fn drive_scripted_model_request(
    adapter: JsValue,
    options: JsValue,
    signal: JsValue,
) -> Result<JsValue, JsValue> {
    let options: HostModelOptions =
        parse_host_json(&stringify_js(&options)?).map_err(|_| js_error("invalid host options"))?;
    let model =
        HostModel::from_js(adapter, options).map_err(|_| js_error("invalid host options"))?;
    let cancellation = CancellationSignal::new();
    let request = crate::fixture::model_request(&model.descriptor().models[0], cancellation)
        .map_err(|_| js_error("invalid host options"))?;
    let draft =
        serde_json::to_string(&request.draft).map_err(|_| js_error("invalid host options"))?;
    let signal = if signal.is_undefined() || signal.is_null() {
        None
    } else {
        Some(signal)
    };
    let stream = match model.request_with_js_signal(request, draft, signal).await {
        Ok(stream) => stream,
        Err(error) => {
            return Ok(JsValue::from_str(&encode_error(
                error.code().as_str(),
                error.category().as_str(),
            )));
        }
    };
    let mut stream = stream;
    let mut completed = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(ModelStreamItem::Completed(response)) => completed = Some(response),
            Ok(_) => {}
            Err(error) => {
                return Ok(JsValue::from_str(&encode_error(
                    error.code().as_str(),
                    error.category().as_str(),
                )));
            }
        }
    }
    let response = completed.ok_or_else(|| js_error("invalid host options"))?;
    let text = response
        .assistant_content
        .first()
        .and_then(|block| match block {
            finstack_ai_kernel::ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .unwrap_or_default();
    let tool_calls = response
        .tool_calls
        .iter()
        .map(|call| {
            serde_json::json!({
                "name": call.name,
                "arguments": serde_json::from_str::<serde_json::Value>(call.arguments.as_str())
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect();
    let encoded = serde_json::to_string(&ModelDriveOk {
        ok: true,
        text,
        completion_id: response.completion_id.to_string(),
        tool_calls,
    })
    .map_err(|_| js_error("invalid host options"))?;
    Ok(JsValue::from_str(&encoded))
}

/// Drive one `Toolset::call` against a JS host adapter.
///
/// # Errors
///
/// Returns a JavaScript exception when options cannot be parsed.
pub async fn drive_scripted_tool_call(
    adapter: JsValue,
    options: JsValue,
    signal: JsValue,
) -> Result<JsValue, JsValue> {
    let options: HostToolsetOptions = serde_json::from_str(&stringify_js(&options)?)
        .map_err(|_| js_error("invalid host options"))?;
    let toolset =
        HostToolset::from_js(adapter, options).map_err(|_| js_error("invalid host options"))?;
    let cancellation = CancellationSignal::new();
    let (ctx, call) =
        crate::fixture::tool_call(cancellation).map_err(|_| js_error("invalid host options"))?;
    let signal = if signal.is_undefined() || signal.is_null() {
        None
    } else {
        Some(signal)
    };
    let stream = match toolset.call_with_js_signal(ctx, call, signal).await {
        Ok(stream) => stream,
        Err(error) => {
            return Ok(JsValue::from_str(&encode_error(
                error.code().as_str(),
                error.category().as_str(),
            )));
        }
    };
    let mut stream = stream;
    let mut completed = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(ToolStreamItem::Completed(result)) => completed = Some(result),
            Ok(_) => {}
            Err(error) => {
                return Ok(JsValue::from_str(&encode_error(
                    error.code().as_str(),
                    error.category().as_str(),
                )));
            }
        }
    }
    let result = completed.ok_or_else(|| js_error("invalid host options"))?;
    let output = serde_json::from_str(result.output.as_str())
        .map_err(|_| js_error("invalid host options"))?;
    let encoded = serde_json::to_string(&ToolDriveOk {
        ok: true,
        output,
        is_error: result.is_error,
    })
    .map_err(|_| js_error("invalid host options"))?;
    Ok(JsValue::from_str(&encoded))
}

/// Drive scripted journal-store health. Always reports `durable: false`.
///
/// # Errors
///
/// Returns a JavaScript exception when the adapter is invalid.
pub async fn drive_scripted_journal_health(
    adapter: JsValue,
    options: JsValue,
) -> Result<JsValue, JsValue> {
    let options: HostJournalStoreOptions = if options.is_undefined() || options.is_null() {
        HostJournalStoreOptions {
            detail: "js_memory_prebeta".into(),
        }
    } else {
        serde_json::from_str(&stringify_js(&options)?)
            .map_err(|_| js_error("invalid host options"))?
    };
    let store = HostJournalStore::from_js(adapter, options)
        .map_err(|_| js_error("invalid host options"))?;
    match store.health().await {
        Ok(health) => {
            let encoded = serde_json::json!({
                "ok": true,
                "ready": health.ready,
                "durable": health.durable,
                "detail": health.detail,
            })
            .to_string();
            Ok(JsValue::from_str(&encoded))
        }
        Err(_) => Ok(JsValue::from_str(&encode_host_failure(HostFailure::Failed))),
    }
}
