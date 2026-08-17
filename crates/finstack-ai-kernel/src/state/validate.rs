use std::collections::BTreeMap;

use crate::agent::OutputConfiguration;
use crate::bounds::{SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
use crate::content::{ContentBlock, LABEL_MAX_BYTES, ToolCallBlock};
use crate::effects::{EffectInput, EffectKind, EffectOutputKind};
use crate::ids::{MessageId, ToolCallId};
use crate::reducer::KernelError;
use crate::tools::{ActiveToolBatch, ActiveToolCallStatus, ToolCallPlan, ToolSettlementKind};

use super::{KernelState, RetryState, RunPhase, TerminalCandidate, TerminalState};

impl KernelState {
    /// Validate v1 collection ceilings for a programmatically assembled state.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidInputPayload`] when a semantic collection
    /// exceeds its frozen v1 ceiling.
    #[expect(
        clippy::too_many_lines,
        reason = "state validation keeps all cross-field replay invariants in one fail-closed boundary"
    )]
    pub fn validate(&self) -> Result<(), KernelError> {
        if self.messages.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(KernelError::InvalidInputPayload {
                field: "messages",
                reason_code: "too_many_items",
            });
        }
        for (field, length) in [
            ("stage_settlements", self.stage_settlements.len()),
            ("model_settlements", self.model_settlements.len()),
            ("completion_identities", self.completion_identities.len()),
            ("tool_calls", self.tool_calls.len()),
            ("tool_settlements", self.tool_settlements.len()),
            ("timer_firings", self.retry.timer_firings.len()),
            ("child_preparations", self.child_preparations.len()),
            ("budget_reservations", self.budget_reservations.len()),
            ("budget_charges", self.budget_charges.len()),
            ("resolution_identities", self.resolution_identities.len()),
        ] {
            if length > SEMANTIC_MAP_MAX_ENTRIES {
                return Err(KernelError::InvalidInputPayload {
                    field,
                    reason_code: "too_many_items",
                });
            }
        }
        if self.completion_identities.keys().any(|completion_id| {
            completion_id.is_empty()
                || completion_id.len() > LABEL_MAX_BYTES
                || completion_id.as_bytes().contains(&0)
        }) {
            return Err(KernelError::InvalidInputPayload {
                field: "completion_identities",
                reason_code: "invalid_label",
            });
        }
        if self.resolution_identities.keys().any(|resolution_id| {
            resolution_id.is_empty()
                || resolution_id.len() > LABEL_MAX_BYTES
                || resolution_id.as_bytes().contains(&0)
        }) {
            return Err(KernelError::InvalidInputPayload {
                field: "resolution_identities",
                reason_code: "invalid_label",
            });
        }
        let has_tool_state = self.active_tool_batch.is_some()
            || !self.tool_calls.is_empty()
            || !self.tool_settlements.is_empty()
            || self.last_tool_batch.is_some();
        let has_control_state = self.cancellation.is_some()
            || self.retry != RetryState::default()
            || self.last_limit.is_some()
            || self.suspension.is_some()
            || matches!(self.terminal, Some(TerminalState::Cancelled(_)));
        let has_structured_state = self.output_configuration.is_some()
            || !self.active_capabilities.is_empty()
            || self.resolved_plan_digest.is_some()
            || self.final_result.is_some()
            || self.validation_failure.is_some();
        let has_composition_state = !self.child_preparations.is_empty()
            || !self.budget_reservations.is_empty()
            || !self.budget_charges.is_empty();
        let has_interaction_state = self.pending_interaction.is_some()
            || !self.resolution_identities.is_empty()
            || self.last_interaction_terminal.is_some();
        if !matches!(self.state_version, 1..=6)
            || (self.state_version == 1 && has_tool_state)
            || (self.state_version < 3 && has_control_state)
            || (self.state_version < 4 && has_structured_state)
            || (self.state_version < 5 && has_composition_state)
            || (self.state_version < 6 && has_interaction_state)
        {
            return Err(KernelError::InvalidInputPayload {
                field: "state_version",
                reason_code: "unsupported_or_inconsistent",
            });
        }
        if (self.phase == Some(RunPhase::AwaitingInteraction) && self.pending_interaction.is_none())
            || (self.pending_interaction.is_some()
                && !matches!(
                    self.phase,
                    Some(
                        RunPhase::AwaitingInteraction | RunPhase::Cancelling | RunPhase::Suspended
                    )
                ))
        {
            return Err(KernelError::InvalidInputPayload {
                field: "pending_interaction",
                reason_code: "inconsistent",
            });
        }
        if self.state_version >= 2 && has_tool_state {
            self.validate_tool_state()?;
        }
        if self.state_version >= 3 {
            if self.accepted.is_some() != self.accepted_at.is_some() {
                return Err(KernelError::InvalidInputPayload {
                    field: "accepted_at",
                    reason_code: "inconsistent",
                });
            }
            if self.cancellation.is_some()
                != matches!(
                    self.phase,
                    Some(RunPhase::Cancelling | RunPhase::Suspended | RunPhase::Cancelled)
                )
                && self.cancellation.is_some()
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "cancellation",
                    reason_code: "inconsistent",
                });
            }
            if self.limit_usage.extension_counters.len() > crate::RunLimits::MAX_EXTENSION_COUNTERS
                || self.accepted.as_ref().is_some_and(|accepted| {
                    self.limit_usage
                        .extension_counters
                        .keys()
                        .any(|key| !accepted.limits().extension_counters.contains_key(key))
                })
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "limit_usage.extension_counters",
                    reason_code: "unregistered_or_too_many_entries",
                });
            }
            if self
                .last_limit
                .as_ref()
                .is_some_and(|limit| limit.validate().is_err())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "last_limit",
                    reason_code: "invalid_value_kind",
                });
            }
            if self.retry.pending.as_ref().is_some_and(|pending| {
                pending.attempt != self.retry.attempts
                    || !matches!(
                        self.phase,
                        Some(RunPhase::Sleeping | RunPhase::Cancelling | RunPhase::Suspended)
                    )
            }) || (self.phase == Some(RunPhase::Sleeping) && self.retry.pending.is_none())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "retry",
                    reason_code: "inconsistent",
                });
            }
            if let Some(cancellation) = &self.cancellation {
                for (field, values) in [
                    (
                        "cancellation.completed_effects",
                        cancellation.completed_effects.as_ref(),
                    ),
                    (
                        "cancellation.cancelled_effects",
                        cancellation.cancelled_effects.as_ref(),
                    ),
                    (
                        "cancellation.uncertain_effects",
                        cancellation.uncertain_effects.as_ref(),
                    ),
                    (
                        "cancellation.outstanding_effects",
                        cancellation.outstanding_effects.as_ref(),
                    ),
                ] {
                    if values.len() > SEMANTIC_ARRAY_MAX_ITEMS
                        || values.windows(2).any(|pair| pair[0] >= pair[1])
                    {
                        return Err(KernelError::InvalidInputPayload {
                            field,
                            reason_code: "invalid_effect_set",
                        });
                    }
                }
                let classified = cancellation
                    .completed_effects
                    .iter()
                    .chain(cancellation.cancelled_effects.iter())
                    .chain(cancellation.uncertain_effects.iter())
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>();
                let classified_len = cancellation.completed_effects.len()
                    + cancellation.cancelled_effects.len()
                    + cancellation.uncertain_effects.len();
                if classified.len() != classified_len
                    || cancellation
                        .outstanding_effects
                        .iter()
                        .any(|id| classified.contains(id))
                {
                    return Err(KernelError::InvalidInputPayload {
                        field: "cancellation",
                        reason_code: "overlapping_effect_sets",
                    });
                }
            }
            let terminal_phase_matches = matches!(
                (&self.terminal, self.phase),
                (Some(TerminalState::Completed(_)), Some(RunPhase::Completed))
                    | (Some(TerminalState::Failed(_)), Some(RunPhase::Failed))
                    | (Some(TerminalState::Cancelled(_)), Some(RunPhase::Cancelled))
                    | (None, _)
            );
            if !terminal_phase_matches {
                return Err(KernelError::InvalidInputPayload {
                    field: "terminal",
                    reason_code: "inconsistent_phase",
                });
            }
        }
        if self.state_version >= 5 {
            let accepted = self
                .accepted
                .as_ref()
                .ok_or(KernelError::InvalidInputPayload {
                    field: "composition_state",
                    reason_code: "missing_accepted_run",
                })?;
            let parent_session_id = self.session_id.ok_or(KernelError::InvalidInputPayload {
                field: "composition_state",
                reason_code: "missing_session_id",
            })?;
            let parent_lane_id = self.lane_id.ok_or(KernelError::InvalidInputPayload {
                field: "composition_state",
                reason_code: "missing_lane_id",
            })?;
            for (effect_id, prepared) in &self.child_preparations {
                let same_session = prepared.child.operation.session_id == parent_session_id;
                let placement_matches = match prepared.placement {
                    crate::ChildPlacement::CompatibleLaneInParentSession => {
                        same_session && prepared.child.operation.lane_id != parent_lane_id
                    }
                    crate::ChildPlacement::IsolatedChildSession
                    | crate::ChildPlacement::RemoteChildSession => !same_session,
                };
                if effect_id != &prepared.parent_effect_id
                    || prepared.parent_run_id != accepted.run_id()
                    || prepared
                        .validate(accepted.security().tenant_scope())
                        .is_err()
                    || !placement_matches
                {
                    return Err(KernelError::InvalidInputPayload {
                        field: "child_preparations",
                        reason_code: "inconsistent",
                    });
                }
            }
            for (reservation_id, replay) in &self.budget_reservations {
                if reservation_id != &replay.request.reservation_id
                    || replay.request.validate().is_err()
                    || replay.settlement.as_ref().is_some_and(|receipt| {
                        receipt.validate().is_err()
                            || receipt.scope_id != replay.request.scope_id
                            || receipt.reservation_id != replay.request.reservation_id
                            || receipt.request_digest != replay.request.request_digest
                            || receipt.reserved != replay.request.amount
                    })
                    || replay.release.as_ref().is_some_and(|receipt| {
                        receipt.validate().is_err()
                            || receipt.scope_id != replay.request.scope_id
                            || receipt.reservation_id != replay.request.reservation_id
                            || replay.settlement.is_none()
                            || self.terminal.is_none()
                    })
                {
                    return Err(KernelError::InvalidInputPayload {
                        field: "budget_reservations",
                        reason_code: "inconsistent",
                    });
                }
            }
            for (effect_id, receipt) in &self.budget_charges {
                if effect_id != &receipt.effect_id
                    || receipt.validate().is_err()
                    || !self
                        .budget_reservations
                        .get(&receipt.reservation_id)
                        .is_some_and(|reservation| {
                            reservation.settlement.is_some()
                                && reservation.request.scope_id == receipt.scope_id
                        })
                {
                    return Err(KernelError::InvalidInputPayload {
                        field: "budget_charges",
                        reason_code: "inconsistent",
                    });
                }
            }
        }
        if self.state_version == 4 {
            if self
                .output_configuration
                .as_ref()
                .is_some_and(|configuration| configuration.validate().is_err())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "output_configuration",
                    reason_code: "invalid",
                });
            }
            if self.active_capabilities.len() > SEMANTIC_ARRAY_MAX_ITEMS
                || self
                    .active_capabilities
                    .windows(2)
                    .any(|pair| pair[0].capability_id >= pair[1].capability_id)
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "active_capabilities",
                    reason_code: "invalid_capability_set",
                });
            }
            if self.final_result.is_some() && self.validation_failure.is_some() {
                return Err(KernelError::InvalidInputPayload {
                    field: "structured_output",
                    reason_code: "conflicting_outcomes",
                });
            }
            if self
                .final_result
                .as_ref()
                .is_some_and(|result| result.validate().is_err())
                || self
                    .validation_failure
                    .as_ref()
                    .is_some_and(|failure| failure.validate().is_err())
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "structured_output",
                    reason_code: "invalid",
                });
            }
            if let Some(result) = self.final_result.as_ref() {
                let configuration_matches = matches!(
                    self.output_configuration.as_ref(),
                    Some(OutputConfiguration {
                        output: crate::OutputSpec::JsonSchema { schema },
                        end_strategy,
                    }) if schema == &result.schema && end_strategy == &result.end_strategy
                );
                let candidate_matches = matches!(
                    self.terminal_candidate.as_ref(),
                    Some(TerminalCandidate::Completed {
                        cycle,
                        turn_id,
                        model_request_id,
                        effect_id,
                        message_id,
                        result_digest,
                    }) if cycle == &result.cycle
                        && turn_id == &result.turn_id
                        && model_request_id == &result.model_request_id
                        && effect_id == &result.effect_id
                        && message_id == &result.message_id
                        && result_digest == &result.value_digest
                );
                if !configuration_matches || !candidate_matches {
                    return Err(KernelError::InvalidInputPayload {
                        field: "final_result",
                        reason_code: "configuration_mismatch",
                    });
                }
            }
            if let Some(failure) = self.validation_failure.as_ref() {
                let schema_matches = matches!(
                    self.output_configuration.as_ref(),
                    Some(OutputConfiguration {
                        output: crate::OutputSpec::JsonSchema { schema },
                        ..
                    }) if schema == &failure.schema
                );
                let candidate_matches = matches!(
                    self.terminal_candidate.as_ref(),
                    Some(TerminalCandidate::Failed {
                        cycle,
                        turn_id: Some(turn_id),
                        model_request_id: Some(model_request_id),
                        effect_id: Some(effect_id),
                        error,
                    }) if cycle == &failure.cycle
                        && turn_id == &failure.turn_id
                        && model_request_id == &failure.model_request_id
                        && effect_id == &failure.effect_id
                        && error == &failure.error
                );
                if !schema_matches || !candidate_matches {
                    return Err(KernelError::InvalidInputPayload {
                        field: "validation_failure",
                        reason_code: "configuration_mismatch",
                    });
                }
            }
            if (self.final_result.is_some() || self.validation_failure.is_some())
                && !matches!(
                    self.output_configuration,
                    Some(OutputConfiguration {
                        output: crate::OutputSpec::JsonSchema { .. },
                        ..
                    })
                )
            {
                return Err(KernelError::InvalidInputPayload {
                    field: "structured_output",
                    reason_code: "missing_configuration",
                });
            }
        }
        Ok(())
    }

    pub(super) fn validate_tool_state(&self) -> Result<(), KernelError> {
        let invalid = || KernelError::InvalidInputPayload {
            field: "tool_state",
            reason_code: "inconsistent",
        };
        if self.tool_calls.is_empty()
            || (self.active_tool_batch.is_some() && self.last_tool_batch.is_some())
        {
            return Err(invalid());
        }

        // One pass over messages builds the authorship index, so each tool call
        // costs a lookup instead of a full message-and-block rescan. The nested
        // form was O(tool_calls x messages x blocks) and ran on every apply and
        // every deserialize, making it a decode-path denial-of-service surface.
        let mut authored: BTreeMap<&ToolCallId, Vec<(&MessageId, &ToolCallBlock)>> =
            BTreeMap::new();
        for message in self.messages.iter() {
            if message.role() != crate::MessageRole::Assistant {
                continue;
            }
            for block in message.content() {
                if let ContentBlock::ToolCall(call) = block {
                    authored
                        .entry(call.tool_call_id())
                        .or_default()
                        .push((message.id(), call));
                }
            }
        }

        let mut effect_ids = std::collections::BTreeSet::new();
        for (tool_call_id, identity) in &self.tool_calls {
            if identity.call.tool_call_id() != tool_call_id
                || identity.tool_batch_id.is_some() != identity.effect_id.is_some()
            {
                return Err(invalid());
            }
            let source_matches = authored.get(tool_call_id).is_some_and(|authorships| {
                authorships.iter().any(|(message_id, call)| {
                    **message_id == identity.source_message_id && *call == &identity.call
                })
            });
            if !source_matches {
                return Err(invalid());
            }
            if let Some(effect_id) = identity.effect_id
                && !effect_ids.insert(effect_id)
            {
                return Err(invalid());
            }
        }
        if self
            .tool_settlements
            .keys()
            .any(|effect_id| !effect_ids.contains(effect_id))
        {
            return Err(invalid());
        }

        if let Some(batch) = &self.active_tool_batch {
            self.validate_active_tool_batch(batch)
                .map_err(|()| invalid())?;
        } else if matches!(self.phase, Some(RunPhase::AwaitingTools))
            || (self.phase == Some(RunPhase::AwaitingExternal)
                && self.pending_model_effect.is_none())
        {
            return Err(invalid());
        }

        if let Some(closed) = &self.last_tool_batch {
            let allowed_phase = matches!(
                self.phase,
                Some(
                    RunPhase::AfterToolBatch
                        | RunPhase::BeforeFinalize
                        | RunPhase::Completed
                        | RunPhase::Failed
                        | RunPhase::Cancelling
                        | RunPhase::Suspended
                        | RunPhase::Cancelled
                )
            );
            let assigned_count = self
                .tool_calls
                .values()
                .filter(|identity| identity.tool_batch_id == Some(closed.tool_batch_id))
                .count();
            // Hoisted out of the membership test below, which was O(results x messages).
            let tool_message_ids = self
                .messages
                .iter()
                .filter(|message| message.role() == crate::MessageRole::Tool)
                .map(crate::Message::id)
                .collect::<std::collections::BTreeSet<_>>();
            if !allowed_phase
                || closed.cycle > self.cycle
                || assigned_count == 0
                || assigned_count != closed.result_message_ids.len()
                || closed
                    .result_message_ids
                    .iter()
                    .any(|message_id| !tool_message_ids.contains(message_id))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the active-batch state machine is validated as one source-ordered invariant"
    )]
    fn validate_active_tool_batch(&self, batch: &ActiveToolBatch) -> Result<(), ()> {
        if !matches!(
            self.phase,
            Some(
                RunPhase::AwaitingTools
                    | RunPhase::AwaitingExternal
                    | RunPhase::Cancelling
                    | RunPhase::Suspended
            )
        ) || self.pending_model_effect.is_some()
            || batch.opened.cycle != self.cycle
            || batch.calls.is_empty()
            || batch.calls.len() != batch.opened.calls.len()
            || usize::try_from(batch.next_source_index).map_err(|_| ())? > batch.calls.len()
            || batch.result_message_ids.len()
                != usize::try_from(batch.next_source_index).map_err(|_| ())?
            || self
                .current_turn
                .as_ref()
                .is_none_or(|turn| turn.turn_id != batch.opened.turn_id)
        {
            return Err(());
        }

        let expected_external = batch.calls.iter().any(|call| {
            matches!(
                call.status,
                ActiveToolCallStatus::Requested {
                    deferred: Some(_),
                    ..
                }
            )
        });
        if matches!(
            self.phase,
            Some(RunPhase::AwaitingTools | RunPhase::AwaitingExternal)
        ) && (self.phase == Some(RunPhase::AwaitingExternal)) != expected_external
        {
            return Err(());
        }

        let finalized = usize::try_from(batch.next_source_index).map_err(|_| ())?;
        let mut has_current_request = false;
        for (index, (call, opened_call)) in batch
            .calls
            .iter()
            .zip(batch.opened.calls.iter())
            .enumerate()
        {
            if &call.assigned != opened_call
                || call.assigned.source_index != u32::try_from(index).map_err(|_| ())?
            {
                return Err(());
            }
            let expected_group = if index == 0 {
                0
            } else {
                let prior = &batch.opened.calls[index - 1];
                if prior.plan.execution() == crate::ToolExecutionMode::Parallel
                    && call.assigned.plan.execution() == crate::ToolExecutionMode::Parallel
                {
                    prior.group_index
                } else {
                    prior.group_index.checked_add(1).ok_or(())?
                }
            };
            if call.assigned.group_index != expected_group {
                return Err(());
            }
            let identity = self
                .tool_calls
                .get(call.assigned.plan.call().tool_call_id())
                .ok_or(())?;
            if identity.call != *call.assigned.plan.call()
                || identity.tool_batch_id != Some(batch.opened.tool_batch_id)
                || identity.effect_id != Some(call.assigned.effect_id)
            {
                return Err(());
            }

            match (&call.assigned.plan, &call.status) {
                (
                    ToolCallPlan::SyntheticClosure(_),
                    ActiveToolCallStatus::Undispatched | ActiveToolCallStatus::Requested { .. },
                ) => {
                    return Err(());
                }
                (
                    ToolCallPlan::Execute(plan),
                    ActiveToolCallStatus::Requested {
                        requested,
                        deferred,
                    },
                ) => {
                    if call.assigned.group_index != batch.current_group
                        || requested.effect_id() != call.assigned.effect_id
                        || requested.kind() != EffectKind::Tool
                        || requested.relation().is_some()
                        || requested.component() != plan.component.as_ref()
                        || requested.pipeline().is_some()
                        || requested.output_contract() != &plan.output_contract
                        || requested.output_contract().kind != EffectOutputKind::ToolResult
                        || !matches!(requested.input(), EffectInput::Tool { call } if call == &plan.call)
                        || requested.retry_safety() != plan.retry_safety
                        || requested.deadline() != plan.deadline
                        || deferred
                            .as_ref()
                            .is_some_and(|value| value.validate_against(requested).is_err())
                    {
                        return Err(());
                    }
                    has_current_request = true;
                }
                (ToolCallPlan::Execute(_), ActiveToolCallStatus::Undispatched)
                    if call.assigned.group_index <= batch.current_group =>
                {
                    return Err(());
                }
                (
                    ToolCallPlan::Execute(_),
                    ActiveToolCallStatus::Buffered { .. } | ActiveToolCallStatus::Settled { .. },
                ) if call.assigned.group_index > batch.current_group
                    && !matches!(self.phase, Some(RunPhase::Cancelling | RunPhase::Suspended)) =>
                {
                    return Err(());
                }
                (
                    _,
                    ActiveToolCallStatus::Buffered {
                        result,
                        settlement_digest,
                        synthetic,
                        error,
                    },
                ) => {
                    if result.tool_call_id() != call.assigned.plan.call().tool_call_id()
                        || *synthetic != error.is_some()
                        || (*synthetic && !result.is_error())
                    {
                        return Err(());
                    }
                    if matches!(call.assigned.plan, ToolCallPlan::Execute(_))
                        && self
                            .tool_settlements
                            .get(&call.assigned.effect_id)
                            .is_none_or(|entry| entry.digest != *settlement_digest)
                    {
                        return Err(());
                    }
                }
                (
                    _,
                    ActiveToolCallStatus::Settled {
                        result_message_id,
                        settlement_digest,
                    },
                ) => {
                    if self
                        .tool_settlements
                        .get(&call.assigned.effect_id)
                        .is_none_or(|entry| entry.digest != *settlement_digest)
                    {
                        return Err(());
                    }
                    if index >= finalized
                        || batch.result_message_ids.get(index) != Some(result_message_id)
                    {
                        return Err(());
                    }
                }
                _ => {}
            }
            if index < finalized && !matches!(call.status, ActiveToolCallStatus::Settled { .. }) {
                return Err(());
            }
            if index >= finalized && matches!(call.status, ActiveToolCallStatus::Settled { .. }) {
                return Err(());
            }
        }
        let has_fail_run_settlement = batch.calls.iter().any(|call| {
            call.assigned.plan.failure_policy() == crate::ToolFailurePolicy::FailRun
                && self
                    .tool_settlements
                    .get(&call.assigned.effect_id)
                    .is_some_and(|entry| entry.kind == ToolSettlementKind::Failed)
        });
        if batch.fatal_error.is_some() != has_fail_run_settlement {
            return Err(());
        }
        if !has_current_request {
            return Err(());
        }
        Ok(())
    }
}
