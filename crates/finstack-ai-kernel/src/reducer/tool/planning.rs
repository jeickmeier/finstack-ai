//! Tool-plan validation and assignment.

use super::super::allocated_ids::IdRequirements;
use super::super::decide::required;
use super::super::decision::KernelError;
use super::super::validation::validate_error_descriptor;
use crate::content::ToolCallBlock;
use crate::effects::EffectOutputKind;
use crate::records::APPEND_BATCH_MAX_RECORDS;
use crate::state::{KernelState, TransitionEnv};
use crate::tools::{AssignedToolCall, ToolCallPlan, ToolFailurePolicy};

pub fn opening_id_requirements(plans: &[ToolCallPlan]) -> Result<IdRequirements, KernelError> {
    let groups = execution_groups(plans)?;
    let first_executable_group = plans
        .iter()
        .zip(&groups)
        .find_map(|(plan, group)| matches!(plan, ToolCallPlan::Execute(_)).then_some(*group));
    let requests = first_executable_group.map_or(0, |group| {
        plans
            .iter()
            .zip(&groups)
            .filter(|(plan, assigned_group)| {
                **assigned_group == group && matches!(plan, ToolCallPlan::Execute(_))
            })
            .count()
    });
    let synthetic = plans
        .iter()
        .take_while(|plan| matches!(plan, ToolCallPlan::SyntheticClosure(_)))
        .count();
    let records = 2 + requests + synthetic + usize::from(first_executable_group.is_none());
    let events = requests + 2 * synthetic;
    Ok(IdRequirements::new(records, events, plans.len(), 0, 0, synthetic).with_tools(1, 0))
}

pub fn validate_plans(
    state: &KernelState,
    source: &[&ToolCallBlock],
    plans: &[ToolCallPlan],
) -> Result<(), KernelError> {
    if source.is_empty() || source.len() != plans.len() {
        return Err(KernelError::ToolBatchPlanMismatch);
    }
    for (source_call, plan) in source.iter().zip(plans) {
        let planned = plan.call();
        if source_call.tool_call_id() != planned.tool_call_id()
            || source_call.tool_name() != planned.tool_name()
            || source_call.arguments() != planned.arguments()
        {
            return Err(KernelError::ToolBatchPlanMismatch);
        }
        let identity = state
            .tool_calls
            .get(source_call.tool_call_id())
            .ok_or(KernelError::ToolBatchPlanMismatch)?;
        if identity.call != **source_call || identity.effect_id.is_some() {
            return Err(KernelError::ToolBatchPlanMismatch);
        }
        match plan {
            ToolCallPlan::Execute(call)
                if call.output_contract.kind != EffectOutputKind::ToolResult =>
            {
                return Err(KernelError::ToolEffectContractMismatch);
            }
            ToolCallPlan::SyntheticClosure(closure) => {
                validate_error_descriptor(&closure.error)?;
            }
            ToolCallPlan::Execute(_) => {}
        }
    }
    Ok(())
}

pub fn assign_plans(
    env: &TransitionEnv,
    plans: &[ToolCallPlan],
) -> Result<Vec<AssignedToolCall>, KernelError> {
    let mut assigned = Vec::with_capacity(plans.len());
    let groups = execution_groups(plans)?;
    for (index, (plan, group)) in plans.iter().zip(groups).enumerate() {
        assigned.push(AssignedToolCall {
            source_index: u32::try_from(index).map_err(|_| KernelError::InvalidInputPayload {
                field: "calls",
                reason_code: "too_many_items",
            })?,
            group_index: group,
            effect_id: required(env.ids.effect_ids(), index, "effect_ids")?,
            plan: plan.clone(),
        });
    }
    Ok(assigned)
}

pub fn execution_groups(plans: &[ToolCallPlan]) -> Result<Vec<u32>, KernelError> {
    let mut groups = Vec::with_capacity(plans.len());
    let mut group = 0_u32;
    for (index, plan) in plans.iter().enumerate() {
        if index > 0
            && !(plans[index - 1].execution() == crate::ToolExecutionMode::Parallel
                && plan.execution() == crate::ToolExecutionMode::Parallel)
        {
            group = group
                .checked_add(1)
                .ok_or(KernelError::InvalidInputPayload {
                    field: "calls",
                    reason_code: "too_many_items",
                })?;
        }
        groups.push(group);
    }
    Ok(groups)
}

pub fn validate_record_batch_bounds(plans: &[ToolCallPlan]) -> Result<(), KernelError> {
    let groups = execution_groups(plans)?;
    let executable_groups = plans
        .iter()
        .zip(&groups)
        .filter(|(plan, _)| matches!(plan, ToolCallPlan::Execute(_)))
        .map(|(_, group)| *group)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let leading_synthetic = plans
        .iter()
        .take_while(|plan| matches!(plan, ToolCallPlan::SyntheticClosure(_)))
        .count();
    let opening_requests = executable_groups.first().map_or(0, |group| {
        plans
            .iter()
            .zip(&groups)
            .filter(|(plan, assigned_group)| {
                **assigned_group == *group && matches!(plan, ToolCallPlan::Execute(_))
            })
            .count()
    });
    let opening_close = usize::from(executable_groups.is_empty());
    ensure_record_batch_bound(2 + leading_synthetic + opening_requests + opening_close)?;

    for (group_position, group) in executable_groups.iter().enumerate() {
        let start = plans
            .iter()
            .zip(&groups)
            .position(|(plan, assigned_group)| {
                *assigned_group == *group && matches!(plan, ToolCallPlan::Execute(_))
            })
            .ok_or(KernelError::InvariantViolation)?;
        let next_group = executable_groups.get(group_position + 1).copied();
        let end = next_group
            .and_then(|next| {
                plans
                    .iter()
                    .zip(&groups)
                    .position(|(plan, assigned_group)| {
                        *assigned_group == next && matches!(plan, ToolCallPlan::Execute(_))
                    })
            })
            .unwrap_or(plans.len());
        let next_requests = next_group.map_or(0, |next| {
            plans
                .iter()
                .zip(&groups)
                .filter(|(plan, assigned_group)| {
                    **assigned_group == next && matches!(plan, ToolCallPlan::Execute(_))
                })
                .count()
        });
        let close = usize::from(next_group.is_none());
        ensure_record_batch_bound(1 + (end - start) + next_requests + close)?;

        let group_can_fail_run = plans.iter().zip(&groups).any(|(plan, assigned_group)| {
            *assigned_group == *group
                && matches!(plan, ToolCallPlan::Execute(_))
                && plan.failure_policy() == ToolFailurePolicy::FailRun
        });
        if group_can_fail_run {
            ensure_record_batch_bound(1 + (plans.len() - start) + 1)?;
        }
    }
    Ok(())
}

pub fn ensure_record_batch_bound(count: usize) -> Result<(), KernelError> {
    if count > APPEND_BATCH_MAX_RECORDS {
        return Err(KernelError::InvalidInputPayload {
            field: "records",
            reason_code: "too_many_items",
        });
    }
    Ok(())
}
