//! Map Rust agent and session errors onto the stable `FinstackError` hierarchy.
//!
//! Every constructor attaches to the interpreter itself, so callers on
//! Rust-owned runtime tasks need no `Python` marker; attaching from a thread
//! that already holds one is a no-op.

use finstack_ai::runtime::session::{SessionError, SessionErrorCategory};
use finstack_ai::{
    AGENT_RUN_CANCELLED, AGENT_RUN_INVALID_CONFIGURATION, AGENT_RUN_TIMEOUT, AgentRun,
    AgentRunError,
};
use finstack_ai_kernel::OperationLocator;
use pyo3::prelude::*;

use crate::locator::locator_dict;
use crate::{CancelledError, ConfigurationError, RuntimeError, TimeoutError};

pub(crate) fn configuration_error(message: impl Into<String>) -> AgentRunError {
    AgentRunError::Configuration {
        code: finstack_ai_kernel::static_error_code!(AGENT_RUN_INVALID_CONFIGURATION),
        message: message.into(),
    }
}

pub(crate) fn agent_error(error: &AgentRunError, locator: Option<&OperationLocator>) -> PyErr {
    Python::attach(|py| {
        let exception = match error.code() {
            AGENT_RUN_INVALID_CONFIGURATION => py.get_type::<ConfigurationError>(),
            AGENT_RUN_TIMEOUT => py.get_type::<TimeoutError>(),
            AGENT_RUN_CANCELLED => py.get_type::<CancelledError>(),
            _ => py.get_type::<RuntimeError>(),
        };
        let value = match exception.call1((error.to_string(),)) {
            Ok(value) => value,
            Err(construction_error) => return construction_error,
        };
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
    })
}

/// [`agent_error`] carrying `run`'s locator as the exception context.
pub(crate) fn run_error(run: &AgentRun, error: &AgentRunError) -> PyErr {
    agent_error(error, Some(run.locator()))
}

pub(crate) fn session_py_error(error: &SessionError) -> PyErr {
    Python::attach(|py| {
        let exception = match error.category() {
            SessionErrorCategory::Configuration => py.get_type::<ConfigurationError>(),
            SessionErrorCategory::Runtime => py.get_type::<RuntimeError>(),
        };
        let value = match exception.call1((error.to_string(),)) {
            Ok(value) => value,
            Err(construction_error) => return construction_error,
        };
        let _ = value.setattr("code", error.code());
        let _ = value.setattr("retryable", false);
        let _ = value.setattr("context", py.None());
        PyErr::from_value(value)
    })
}
