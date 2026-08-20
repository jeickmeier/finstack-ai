//! Target-neutral Toolset port, offline schema resolution, and stream normalization.

mod authority;
mod catalog;
mod error;
mod port;
mod resume;
mod stream;
mod types;
mod validator;

#[cfg(test)]
mod tests;

pub use authority::verify_authority;
pub use catalog::{
    ApprovalState, ResolvedTool, ResolvedToolCatalog, ToolCatalogPlan, ToolExecutionPolicy,
    ToolPolicyDecision, ToolsetRegistration,
};
pub use error::{
    MCP_SAMPLING_REQUIRED, MCP_SAMPLING_UNAVAILABLE, TOOL_CANCELLED, TOOL_DEADLINE_EXCEEDED,
    TOOL_DEFERRAL_EXPIRED, TOOL_INTERACTION_REQUIRED, TOOL_OUTPUT_INVALID,
    TOOL_RECONCILIATION_UNSUPPORTED, ToolError,
};
#[cfg(test)]
pub(crate) use error::{
    TOOL_DEFERRAL_INVALID, TOOL_DEFERRAL_NOT_DECLARED, TOOL_POLICY_DENIED,
    TOOL_REGISTRATION_INVALID,
};
#[cfg(any(test, feature = "native-tokio"))]
pub(crate) use error::{TOOL_PANICKED, TOOL_STREAM_INVALID};
pub use port::Toolset;
pub use resume::{map_tool_reconcile_result, tool_resume_action, tool_retry_allowed};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use stream::AssembledToolTerminal;
pub use stream::{
    AssembledToolStream, ToolStreamAssembler, ToolStreamLimits, ToolTerminal, normalize_tool_result,
};
pub use types::{
    NestedSample, PendingToolEffect, ToolCallContext, ToolDeferral, ToolEventStream,
    ToolReconcileResult, ToolResult, ToolResumeAction, ToolStreamItem, ToolsetDescriptor,
};
pub use validator::{JsonSchemaToolValidatorCompiler, ToolValidator, ToolValidatorCompiler};
