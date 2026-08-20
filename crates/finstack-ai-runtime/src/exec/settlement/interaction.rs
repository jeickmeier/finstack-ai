use std::collections::BTreeMap;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, ComponentId, ComponentRef, ContentBlock, EffectTag, EventTag,
    InteractionExpired, InteractionId, InteractionKind, InteractionRequest, InteractionSettled,
    InteractionTag, InteractionTerminalOutcome, KernelInput, METADATA_MAX_BYTES, MessageTag,
    Metadata, RawJson, RecordBody, RecordTag, RequestInteraction, Stage, StageCursor,
    TEXT_MAX_BYTES, TextBlock, ToolBatchTag, ToolCallId, ToolCallPlan, TransitionEnv, Version,
};

use crate::coordinator::CommitCoordinator;
use crate::run_types::RunHandleError;
use crate::{Clock, InteractionResumeAction, LoadRequest, RandomSource, interaction_resume_action};

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

pub(super) fn approval_cursor(state: &finstack_ai_kernel::KernelState) -> StageCursor {
    StageCursor {
        cycle: state.cycle,
        stage: Stage::BeforeToolBatch,
    }
}

/// One unpaid paid tool included in an approval park.
pub(super) struct ApprovalSubject {
    pub(super) tool_call_id: ToolCallId,
    pub(super) tool_name: String,
    pub(super) arguments: RawJson,
}

const ARGS_DISPLAY_MAX: usize = 8 * 1024;

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
    subjects: &[ApprovalSubject],
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
            TextBlock::try_new(approval_prompt(subjects)?).map_err(|_| {
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
        approval_metadata(subjects)?,
    )
    .map_err(|_| RunHandleError::InteractionSettlement {
        code: "approval_request_invalid",
    })?;
    sources.record_parked_approval(
        subjects
            .iter()
            .map(|subject| subject.tool_call_id)
            .collect(),
    );
    submit_interaction_request(coordinator, sources, request).await
}

fn approval_prompt(subjects: &[ApprovalSubject]) -> Result<String, RunHandleError> {
    if subjects.is_empty() {
        return Err(RunHandleError::InteractionSettlement {
            code: "approval_subjects_missing",
        });
    }
    let header = if subjects.len() == 1 {
        "approve the next tool action:"
    } else {
        "approve the next tool actions:"
    };
    let reserved = header.len()
        + subjects
            .iter()
            .map(|subject| subject.tool_name.len() + 8)
            .sum::<usize>();
    let args_budget = TEXT_MAX_BYTES.saturating_sub(reserved).max(subjects.len());
    let per_args = (args_budget / subjects.len()).min(ARGS_DISPLAY_MAX);
    let mut prompt = String::from(header);
    for subject in subjects {
        prompt.push('\n');
        prompt.push_str(&subject.tool_name);
        prompt.push(' ');
        prompt.push_str(&truncated_args(subject.arguments.as_str(), per_args));
    }
    if prompt.len() > TEXT_MAX_BYTES {
        return Err(RunHandleError::InteractionSettlement {
            code: "approval_prompt_invalid",
        });
    }
    Ok(prompt)
}

fn truncated_args(args: &str, max: usize) -> String {
    if args.len() <= max {
        return args.to_owned();
    }
    let keep = max.saturating_sub(3);
    let mut end = keep;
    while end > 0 && !args.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &args[..end])
}

fn approval_metadata(subjects: &[ApprovalSubject]) -> Result<Metadata, RunHandleError> {
    let ids = subjects
        .iter()
        .map(|subject| subject.tool_call_id.to_string())
        .collect::<Vec<_>>();
    let value = serde_json::json!({ "tool_call_ids": ids });
    let bytes = serde_json_canonicalizer::to_vec(&value).map_err(|_| {
        RunHandleError::InteractionSettlement {
            code: "approval_metadata_invalid",
        }
    })?;
    if bytes.len() > METADATA_MAX_BYTES {
        return Err(RunHandleError::InteractionSettlement {
            code: "approval_metadata_invalid",
        });
    }
    Metadata::parse(bytes).map_err(|_| RunHandleError::InteractionSettlement {
        code: "approval_metadata_invalid",
    })
}

/// Replay committed approval parks after a process-local ledger is lost.
///
/// Reads `tool_call_ids` from each `InteractionRequested` Approval metadata
/// and pairs it with the later resolved, expired, or cancelled record. A
/// load or parse failure returns no pairs so those tools stay unpaid and
/// re-prompt — never an unapproved execute.
pub(super) async fn journaled_approval_outcomes(
    coordinator: &CommitCoordinator,
) -> Vec<(ToolCallId, InteractionTerminalOutcome)> {
    let Some(session_id) = coordinator.state().session_id else {
        return Vec::new();
    };
    let Ok(loaded) = coordinator
        .journal_store()
        .load(LoadRequest { session_id })
        .await
    else {
        return Vec::new();
    };
    let mut pending = BTreeMap::<InteractionId, Vec<ToolCallId>>::new();
    let mut outcomes = Vec::new();
    for batch in loaded.committed_batches.iter() {
        for record in batch.records.iter() {
            match record.body() {
                RecordBody::InteractionRequested(request)
                    if matches!(request.kind(), InteractionKind::Approval) =>
                {
                    if let Some(ids) = tool_call_ids_from_metadata(request.metadata()) {
                        pending.insert(request.interaction_id(), ids);
                    }
                }
                RecordBody::InteractionResolved(resolution) => {
                    if let Some(ids) = pending.remove(&resolution.interaction_id())
                        && let Some(outcome) = approval_response_outcome(resolution.response())
                    {
                        outcomes.extend(ids.into_iter().map(|id| (id, outcome)));
                    }
                }
                RecordBody::InteractionExpired(expired) => {
                    if let Some(ids) = pending.remove(&expired.interaction_id) {
                        outcomes.extend(
                            ids.into_iter()
                                .map(|id| (id, InteractionTerminalOutcome::Expired)),
                        );
                    }
                }
                RecordBody::InteractionCancelled(cancelled) => {
                    if let Some(ids) = pending.remove(&cancelled.interaction_id()) {
                        outcomes.extend(
                            ids.into_iter()
                                .map(|id| (id, InteractionTerminalOutcome::Cancelled)),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    outcomes
}

fn tool_call_ids_from_metadata(metadata: &Metadata) -> Option<Vec<ToolCallId>> {
    let value: serde_json::Value = serde_json::from_str(metadata.as_raw_json().as_str()).ok()?;
    let ids = value.get("tool_call_ids")?.as_array()?;
    let mut parsed = Vec::with_capacity(ids.len());
    for id in ids {
        parsed.push(ToolCallId::parse(id.as_str()?).ok()?);
    }
    Some(parsed)
}

fn approval_response_outcome(response: &RawJson) -> Option<InteractionTerminalOutcome> {
    let value: serde_json::Value = serde_json::from_str(response.as_str()).ok()?;
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    match object.get("approved").and_then(serde_json::Value::as_bool) {
        Some(true) => Some(InteractionTerminalOutcome::Granted),
        Some(false) => Some(InteractionTerminalOutcome::Denied),
        None => None,
    }
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
