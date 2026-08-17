//! In-process reference workflow driver.
//!
//! This crate drives [`WorkflowSession`]. It does not depend on the kernel
//! crate. Semantic types come from `finstack-ai-runtime`. It does not
//! depend on the Temporal-shaped sibling.
//!
//! Cron schedule rows are adapter state persisted beside the journal. They
//! are not kernel records and do not add a `RecordBody` variant.

#![warn(missing_docs)]

mod cron;
mod driver;
mod store;

pub use cron::{CronError, CronExpression, CronFire, CronSchedule};
pub use driver::LocalWorkflowDriver;
pub use finstack_ai_runtime::{
    WorkflowCheckpoint, WorkflowDriverError, WorkflowRetryDecision, WorkflowSession, WorkflowWait,
    classify_wait, resolve_checkpoint_sequence, retry_decision,
};
pub use store::{CronScheduleStore, MemoryCronStore, SqliteCronStore};
