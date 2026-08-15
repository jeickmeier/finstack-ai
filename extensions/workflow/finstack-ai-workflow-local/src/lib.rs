//! In-process reference workflow driver.
//!
//! This crate does not depend on the kernel crate. Semantic types come from
//! `finstack-ai-runtime`. It does not depend on the Temporal-shaped sibling.

#![warn(missing_docs)]

pub use finstack_ai_runtime::{
    WorkflowCheckpoint, WorkflowDriverError, WorkflowRetryDecision, WorkflowSession, WorkflowWait,
    classify_wait, resolve_checkpoint_sequence, retry_decision,
};

/// Reference name for the in-process driver.
pub type LocalWorkflowDriver = WorkflowSession;
