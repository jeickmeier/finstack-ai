//! Recovery checks for the host proposal between tool settlement and activation.

use std::collections::BTreeSet;

use finstack_ai_kernel::{RecordBody, RecordEnvelope, RunId, ToolCallPlan, ToolResultBlock};

use super::super::activation::NativeCapabilityHost;
use super::DurableHostError;

/// Tool output is evidence of completion, never authority to restore a mask.
/// A committed activation clears the proposal; an otherwise lost proposal
/// requires reconciliation instead of silently continuing with an older mask.
pub(in crate::agent) fn validate_activation_prefix<'a>(
    host: Option<&NativeCapabilityHost>,
    run_id: RunId,
    records: impl Iterator<Item = &'a RecordEnvelope>,
) -> Result<(), DurableHostError> {
    let Some(host) = host else { return Ok(()) };
    let mut activations = BTreeSet::new();
    let mut unsettled_proposal = false;
    for record in records.filter(|record| record.run_id() == Some(run_id)) {
        match record.body() {
            RecordBody::ToolBatchOpened(opened) => {
                for call in opened.calls.iter() {
                    if matches!(&call.plan, ToolCallPlan::Execute(plan)
                        if plan.tool_id.as_str() == "finstack.tools.skills.activate")
                    {
                        activations.insert(call.effect_id);
                    }
                }
            }
            RecordBody::EffectCompleted(completed)
                if activations.remove(&completed.effect_id()) =>
            {
                let result: ToolResultBlock = serde_json::from_slice(completed.output().as_bytes())
                    .map_err(|_| DurableHostError::new("durable_capability_uncertain"))?;
                unsettled_proposal |= !result.is_error();
            }
            RecordBody::CapabilitiesActivated(_) => unsettled_proposal = false,
            _ => {}
        }
    }
    if unsettled_proposal && !host.has_pending(run_id) {
        return Err(DurableHostError::new("durable_capability_uncertain"));
    }
    Ok(())
}
