//! Tool-batch decisions and deterministic normalization helpers.

mod cancellation;
mod decide_open;
mod decide_settle;
mod followups;
mod planning;
mod records;

pub(super) use cancellation::{
    buffer_cancelled_effect, buffer_reconciled_tool_closures, cancellation_followups,
};
pub(super) use decide_open::decide_batch_prepared;
pub(super) use decide_settle::{decide_external_tool, decide_tool_settled, is_known_tool_effect};
pub(super) use records::{decode_tool_result, synthetic_result};
