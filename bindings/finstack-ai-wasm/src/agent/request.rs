use std::time::Duration;

use finstack_ai::runtime::ports::model::ModelName;
use finstack_ai::{
    AgentRunRequest, AttachmentInput, DEFAULT_MAX_CYCLES, DEFAULT_MAX_OUTPUT_RETRIES,
    DEFAULT_RUN_TIMEOUT, MAX_CONFIGURED_CYCLES, MAX_CONFIGURED_OUTPUT_RETRIES, PrincipalRef,
    RunSecurityContext,
};
use finstack_ai_kernel::{CapabilityId, ComponentId, ComponentRef, Version};
use wasm_bindgen::prelude::*;

use super::errors::{agent_error, configuration_error};

const PREVIEW_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

#[expect(
    clippy::too_many_arguments,
    reason = "run_request forwards model, bounds, capability, and attachments distinctly"
)]
pub(super) fn run_request(
    model: &ModelName,
    tenant_scope: &str,
    input: String,
    timeout_seconds: Option<f64>,
    max_cycles: Option<f64>,
    max_output_retries: Option<f64>,
    capability: Option<String>,
    attachments: Vec<AttachmentInput>,
) -> Result<AgentRunRequest, JsValue> {
    let timeout = match timeout_seconds {
        None => DEFAULT_RUN_TIMEOUT,
        Some(value) => {
            if !value.is_finite() || value <= 0.0 {
                return Err(agent_error(
                    &configuration_error("timeoutSeconds must be finite and positive"),
                    None,
                ));
            }
            Duration::try_from_secs_f64(value).map_err(|_| {
                agent_error(&configuration_error("timeoutSeconds is out of range"), None)
            })?
        }
    };
    let max_cycles = optional_u64(
        max_cycles,
        DEFAULT_MAX_CYCLES,
        MAX_CONFIGURED_CYCLES,
        "maxCycles",
    )?;
    let max_output_retries = u32::try_from(optional_u64(
        max_output_retries,
        u64::from(DEFAULT_MAX_OUTPUT_RETRIES),
        u64::from(MAX_CONFIGURED_OUTPUT_RETRIES),
        "maxOutputRetries",
    )?)
    .map_err(|_| {
        agent_error(
            &configuration_error("maxOutputRetries is out of range"),
            None,
        )
    })?;
    let security = RunSecurityContext::try_new(
        tenant_scope,
        PrincipalRef::try_new("finstack-ai-wasm", "local-user", Some(tenant_scope))
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        "local",
        "js-embedded",
        "js-policy-v1",
        "js-decision-v1",
        None,
    )
    .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?;
    let mut request = AgentRunRequest::try_new(model.clone(), input, security)
        .map_err(|error| agent_error(&error, None))?;
    request.timeout = timeout;
    request.max_cycles = max_cycles;
    request.max_output_retries = max_output_retries;
    if let Some(capability) = capability {
        request.capability = Some(
            CapabilityId::parse(&capability)
                .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        );
    }
    request.attachments = attachments.into();
    Ok(request)
}

fn optional_u64(value: Option<f64>, default: u64, max: u64, name: &str) -> Result<u64, JsValue> {
    let Some(value) = value else {
        return Ok(default);
    };
    if !value.is_finite() || value < 1.0 || value.fract() != 0.0 || value > max as f64 {
        return Err(agent_error(
            &configuration_error(format!("{name} must be a positive integer in 1..={max}")),
            None,
        ));
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite integer was range-checked against max"
    )]
    Ok(value as u64)
}

pub(super) fn component(id: &str) -> Result<ComponentRef, JsValue> {
    Ok(ComponentRef::new(
        ComponentId::parse(id)
            .map_err(|error| agent_error(&configuration_error(error.to_string()), None))?,
        Some(PREVIEW_VERSION),
    ))
}
