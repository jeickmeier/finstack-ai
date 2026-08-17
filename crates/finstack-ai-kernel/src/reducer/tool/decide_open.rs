//! Tool-batch opening decisions.

use super::super::allocated_ids::validate_allocated_ids;
use super::super::capacity::{self, StateGrowth};
use super::super::decide::{draft_for_state, next_sequence, required};
use super::super::decision::{Decision, KernelError};
use super::super::fingerprint::{synthetic_tool_digest, tool_batch_plan_digest};
use super::super::input::StageSettled;
use crate::entries::{StageDisposition, StageOutcomeRecorded};
use crate::records::RecordBody;
use crate::state::{KernelState, RunPhase, TransitionEnv};
use crate::tools::{ToolBatchOpened, ToolCallPlan};

use super::planning::{
    assign_plans, ensure_record_batch_bound, opening_id_requirements, validate_plans,
    validate_record_batch_bounds,
};
use super::records::synthetic_result;
use super::records::{
    BufferedToolResult, append_group_requests, assistant_calls, close_record,
    first_executable_group, outcome_for_continuation, requirements_for_bodies, tool_settled_record,
};

#[expect(
    clippy::too_many_lines,
    reason = "batch opening keeps the frozen validation, record order, and ID preflight in one path"
)]
pub fn decide_batch_prepared(
    state: &KernelState,
    env: &TransitionEnv,
    input: &StageSettled,
    settlement_digest: crate::Digest,
) -> Result<Decision, KernelError> {
    let super::super::input::ReducerStageOutcome::ToolBatchPrepared {
        calls,
        continuation,
    } = &input.outcome
    else {
        return Err(KernelError::InvariantViolation);
    };
    if state.phase != Some(RunPhase::BeforeToolBatch) {
        return Err(KernelError::InvalidPhaseInput {
            phase: state.phase,
            input: "stage_settled",
        });
    }
    let turn = state
        .current_turn
        .as_ref()
        .ok_or(KernelError::InvariantViolation)?;
    let source_message = state
        .messages
        .last()
        .ok_or(KernelError::InvariantViolation)?;
    let source_calls = assistant_calls(source_message);
    validate_plans(state, &source_calls, calls)?;
    validate_record_batch_bounds(calls)?;
    validate_allocated_ids(&env.ids, opening_id_requirements(calls)?)?;

    let tool_batch_id = required(env.ids.tool_batch_ids(), 0, "tool_batch_ids")?;
    let assigned = assign_plans(env, calls)?;
    let plan_digest = tool_batch_plan_digest(
        state.cycle,
        turn.turn_id,
        tool_batch_id,
        *source_message.id(),
        &assigned,
        *continuation,
    )?;
    let opened = ToolBatchOpened {
        cycle: state.cycle,
        turn_id: turn.turn_id,
        tool_batch_id,
        source_message_id: *source_message.id(),
        calls: assigned.clone().into(),
        continuation: *continuation,
        plan_digest,
    };

    let first_group = first_executable_group(&assigned);
    let mut bodies = vec![
        RecordBody::StageOutcomeRecorded(StageOutcomeRecorded {
            cursor: input.cursor,
            disposition: StageDisposition::ToolBatchPrepared {
                tool_batch_id,
                plan_digest,
            },
            settlement_digest,
        }),
        RecordBody::ToolBatchOpened(opened.clone()),
    ];
    let mut actions = Vec::new();
    if let Some(group) = first_group {
        append_group_requests(&assigned, group, &mut bodies, &mut actions)?;
    }

    let mut result_ids = Vec::new();
    let mut synthetic_effects = Vec::new();
    for assigned_call in &assigned {
        let ToolCallPlan::SyntheticClosure(closure) = &assigned_call.plan else {
            break;
        };
        let result = synthetic_result(&closure.call, &closure.error)?;
        let digest = synthetic_tool_digest(
            tool_batch_id,
            *closure.call.tool_call_id(),
            assigned_call.effect_id,
            &result,
            &closure.error,
        )?;
        let message_id = required(env.ids.message_ids(), result_ids.len(), "message_ids")?;
        bodies.push(RecordBody::ToolCallSettled(tool_settled_record(
            &opened,
            assigned_call,
            message_id,
            env,
            BufferedToolResult {
                result,
                settlement_digest: digest,
                synthetic: true,
                error: Some(closure.error.clone()),
            },
        )?));
        result_ids.push(message_id);
        synthetic_effects.push(assigned_call.effect_id);
    }

    if result_ids.len() == assigned.len() {
        bodies.push(RecordBody::ToolBatchClosed(close_record(
            &opened,
            result_ids.clone(),
            outcome_for_continuation(*continuation),
        )?));
    }
    ensure_record_batch_bound(bodies.len())?;

    capacity::preflight_decision(
        state,
        StateGrowth {
            messages: result_ids.len(),
            stage: Some(input.cursor),
            tool_settlements: &synthetic_effects,
            ..StateGrowth::default()
        },
    )?;
    let requirements = requirements_for_bodies(&bodies, result_ids.len(), assigned.len(), 1)?;
    validate_allocated_ids(&env.ids, requirements)?;
    let records = draft_for_state(state, env, bodies)?;
    Ok(Decision {
        expected_sequence: next_sequence(state)?,
        records,
        actions,
        diagnostics: Vec::new(),
    })
}
