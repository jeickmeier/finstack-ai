use std::sync::Arc;

use finstack_ai::runtime::ports::PortFuture;
use finstack_ai::runtime::ports::context::{
    ContextCallContext, ContextContribution, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest,
};
use finstack_ai_kernel::{ComponentRef, ErrorCategory, Metadata};
use pyo3::prelude::*;

use super::context::PyCallbackContext;
use super::engine::{CallbackFailure, PythonCallback};
use super::shared::{callback_invocation, exact_component};

struct PythonContextProviderAdapter {
    callback: Arc<PythonCallback>,
    descriptor: ContextProviderDescriptor,
}

impl ContextProvider for PythonContextProviderAdapter {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let callback = Arc::clone(&self.callback);
        Box::pin(async move {
            let context = PyCallbackContext::new("context", &ctx.run);
            let contribution: ContextContribution = callback
                .invoke(context, &request)
                .await
                .map_err(context_failure)?;
            contribution.to_raw_json()?;
            Ok(contribution)
        })
    }
}

fn context_failure(failure: CallbackFailure) -> ContextError {
    ContextError::try_new(
        failure.code(),
        failure.category(ErrorCategory::Context),
        failure.message(),
        Metadata::empty(),
    )
    .unwrap_or_else(ContextError::from)
}

/// Trusted Python implementation of the `ContextProvider` port.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "PythonContextProvider",
    frozen,
    skip_from_py_object
)]
#[derive(Clone)]
pub(crate) struct PyPythonContextProvider {
    inner: Arc<PythonContextProviderAdapter>,
}

#[pymethods]
impl PyPythonContextProvider {
    /// Register one trusted coarse Python context-provider callback.
    #[new]
    #[pyo3(signature = (callback, *, component, callback_timeout_seconds = 30.0, trusted_application_instructions = false))]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        component: &str,
        callback_timeout_seconds: f64,
        trusted_application_instructions: bool,
    ) -> PyResult<Self> {
        let component = exact_component(component)?;
        Ok(Self {
            inner: Arc::new(PythonContextProviderAdapter {
                callback: Arc::new(PythonCallback::try_new(
                    py,
                    callback,
                    callback_timeout_seconds,
                )?),
                descriptor: ContextProviderDescriptor {
                    invocation: callback_invocation(&component),
                    trusted_application_instructions,
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

impl PyPythonContextProvider {
    /// Ready handle pair for [`finstack_ai::NativeAgentBuilder`].
    pub(crate) fn registration(&self) -> (ComponentRef, Arc<dyn ContextProvider>) {
        (
            ComponentRef::new(
                self.inner.descriptor.invocation.component.clone(),
                Some(self.inner.descriptor.invocation.version),
            ),
            Arc::clone(&self.inner) as Arc<dyn ContextProvider>,
        )
    }
}
