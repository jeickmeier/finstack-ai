use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, ComponentId, ComponentRef, ContentBlock, EffectTag, EventTag,
    InteractionExpired, InteractionKind, InteractionRequest, InteractionSettled, InteractionTag,
    InteractionTerminalOutcome, KernelInput, MessageTag, Metadata, RawJson, RecordTag,
    RequestInteraction, Stage, StageCursor, TextBlock, ToolBatchTag, ToolCallPlan, TransitionEnv,
    Version,
};

use crate::coordinator::CommitCoordinator;
use crate::run_types::RunHandleError;
use crate::{Clock, InteractionResumeAction, RandomSource, interaction_resume_action};

use super::SettlementSources;
use super::tool::{generate_tool_id, generate_tool_ids};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToolOpeningCounts {
    records: usize,
    events: usize,
    effects: usize,
    messages: usize,
}

fn tool_opening_counts(plans: &[ToolCallPlan]) -> Result<ToolOpeningCounts, RunHandleError> {
    let mut groups = Vec::with_capacity(plans.len());
    let mut group = 0_u32;
    for (index, plan) in plans.iter().enumerate() {
        if index > 0
            && !(plans[index - 1].execution() == finstack_ai_kernel::ToolExecutionMode::Parallel
                && plan.execution() == finstack_ai_kernel::ToolExecutionMode::Parallel)
        {
            group = group.checked_add(1).ok_or(RunHandleError::ToolSettlement {
                code: "tool_group_count_overflow",
            })?;
        }
        groups.push(group);
    }
    let first_executable_group = plans
        .iter()
        .zip(&groups)
        .find_map(|(plan, group)| matches!(plan, ToolCallPlan::Execute(_)).then_some(*group));
    let requests = first_executable_group.map_or(0, |first| {
        plans
            .iter()
            .zip(&groups)
            .filter(|(plan, group)| **group == first && matches!(plan, ToolCallPlan::Execute(_)))
            .count()
    });
    let messages = plans
        .iter()
        .take_while(|plan| matches!(plan, ToolCallPlan::SyntheticClosure(_)))
        .count();
    Ok(ToolOpeningCounts {
        records: 2 + requests + messages + usize::from(first_executable_group.is_none()),
        events: requests + 2 * messages,
        effects: plans.len(),
        messages,
    })
}

fn approval_cursor(state: &finstack_ai_kernel::KernelState) -> StageCursor {
    StageCursor {
        cycle: state.cycle,
        stage: Stage::BeforeToolBatch,
    }
}

pub(super) fn approval_released_for_current_cursor(
    state: &finstack_ai_kernel::KernelState,
) -> bool {
    state
        .last_interaction_terminal
        .as_ref()
        .is_some_and(|terminal| {
            terminal.kind == InteractionKind::Approval
                && terminal.outcome == InteractionTerminalOutcome::Granted
                && terminal.cursor == approval_cursor(state)
        })
}

pub(super) fn approval_refused_for_current_cursor(state: &finstack_ai_kernel::KernelState) -> bool {
    state
        .last_interaction_terminal
        .as_ref()
        .is_some_and(|terminal| {
            terminal.kind == InteractionKind::Approval
                && matches!(
                    terminal.outcome,
                    InteractionTerminalOutcome::Denied
                        | InteractionTerminalOutcome::Expired
                        | InteractionTerminalOutcome::Cancelled
                )
                && terminal.cursor == approval_cursor(state)
        })
}

fn approval_schema() -> Result<RawJson, RunHandleError> {
    RawJson::parse(
        r#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "approval_schema_invalid",
    })
}

pub(super) async fn request_approval_interaction<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
) -> Result<(), RunHandleError> {
    let interaction_id = generate_tool_id::<InteractionTag, _, _>(sources)?;
    let effect_id = generate_tool_id::<EffectTag, _, _>(sources)?;
    let expires_at = coordinator
        .state()
        .accepted
        .as_ref()
        .and_then(finstack_ai_kernel::RunAccepted::effective_deadline);
    let request = InteractionRequest::try_new(
        1,
        interaction_id,
        effect_id,
        InteractionKind::Approval,
        vec![ContentBlock::Text(
            TextBlock::try_new("approve the next tool action").map_err(|_| {
                RunHandleError::InteractionSettlement {
                    code: "approval_prompt_invalid",
                }
            })?,
        )],
        approval_schema()?,
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").map_err(|_| {
                RunHandleError::InteractionSettlement {
                    code: "approval_policy_invalid",
                }
            })?,
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        expires_at,
        false,
        Metadata::empty(),
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "approval_request_invalid",
    })?;
    submit_interaction_request(coordinator, sources, request).await
}

pub(crate) async fn request_tool_interaction<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    template: &InteractionRequest,
) -> Result<(), RunHandleError> {
    let interaction_id = generate_tool_id::<InteractionTag, _, _>(sources)?;
    let effect_id = generate_tool_id::<EffectTag, _, _>(sources)?;
    let expires_at = template.expires_at().or_else(|| {
        coordinator
            .state()
            .accepted
            .as_ref()
            .and_then(finstack_ai_kernel::RunAccepted::effective_deadline)
    });
    let request = InteractionRequest::try_new(
        template.request_version(),
        interaction_id,
        effect_id,
        template.kind().clone(),
        template.prompt().to_vec(),
        template.response_schema().clone(),
        template.policy_component().clone(),
        template.policy_version(),
        template.assignee_hint().cloned(),
        expires_at,
        template.delegatable(),
        template.metadata().clone(),
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "tool_interaction_request_invalid",
    })?;
    submit_interaction_request(coordinator, sources, request).await
}

async fn submit_interaction_request<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    request: InteractionRequest,
) -> Result<(), RunHandleError> {
    let now = sources.now()?;
    let interaction_id = request.interaction_id();
    let effect_id = request.effect_id();
    let input = KernelInput::RequestInteraction(RequestInteraction { request });
    let ids = AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(2, sources)?,
        generate_tool_ids::<EventTag, _, _>(2, sources)?,
        vec![effect_id],
        vec![interaction_id],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "interaction_request_ids_invalid",
    })?;
    let env = TransitionEnv { now, ids };
    coordinator.classify(&env, input.clone()).map_err(|_| {
        RunHandleError::InteractionSettlement {
            code: "interaction_request_allocation_mismatch",
        }
    })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(())
}

pub(crate) async fn apply_interaction_resume<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
) -> Result<InteractionResumeAction, RunHandleError> {
    if coordinator.state().cancellation.is_some() {
        return Ok(InteractionResumeAction::WaitResolution);
    }
    let now = sources.now()?;
    let action = interaction_resume_action(coordinator.state(), now);
    if action != InteractionResumeAction::ExpireIfDue {
        return Ok(action);
    }
    let pending = coordinator.state().pending_interaction.as_ref().ok_or(
        RunHandleError::InteractionSettlement {
            code: "interaction_pending_missing",
        },
    )?;
    let input = KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
        interaction_id: pending.request.interaction_id(),
        expired_at: now,
    }));
    let ids = AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(2, sources)?,
        generate_tool_ids::<EventTag, _, _>(2, sources)?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "interaction_expire_ids_invalid",
    })?;
    let env = TransitionEnv { now, ids };
    coordinator.classify(&env, input.clone()).map_err(|_| {
        RunHandleError::InteractionSettlement {
            code: "interaction_expire_allocation_mismatch",
        }
    })?;
    let outcome = coordinator
        .submit(env, input)
        .await
        .map_err(RunHandleError::Coordinator)?;
    if let Some(fault) = outcome.fault {
        return Err(RunHandleError::Faulted { code: fault.code });
    }
    Ok(action)
}

pub(super) fn allocate_tool_opening<C: Clock, R: RandomSource>(
    plans: &[ToolCallPlan],
    sources: &SettlementSources<C, R>,
) -> Result<AllocatedIds, RunHandleError> {
    let counts = tool_opening_counts(plans)?;
    AllocatedIds::try_new(
        generate_tool_ids::<RecordTag, _, _>(counts.records, sources)?,
        generate_tool_ids::<EventTag, _, _>(counts.events, sources)?,
        generate_tool_ids::<EffectTag, _, _>(counts.effects, sources)?,
        Vec::new(),
        generate_tool_ids::<MessageTag, _, _>(counts.messages, sources)?,
        Vec::new(),
        Vec::new(),
        vec![generate_tool_id::<ToolBatchTag, _, _>(sources)?],
        Vec::new(),
        vec![generate_tool_id::<AppendBatchTag, _, _>(sources)?],
        Vec::new(),
    )
    .map_err(|_| RunHandleError::ToolSettlement {
        code: "tool_opening_ids_invalid",
    })
}
