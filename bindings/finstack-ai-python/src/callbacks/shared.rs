use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ComponentRef, Digest, InvocationRecovery, Stage, Version,
};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;

const CALLBACK_VERSION: Version = Version {
    major: 0,
    minor: 0,
    patch: 1,
};

pub(super) fn callback_invocation(component: &ComponentRef) -> ComponentInvocation {
    ComponentInvocation {
        component: component.id().clone(),
        version: component.version().unwrap_or(CALLBACK_VERSION),
        configuration_digest: Digest::raw_json(b"{}"),
        recovery: InvocationRecovery::NonRepeatable,
    }
}

pub(super) fn parse_stage(value: &str) -> PyResult<Stage> {
    match value {
        "before_run" => Ok(Stage::BeforeRun),
        "prepare_context" => Ok(Stage::PrepareContext),
        "before_model" => Ok(Stage::BeforeModel),
        "after_model" => Ok(Stage::AfterModel),
        "before_tool_batch" => Ok(Stage::BeforeToolBatch),
        "after_tool_batch" => Ok(Stage::AfterToolBatch),
        "before_finalize" => Ok(Stage::BeforeFinalize),
        _ => Err(PyTypeError::new_err(format!(
            "unsupported middleware stage: {value}"
        ))),
    }
}

pub(super) fn exact_component(value: &str) -> PyResult<ComponentRef> {
    let id = ComponentId::parse(value).map_err(configuration_error)?;
    Ok(ComponentRef::new(id, Some(CALLBACK_VERSION)))
}

pub(super) fn configuration_error(error: impl std::fmt::Display) -> PyErr {
    PyTypeError::new_err(error.to_string())
}
