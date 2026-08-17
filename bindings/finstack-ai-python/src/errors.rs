//! Map Rust agent and session errors onto the stable `FinstackError` hierarchy.

use finstack_ai::runtime::{OperationLocator, SessionError};
use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_TIMEOUT, AgentRunError,
};
use pyo3::prelude::*;

use crate::locator::locator_dict;
use crate::{CancelledError, ConfigurationError, RuntimeError, TimeoutError};

#[expect(
    clippy::needless_pass_by_value,
    reason = "used directly as a Result::map_err adapter"
)]
pub(crate) fn model_configuration_error(error: finstack_ai::runtime::ModelError) -> AgentRunError {
    configuration_error(format!("{}: {}", error.code(), error.message()))
}

pub(crate) fn configuration_error(message: impl Into<String>) -> AgentRunError {
    AgentRunError::Configuration {
        code: AGENT_RUN_INVALID_CONFIGURATION,
        message: message.into(),
    }
}

pub(crate) fn agent_error(
    py: Python<'_>,
    error: &AgentRunError,
    locator: Option<&OperationLocator>,
) -> PyErr {
    let exception = match error.code() {
        AGENT_RUN_INVALID_CONFIGURATION => py.get_type::<ConfigurationError>(),
        AGENT_RUN_TIMEOUT => py.get_type::<TimeoutError>(),
        AGENT_RUN_CANCELLED => py.get_type::<CancelledError>(),
        _ => py.get_type::<RuntimeError>(),
    };
    let value = exception.call1((error.to_string(),));
    match value {
        Ok(value) => {
            let _ = value.setattr("code", error.code());
            let _ = value.setattr("retryable", error.retryable());
            if let Some(locator) = locator {
                if let Ok(context) = locator_dict(py, locator) {
                    let _ = value.setattr("context", context);
                }
            } else {
                let _ = value.setattr("context", py.None());
            }
            PyErr::from_value(value)
        }
        Err(construction_error) => construction_error,
    }
}

pub(crate) fn session_py_error(py: Python<'_>, error: &SessionError) -> PyErr {
    let value = py
        .get_type::<ConfigurationError>()
        .call1((error.to_string(),));
    match value {
        Ok(value) => {
            let _ = value.setattr("code", error.code());
            let _ = value.setattr("retryable", false);
            let _ = value.setattr("context", py.None());
            PyErr::from_value(value)
        }
        Err(construction_error) => construction_error,
    }
}
