//! Target-neutral Toolset port, offline schema resolution, and stream normalization.

mod catalog;
mod error;
mod port;
mod resume;
mod stream;
mod types;
mod validator;

#[cfg(test)]
mod tests;

pub use catalog::{
    ResolvedTool, ResolvedToolCatalog, ToolCatalogPlan, ToolExecutionPolicy, ToolPolicyDecision,
    ToolsetRegistration,
};
pub use error::{
    MCP_SAMPLING_REQUIRED, MCP_SAMPLING_UNAVAILABLE, MCP_SAMPLING_UNSUPPORTED,
    TOOL_APPROVAL_REQUIRED, TOOL_ARGUMENTS_INVALID, TOOL_CANCELLED, TOOL_DEADLINE_EXCEEDED,
    TOOL_INTERACTION_REQUIRED, TOOL_OUTPUT_INVALID, TOOL_PANICKED, TOOL_POLICY_DENIED,
    TOOL_RECONCILIATION_UNSUPPORTED, TOOL_REGISTRATION_INVALID, TOOL_RESULT_LIMIT_EXCEEDED,
    TOOL_STREAM_INVALID, TOOL_STREAM_LIMIT_EXCEEDED, ToolError, UNKNOWN_TOOL,
};
pub use port::Toolset;
pub use resume::{map_tool_reconcile_result, tool_resume_action, tool_retry_allowed};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use stream::AssembledToolTerminal;
pub use stream::{
    AssembledToolStream, ToolStreamAssembler, ToolStreamLimits, normalize_tool_result,
};
pub use types::{
    NestedSample, PendingToolEffect, ToolCallContext, ToolDeferral, ToolEventStream,
    ToolReconcileResult, ToolResult, ToolResumeAction, ToolStreamItem, ToolsetDescriptor,
};
pub use validator::{JsonSchemaToolValidatorCompiler, ToolValidator, ToolValidatorCompiler};
