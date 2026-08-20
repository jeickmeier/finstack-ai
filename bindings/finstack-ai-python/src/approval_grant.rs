//! Paid-tool approval grant mode accepted by Python agent factories.

use finstack_ai::ApprovalGrantMode;
use pyo3::prelude::*;

/// How a run parks and releases paid-tool approvals.
///
/// Maps onto Rust [`finstack_ai::RunPolicy::approval_grant`]. Construction
/// defaults to [`ApprovalGrantMode::PerCall`]. Neither mode relaxes the
/// `Policy` catalog floor.
#[pyclass(
    module = "finstack_ai._finstack_ai",
    name = "ApprovalGrantMode",
    frozen,
    skip_from_py_object
)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct PyApprovalGrantMode {
    inner: ApprovalGrantMode,
}

#[pymethods]
impl PyApprovalGrantMode {
    /// Park once per unpaid paid tool call.
    #[staticmethod]
    fn per_call() -> Self {
        Self {
            inner: ApprovalGrantMode::PerCall,
        }
    }

    /// Park once listing every unpaid paid tool call.
    #[staticmethod]
    fn informed_batch() -> Self {
        Self {
            inner: ApprovalGrantMode::InformedBatch,
        }
    }
}

impl PyApprovalGrantMode {
    pub(crate) fn to_rust(self) -> ApprovalGrantMode {
        self.inner
    }
}
