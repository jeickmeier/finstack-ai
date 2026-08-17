//! Tool-call planning, durable batch records, and replay state.
//!
//! [`ToolCallPlan`] and [`ValidatedToolCall`] describe source-ordered calls.
//! [`ToolBatchOpened`], [`ToolCallSettled`], and [`ToolBatchClosed`] are
//! durable records. [`ActiveToolBatch`] and [`ToolCallIdentity`] are the
//! replay-derived in-flight state used by the reducer.

mod types;

pub use types::{
    ActiveToolBatch, ActiveToolCall, ActiveToolCallStatus, AssignedToolCall, SyntheticToolClosure,
    ToolBatchClosed, ToolBatchContinuation, ToolBatchOpened, ToolBatchOutcome, ToolCallIdentity,
    ToolCallPlan, ToolCallSettled, ToolExecutionMode, ToolFailurePolicy, ToolSettlementFingerprint,
    ToolSettlementKind, ValidatedToolCall,
};
