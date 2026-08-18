//! Child-run admission policy accepted by Python agent factories.

use finstack_ai::ChildRunPolicy;
use pyo3::prelude::*;

/// Child-run admission policy. Construction defaults to [`ChildRunPolicy::Deny`].
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "ChildRunPolicy",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct PyChildRunPolicy {
    inner: ChildRunPolicy,
}

#[pymethods]
impl PyChildRunPolicy {
    /// Reject every child invocation.
    #[staticmethod]
    fn deny() -> Self {
        Self {
            inner: ChildRunPolicy::Deny,
        }
    }

    /// Allow children up to the inclusive `max_depth`.
    #[staticmethod]
    fn allow(max_depth: u16) -> Self {
        Self {
            inner: ChildRunPolicy::Allow { max_depth },
        }
    }
}

impl PyChildRunPolicy {
    pub(crate) fn to_rust(self) -> ChildRunPolicy {
        self.inner
    }
}
