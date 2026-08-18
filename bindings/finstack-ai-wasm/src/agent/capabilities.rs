use std::sync::Arc;

use finstack_ai::runtime::CapabilityId;
use finstack_ai::{
    ActiveCapability, CapabilityActivation, CapabilityActivationSource, CapabilityCatalogEntry,
    CapabilitySpec, InstructionSpec,
};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use super::errors::{agent_error, configuration_error};

/// WASM Capability stays instruction-only (id, description, instructions,
/// activation). Native Rust may attach executable refs; this wire cannot.
#[derive(Debug, Deserialize)]
struct JsCapabilityWire {
    id: String,
    description: String,
    instructions: Vec<String>,
    #[serde(default)]
    activation: Option<String>,
}

pub(super) fn parse_capabilities(json: Option<&str>) -> Result<Vec<CapabilitySpec>, JsValue> {
    let Some(json) = json.filter(|value| !value.is_empty() && *value != "undefined") else {
        return Ok(Vec::new());
    };
    let wires: Vec<JsCapabilityWire> = serde_json::from_str(json)
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    wires
        .into_iter()
        .map(capability_from_wire)
        .collect::<Result<Vec<_>, _>>()
}

pub(super) fn parse_active_capabilities(json: Option<&str>) -> Result<Vec<CapabilityId>, JsValue> {
    let Some(json) = json.filter(|value| !value.is_empty() && *value != "undefined") else {
        return Ok(Vec::new());
    };
    let ids: Vec<String> = serde_json::from_str(json)
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    ids.into_iter()
        .map(|id| {
            CapabilityId::parse(id)
                .map_err(|error| agent_error(&configuration_error(error.to_string()), None))
        })
        .collect()
}

fn capability_from_wire(wire: JsCapabilityWire) -> Result<CapabilitySpec, JsValue> {
    let activation = match wire.activation.as_deref().unwrap_or("application") {
        "always" => CapabilityActivation::Always,
        "application" => CapabilityActivation::Application,
        "model" => CapabilityActivation::Model,
        "disabled" => CapabilityActivation::Disabled,
        other => {
            return Err(agent_error(
                &configuration_error(format!(
                    "activation must be always, application, model, or disabled: {other}"
                )),
                None,
            ));
        }
    };
    let instructions = wire
        .instructions
        .into_iter()
        .map(InstructionSpec::try_new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let capability = CapabilitySpec {
        id: CapabilityId::parse(wire.id)
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        description: Arc::from(wire.description),
        instructions: instructions.into(),
        toolsets: Arc::from([]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        activation,
    };
    capability
        .validate()
        .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    Ok(capability)
}

pub(super) fn catalog_array(entries: Vec<CapabilityCatalogEntry>) -> Result<JsValue, JsValue> {
    let array = js_sys::Array::new();
    for entry in entries {
        let object = js_sys::Object::new();
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("id"),
            &JsValue::from_str(entry.id().as_str()),
        )?;
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("description"),
            &JsValue::from_str(entry.description()),
        )?;
        array.push(&object);
    }
    Ok(array.into())
}

pub(super) fn active_capability_array(active: &[ActiveCapability]) -> Result<JsValue, JsValue> {
    let array = js_sys::Array::new();
    for item in active {
        let object = js_sys::Object::new();
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("id"),
            &JsValue::from_str(item.capability_id.as_str()),
        )?;
        js_sys::Reflect::set(
            &object,
            &JsValue::from_str("source"),
            &JsValue::from_str(match item.source {
                CapabilityActivationSource::Always => "always",
                CapabilityActivationSource::Application => "application",
                CapabilityActivationSource::Model => "model",
            }),
        )?;
        array.push(&object);
    }
    Ok(array.into())
}
