use std::sync::Arc;

use finstack_ai::runtime::{
    ComponentRef, ErrorCategory, Metadata, Middleware, MiddlewareContext, MiddlewareDescriptor,
    MiddlewareError, MiddlewareOrder, MiddlewareRole, OrderTier, PortFuture, StageInput, StageMask,
    StageOutcome,
};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;

use super::context::PyCallbackContext;
use super::engine::{CallbackFailure, PythonCallback};
use super::shared::{callback_invocation, exact_component, parse_stage};

struct PythonMiddlewareAdapter {
    callback: Arc<PythonCallback>,
    descriptor: MiddlewareDescriptor,
}

impl Middleware for PythonMiddlewareAdapter {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let callback = Arc::clone(&self.callback);
        Box::pin(async move {
            let context = PyCallbackContext::new("middleware", &ctx.run);
            let outcome: StageOutcome = callback
                .invoke(context, &input)
                .await
                .map_err(middleware_failure)?;
            outcome.to_raw_json()?;
            Ok(outcome)
        })
    }
}

fn middleware_failure(failure: CallbackFailure) -> MiddlewareError {
    MiddlewareError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Middleware),
        failure.message(),
        Metadata::empty(),
    )
    .expect("frozen Python middleware callback error is valid")
}

/// Trusted Python implementation of the Middleware port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonMiddleware",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonMiddleware {
    inner: Arc<PythonMiddlewareAdapter>,
}

#[pymethods]
impl PyPythonMiddleware {
    /// Register one trusted coarse Python stage callback.
    #[new]
    #[pyo3(signature = (callback, *, component, stages, priority = 0, callback_timeout_seconds = 30.0))]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "PyO3 owns the extracted Python list at the constructor boundary"
    )]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        stages: Vec<String>,
        priority: i32,
        callback_timeout_seconds: f64,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        let stages = stages
            .iter()
            .map(|stage| parse_stage(stage))
            .collect::<PyResult<Vec<_>>>()?;
        if stages.is_empty() {
            return Err(PyTypeError::new_err("stages must not be empty"));
        }
        Ok(Self {
            inner: Arc::new(PythonMiddlewareAdapter {
                callback: Arc::new(PythonCallback::try_new(
                    py,
                    callback,
                    callback_timeout_seconds,
                )?),
                descriptor: MiddlewareDescriptor {
                    invocation: callback_invocation(&component),
                    stages: StageMask::from_stages(stages),
                    order: MiddlewareOrder {
                        tier: OrderTier::Standard,
                        priority,
                        before: Arc::from([]),
                        after: Arc::from([]),
                    },
                    role: MiddlewareRole::Standard,
                    metadata: Metadata::empty(),
                },
            }),
        })
    }

    /// Exact registered component identity.
    #[getter]
    fn component(&self) -> String {
        self.inner.descriptor.invocation.component.to_string()
    }
}

impl PyPythonMiddleware {
    /// Ready handle pair for [`finstack_ai::NativeAgentBuilder`].
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn Middleware>) {
        (
            ComponentRef::new(
                self.inner.descriptor.invocation.component.clone(),
                Some(self.inner.descriptor.invocation.version),
            ),
            Arc::clone(&self.inner) as Arc<dyn Middleware>,
        )
    }
}
