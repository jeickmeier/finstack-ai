//! Agent output policy, budgets, capabilities, limits, and validation.

mod agent;
mod budget;
mod capabilities;
mod limits;
mod validation;

pub use agent::{
    FinalResultRecorded, INTERNAL_TOOL_NAMESPACE, JsonSchemaDraft, LOAD_CAPABILITY_TOOL,
    OutputConfiguration, OutputEndStrategy, OutputSpec, SUBMIT_FINAL_OUTPUT_TOOL, SchemaRef,
    StructuredResultSource, is_internal_tool_name,
};
pub use budget::{
    BudgetChargeReceipt, BudgetChargeRecorded, BudgetChargeRequest, BudgetRecordError,
    BudgetReleaseReceipt, BudgetReleaseRequest, BudgetRequest, BudgetReservationReceipt,
    BudgetReservationReleased, BudgetReservationRequested, BudgetReservationSettled,
    BudgetReserveRequest,
};
pub use capabilities::{ActiveCapability, CapabilitiesActivated, CapabilityActivationSource};
pub use limits::{
    CostLimit, LimitDimension, LimitReached, LimitUsage, LimitValue, LimitsError, RunLimits,
    UnknownUsagePolicy,
};
pub(crate) use validation::expected_validation_error;
pub use validation::{OutputValidated, OutputValidationFailed, ValidationIssue, ValidationOutcome};
