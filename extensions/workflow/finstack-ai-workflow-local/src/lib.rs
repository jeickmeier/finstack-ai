//! In-process reference workflow driver.
//!
//! This crate drives [`WorkflowSession`]. It does not depend on the kernel
//! crate. Semantic types come from `finstack-ai-runtime`. It does not
//! depend on the Temporal-shaped sibling.
//!
//! Cron schedule rows are adapter state persisted beside the journal. They
//! are not kernel records and do not add a `RecordBody` variant.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

mod cron;
mod driver;
mod store;

pub use cron::{CronError, CronFire, CronSchedule, IntervalSchedule};
pub use driver::LocalWorkflowDriver;
pub use finstack_ai_runtime::{
    WorkflowCheckpoint, WorkflowDriverError, WorkflowRetryDecision, WorkflowSession, WorkflowWait,
    classify_wait, resolve_checkpoint_sequence, retry_decision,
};
pub use store::{CronScheduleStore, MemoryCronStore, SqliteCronStore};
