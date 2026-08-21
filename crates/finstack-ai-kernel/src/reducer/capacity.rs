//! Hard authoritative-state capacity preflight.

use std::collections::BTreeSet;

use crate::StageCursor;
use crate::content::ContentBlock;
use crate::primitives::{BudgetReservationId, EffectId, ToolCallId};
use crate::primitives::{SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
use crate::records::{RecordBody, RecordEnvelope};
use crate::state::KernelState;

use super::KernelError;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct StateGrowth<'a> {
    pub messages: usize,
    pub stage: Option<StageCursor>,
    pub model: Option<EffectId>,
    pub extension: Option<EffectId>,
    pub completion: Option<&'a str>,
    pub resolution: Option<&'a str>,
    pub tool_calls: &'a [ToolCallId],
    pub tool_settlements: &'a [EffectId],
    pub extra_tool_settlements: usize,
    pub timer_firings: Option<EffectId>,
    pub child_preparations: Option<EffectId>,
    pub budget_reservations: Option<BudgetReservationId>,
    pub budget_charges: Option<EffectId>,
}

#[expect(
    clippy::too_many_lines,
    reason = "decision preflight keeps every semantic collection ceiling in one fail-closed pass"
)]
pub(super) fn preflight_decision(
    state: &KernelState,
    growth: StateGrowth<'_>,
) -> Result<(), KernelError> {
    check(
        "messages",
        state.messages.len(),
        growth.messages,
        SEMANTIC_ARRAY_MAX_ITEMS,
    )?;
    check(
        "stage_settlements",
        state.stage_settlements.len(),
        usize::from(
            growth
                .stage
                .is_some_and(|key| !state.stage_settlements.contains_key(&key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "model_settlements",
        state.model_settlements.len(),
        usize::from(
            growth
                .model
                .is_some_and(|key| !state.model_settlements.contains_key(&key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "extension_settlements",
        state.extension_settlements.len(),
        usize::from(
            growth
                .extension
                .is_some_and(|key| !state.extension_settlements.contains_key(&key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "completion_identities",
        state.completion_identities.len(),
        usize::from(
            growth
                .completion
                .is_some_and(|key| !state.completion_identities.contains_key(key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "resolution_identities",
        state.resolution_identities.len(),
        usize::from(
            growth
                .resolution
                .is_some_and(|key| !state.resolution_identities.contains_key(key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    let tool_call_growth = growth
        .tool_calls
        .iter()
        .filter(|key| !state.tool_calls.contains_key(key))
        .copied()
        .collect::<BTreeSet<_>>()
        .len();
    check(
        "tool_calls",
        state.tool_calls.len(),
        tool_call_growth,
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    let tool_settlement_growth = growth
        .tool_settlements
        .iter()
        .filter(|key| !state.tool_settlements.contains_key(key))
        .copied()
        .collect::<BTreeSet<_>>()
        .len()
        .saturating_add(growth.extra_tool_settlements);
    check(
        "tool_settlements",
        state.tool_settlements.len(),
        tool_settlement_growth,
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "timer_firings",
        state.retry.timer_firings.len(),
        usize::from(
            growth
                .timer_firings
                .is_some_and(|key| !state.retry.timer_firings.contains_key(&key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "child_preparations",
        state.child_preparations.len(),
        usize::from(
            growth
                .child_preparations
                .is_some_and(|key| !state.child_preparations.contains_key(&key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "budget_reservations",
        state.budget_reservations.len(),
        usize::from(
            growth
                .budget_reservations
                .is_some_and(|key| !state.budget_reservations.contains_key(&key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "budget_charges",
        state.budget_charges.len(),
        usize::from(
            growth
                .budget_charges
                .is_some_and(|key| !state.budget_charges.contains_key(&key)),
        ),
        SEMANTIC_MAP_MAX_ENTRIES,
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "batch preflight keeps every semantic collection ceiling in one fail-closed pass"
)]
pub(super) fn preflight_batch(
    state: &KernelState,
    records: &[RecordEnvelope],
) -> Result<(), KernelError> {
    let mut messages = 0_usize;
    let mut stage_keys = BTreeSet::new();
    let mut model_keys = BTreeSet::new();
    let mut extension_keys = BTreeSet::new();
    let mut completion_keys = BTreeSet::new();
    let mut resolution_keys = BTreeSet::new();
    let mut tool_call_keys = BTreeSet::new();
    let mut tool_settlement_keys = BTreeSet::new();
    let mut timer_firing_keys = BTreeSet::new();
    let mut child_preparation_keys = BTreeSet::new();
    let mut budget_reservation_keys = BTreeSet::new();
    let mut budget_charge_keys = BTreeSet::new();
    for record in records {
        match record.body() {
            RecordBody::StageOutcomeRecorded(outcome) => {
                if !state.stage_settlements.contains_key(&outcome.cursor) {
                    stage_keys.insert(outcome.cursor);
                }
            }
            RecordBody::EntryAppended(entry) => {
                messages += 1;
                for block in entry.message.content() {
                    if let ContentBlock::ToolCall(call) = block
                        && !state.tool_calls.contains_key(call.tool_call_id())
                    {
                        tool_call_keys.insert(*call.tool_call_id());
                    }
                }
            }
            RecordBody::ToolCallSettled(settled) => {
                messages += 1;
                if !state.tool_settlements.contains_key(&settled.effect_id) {
                    tool_settlement_keys.insert(settled.effect_id);
                }
            }
            RecordBody::InteractionResolved(value) => {
                if !state
                    .resolution_identities
                    .contains_key(value.resolution_id())
                {
                    resolution_keys.insert(value.resolution_id());
                }
            }
            RecordBody::EffectCompleted(value) => {
                if value.output_contract().kind == crate::EffectOutputKind::ToolResult {
                    if !state.tool_settlements.contains_key(&value.effect_id()) {
                        tool_settlement_keys.insert(value.effect_id());
                    }
                } else if matches!(
                    value.output_contract().kind,
                    crate::EffectOutputKind::ContextContribution
                        | crate::EffectOutputKind::MiddlewareOutcome
                ) && !state.extension_settlements.contains_key(&value.effect_id())
                {
                    extension_keys.insert(value.effect_id());
                } else if value.output_contract().kind == crate::EffectOutputKind::ModelResponse
                    && !state.model_settlements.contains_key(&value.effect_id())
                {
                    model_keys.insert(value.effect_id());
                }
                if let Some(id) = value.completion_id()
                    && !state.completion_identities.contains_key(id)
                {
                    completion_keys.insert(id);
                }
            }
            RecordBody::TimerFired(fired) => {
                if !state.retry.timer_firings.contains_key(&fired.effect_id) {
                    timer_firing_keys.insert(fired.effect_id);
                }
            }
            RecordBody::ChildRunPrepared(prepared) => {
                if !state
                    .child_preparations
                    .contains_key(&prepared.parent_effect_id)
                {
                    child_preparation_keys.insert(prepared.parent_effect_id);
                }
            }
            RecordBody::BudgetReservationRequested(requested) => {
                if !state
                    .budget_reservations
                    .contains_key(&requested.request.reservation_id)
                {
                    budget_reservation_keys.insert(requested.request.reservation_id);
                }
            }
            RecordBody::BudgetChargeRecorded(charged) => {
                if !state
                    .budget_charges
                    .contains_key(&charged.receipt.effect_id)
                {
                    budget_charge_keys.insert(charged.receipt.effect_id);
                }
            }
            RecordBody::EffectFailed(value) => {
                if value.output_contract().kind == crate::EffectOutputKind::ToolResult {
                    if !state.tool_settlements.contains_key(&value.effect_id()) {
                        tool_settlement_keys.insert(value.effect_id());
                    }
                } else if matches!(
                    value.output_contract().kind,
                    crate::EffectOutputKind::ContextContribution
                        | crate::EffectOutputKind::MiddlewareOutcome
                ) && !state.extension_settlements.contains_key(&value.effect_id())
                {
                    extension_keys.insert(value.effect_id());
                } else if value.output_contract().kind == crate::EffectOutputKind::ModelResponse
                    && !state.model_settlements.contains_key(&value.effect_id())
                {
                    model_keys.insert(value.effect_id());
                }
                if let Some(id) = value.completion_id()
                    && !state.completion_identities.contains_key(id)
                {
                    completion_keys.insert(id);
                }
            }
            _ => {}
        }
    }
    check(
        "messages",
        state.messages.len(),
        messages,
        SEMANTIC_ARRAY_MAX_ITEMS,
    )?;
    check(
        "stage_settlements",
        state.stage_settlements.len(),
        stage_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "model_settlements",
        state.model_settlements.len(),
        model_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "extension_settlements",
        state.extension_settlements.len(),
        extension_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "completion_identities",
        state.completion_identities.len(),
        completion_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "resolution_identities",
        state.resolution_identities.len(),
        resolution_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "tool_calls",
        state.tool_calls.len(),
        tool_call_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "tool_settlements",
        state.tool_settlements.len(),
        tool_settlement_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "timer_firings",
        state.retry.timer_firings.len(),
        timer_firing_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "child_preparations",
        state.child_preparations.len(),
        child_preparation_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "budget_reservations",
        state.budget_reservations.len(),
        budget_reservation_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )?;
    check(
        "budget_charges",
        state.budget_charges.len(),
        budget_charge_keys.len(),
        SEMANTIC_MAP_MAX_ENTRIES,
    )
}

fn check(
    field: &'static str,
    current: usize,
    growth: usize,
    maximum: usize,
) -> Result<(), KernelError> {
    if current
        .checked_add(growth)
        .is_none_or(|prospective| prospective > maximum)
    {
        return Err(KernelError::StateCapacityExceeded { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conversation::{Message, MessageRole, ProviderIds};
    use crate::effects::{EffectCompleted, EffectOutputContract, EffectOutputKind};
    use crate::primitives::{
        EffectId, EventId, LaneId, MessageId, ModelRequestId, RecordId, RunId, SessionId,
        ToolCallId, TurnId,
    };
    use crate::primitives::{Metadata, RawJson};
    use crate::records::lifecycle::EntryAppended;
    use crate::records::tools::{ToolCallIdentity, ToolSettlementFingerprint, ToolSettlementKind};
    use crate::state::{CompletionIdentity, ModelSettlementFingerprint, ModelSettlementKind};
    use crate::{
        ContentBlock, Digest, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, Stage, TextBlock,
        Timestamp,
    };

    #[test]
    fn exact_capacity_is_allowed_and_one_over_is_stable() {
        for (field, maximum) in [
            ("messages", SEMANTIC_ARRAY_MAX_ITEMS),
            ("stage_settlements", SEMANTIC_MAP_MAX_ENTRIES),
            ("model_settlements", SEMANTIC_MAP_MAX_ENTRIES),
            ("completion_identities", SEMANTIC_MAP_MAX_ENTRIES),
            ("timer_firings", SEMANTIC_MAP_MAX_ENTRIES),
            ("child_preparations", SEMANTIC_MAP_MAX_ENTRIES),
            ("budget_reservations", SEMANTIC_MAP_MAX_ENTRIES),
            ("budget_charges", SEMANTIC_MAP_MAX_ENTRIES),
        ] {
            assert_eq!(check(field, maximum, 0, maximum), Ok(()));
            assert_eq!(
                check(field, maximum, 1, maximum),
                Err(KernelError::StateCapacityExceeded { field })
            );
        }
    }

    #[test]
    fn decision_preflight_allows_at_limit_duplicates() {
        let state = full_capacity_state();
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    messages: 0,
                    stage: Some(full_stage_key()),
                    model: Some(full_model_key()),
                    completion: Some("completion-0"),
                    ..StateGrowth::default()
                },
            ),
            Ok(()),
            "at-limit duplicates do not grow any collection"
        );
    }

    #[test]
    fn decision_preflight_allows_one_below_growth_for_every_field() {
        let state = full_capacity_state();
        let mut one_below = state.clone();
        one_below.messages = vec![assistant_message(); SEMANTIC_ARRAY_MAX_ITEMS - 1].into();
        assert_eq!(preflight_decision(&one_below, message_growth()), Ok(()));

        let mut one_below_stage = state.clone();
        one_below_stage.stage_settlements.remove(&full_stage_key());
        assert_eq!(
            preflight_decision(
                &one_below_stage,
                StateGrowth {
                    messages: 0,
                    stage: Some(full_stage_key()),
                    model: Some(full_model_key()),
                    completion: Some("completion-0"),
                    ..StateGrowth::default()
                },
            ),
            Ok(()),
            "one-below stage map may grow to the exact limit"
        );
        let mut one_below_model = state.clone();
        one_below_model.model_settlements.remove(&full_model_key());
        assert_eq!(
            preflight_decision(
                &one_below_model,
                StateGrowth {
                    messages: 0,
                    stage: Some(full_stage_key()),
                    model: Some(full_model_key()),
                    completion: Some("completion-0"),
                    ..StateGrowth::default()
                },
            ),
            Ok(()),
            "one-below model map may grow to the exact limit"
        );
        let mut one_below_completion = state.clone();
        one_below_completion
            .completion_identities
            .remove("completion-0");
        assert_eq!(
            preflight_decision(
                &one_below_completion,
                StateGrowth {
                    messages: 0,
                    stage: Some(full_stage_key()),
                    model: Some(full_model_key()),
                    completion: Some("completion-0"),
                    ..StateGrowth::default()
                },
            ),
            Ok(()),
            "one-below completion map may grow to the exact limit"
        );
    }

    #[test]
    fn decision_preflight_rejects_growth_past_each_map_limit() {
        let state = full_capacity_state();
        let error = preflight_decision(
            &state,
            StateGrowth {
                messages: 0,
                stage: Some(StageCursor {
                    cycle: 10_000,
                    stage: Stage::AfterModel,
                }),
                model: Some(full_model_key()),
                completion: Some("completion-0"),
                ..StateGrowth::default()
            },
        )
        .expect_err("stage map growth past its limit");
        assert_eq!(
            error,
            KernelError::StateCapacityExceeded {
                field: "stage_settlements"
            }
        );
        let error = preflight_decision(
            &state,
            StateGrowth {
                messages: 0,
                stage: Some(full_stage_key()),
                model: Some(effect_id(10_001)),
                completion: Some("completion-0"),
                ..StateGrowth::default()
            },
        )
        .expect_err("model map growth past its limit");
        assert_eq!(
            error,
            KernelError::StateCapacityExceeded {
                field: "model_settlements"
            }
        );
        let error = preflight_decision(
            &state,
            StateGrowth {
                messages: 0,
                stage: Some(full_stage_key()),
                model: Some(full_model_key()),
                completion: Some("completion-new"),
                ..StateGrowth::default()
            },
        )
        .expect_err("completion map growth past its limit");
        assert_eq!(
            error,
            KernelError::StateCapacityExceeded {
                field: "completion_identities"
            }
        );
    }

    #[test]
    fn decision_preflight_reports_fixed_field_precedence() {
        let state = full_capacity_state();
        let error = preflight_decision(&state, compound_growth())
            .expect_err("compound growth must report the first overflowing field");
        assert_eq!(
            error,
            KernelError::StateCapacityExceeded { field: "messages" }
        );
    }

    #[test]
    fn tool_map_preflight_allows_exact_limit_and_rejects_one_over_in_fixed_order() {
        let state = full_capacity_state();
        let duplicate_call = tool_call_id(0);
        let duplicate_settlement = effect_id(600);
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    tool_calls: &[duplicate_call],
                    tool_settlements: &[duplicate_settlement],
                    ..StateGrowth::default()
                },
            ),
            Ok(())
        );

        let mut one_below = state.clone();
        one_below.tool_calls.remove(&duplicate_call);
        one_below.tool_settlements.remove(&duplicate_settlement);
        assert_eq!(
            preflight_decision(
                &one_below,
                StateGrowth {
                    tool_calls: &[duplicate_call],
                    tool_settlements: &[duplicate_settlement],
                    ..StateGrowth::default()
                },
            ),
            Ok(())
        );

        let new_call = tool_call_id(10_000);
        let new_settlement = effect_id(10_001);
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    tool_calls: &[new_call],
                    tool_settlements: &[new_settlement],
                    ..StateGrowth::default()
                },
            ),
            Err(KernelError::StateCapacityExceeded {
                field: "tool_calls"
            })
        );
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    tool_settlements: &[new_settlement],
                    ..StateGrowth::default()
                },
            ),
            Err(KernelError::StateCapacityExceeded {
                field: "tool_settlements"
            })
        );
    }

    fn full_capacity_state() -> KernelState {
        KernelState {
            messages: vec![assistant_message(); SEMANTIC_ARRAY_MAX_ITEMS].into(),
            stage_settlements: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|cycle| {
                    (
                        StageCursor {
                            cycle: u64::try_from(cycle).expect("cycle fits u64"),
                            stage: Stage::BeforeRun,
                        },
                        Digest::raw_json(format!("stage-{cycle}").as_bytes()),
                    )
                })
                .collect(),
            model_settlements: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|ordinal| {
                    (
                        effect_id(u64::try_from(ordinal + 10).expect("ordinal fits u64")),
                        ModelSettlementFingerprint {
                            kind: ModelSettlementKind::Completed,
                            digest: Digest::raw_json(format!("model-{ordinal}").as_bytes()),
                        },
                    )
                })
                .collect(),
            completion_identities: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|ordinal| {
                    let effect_id =
                        effect_id(u64::try_from(ordinal + 300).expect("ordinal fits u64"));
                    (
                        Arc::from(format!("completion-{ordinal}")),
                        CompletionIdentity {
                            effect_id,
                            settlement_digest: Digest::raw_json(
                                format!("completion-{ordinal}").as_bytes(),
                            ),
                        },
                    )
                })
                .collect(),
            tool_calls: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|ordinal| {
                    let ordinal = u64::try_from(ordinal).expect("ordinal fits u64");
                    let identity = tool_call_identity(ordinal);
                    (*identity.call.tool_call_id(), identity)
                })
                .collect(),
            tool_settlements: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|ordinal| {
                    let ordinal = u64::try_from(ordinal + 600).expect("ordinal fits u64");
                    (
                        effect_id(ordinal),
                        ToolSettlementFingerprint {
                            kind: ToolSettlementKind::Completed,
                            digest: Digest::raw_json(format!("tool-{ordinal}").as_bytes()),
                        },
                    )
                })
                .collect(),
            ..KernelState::default()
        }
    }

    const fn full_stage_key() -> StageCursor {
        StageCursor {
            cycle: 0,
            stage: Stage::BeforeRun,
        }
    }

    fn full_model_key() -> EffectId {
        effect_id(10)
    }

    fn message_growth() -> StateGrowth<'static> {
        StateGrowth {
            messages: 1,
            stage: Some(full_stage_key()),
            model: Some(full_model_key()),
            completion: Some("completion-0"),
            ..StateGrowth::default()
        }
    }

    fn compound_growth() -> StateGrowth<'static> {
        StateGrowth {
            messages: 1,
            stage: Some(StageCursor {
                cycle: 10_000,
                stage: Stage::AfterModel,
            }),
            model: Some(effect_id(10_001)),
            completion: Some("completion-new"),
            ..StateGrowth::default()
        }
    }

    #[test]
    fn multi_record_completion_batch_preflights_atomically_at_boundary() {
        let message = assistant_message();
        let records = completion_records(&message);
        let mut exact = KernelState {
            messages: vec![message.clone(); SEMANTIC_ARRAY_MAX_ITEMS - 1].into(),
            ..KernelState::default()
        };
        assert_eq!(preflight_batch(&exact, &records), Ok(()));
        exact.messages = vec![message; SEMANTIC_ARRAY_MAX_ITEMS].into();
        assert_eq!(
            preflight_batch(&exact, &records),
            Err(KernelError::StateCapacityExceeded { field: "messages" })
        );
        assert_eq!(exact.messages.len(), SEMANTIC_ARRAY_MAX_ITEMS);
        assert!(exact.model_settlements.is_empty());
        assert!(exact.completion_identities.is_empty());
    }

    fn completion_records(message: &Message) -> Vec<RecordEnvelope> {
        let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("effect");
        let turn_id = TurnId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("turn");
        let request_id =
            ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("request");
        let completion = EffectCompleted::try_new(
            effect_id,
            EffectOutputContract {
                kind: EffectOutputKind::ModelResponse,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"schema"),
            },
            RawJson::parse("{}").expect("output"),
            None,
            vec![],
            ProviderIds::empty(),
            Some("completion"),
            None,
        )
        .expect("completion");
        let entry = EntryAppended {
            cycle: 0,
            turn_id,
            model_request_id: request_id,
            effect_id,
            parent_message_id: None,
            message: message.clone(),
        };
        vec![
            envelope(1, 1, RecordBody::EffectCompleted(completion)),
            envelope(2, 2, RecordBody::EntryAppended(entry)),
        ]
    }

    fn envelope(record: u8, event: u8, body: RecordBody) -> RecordEnvelope {
        RecordEnvelope::try_new(
            RECORD_FORMAT_VERSION,
            RECORD_KIND_VERSION,
            RecordId::parse(&format!("01234567-89ab-7cde-89ab-0123456789{record:02x}"))
                .expect("record"),
            SessionId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("session"),
            LaneId::parse("01234567-89ab-7cde-89ab-0123456789b2").expect("lane"),
            Some(RunId::parse("01234567-89ab-7cde-89ab-0123456789b3").expect("run")),
            u64::from(record),
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            Digest::raw_json(b"payload"),
            None,
            Digest::raw_json(b"checksum"),
            vec![
                EventId::parse(&format!("01234567-89ab-7cde-89ab-0123456789{event:02x}"))
                    .expect("event"),
            ],
            body,
        )
        .expect("envelope")
    }

    fn assistant_message() -> Message {
        Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("message"),
            MessageRole::Assistant,
            vec![ContentBlock::Text(
                TextBlock::try_new("hello").expect("text"),
            )],
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn effect_id(ordinal: u64) -> EffectId {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        EffectId::from_bytes(bytes)
    }

    fn tool_call_id(ordinal: u64) -> ToolCallId {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        ToolCallId::from_bytes(bytes)
    }

    #[test]
    fn four_map_preflight_allows_duplicates_and_rejects_one_over() {
        let state = four_map_capacity_state();
        let duplicate = StateGrowth {
            timer_firings: Some(full_timer_key()),
            child_preparations: Some(full_child_key()),
            budget_reservations: Some(full_reservation_key()),
            budget_charges: Some(full_charge_key()),
            ..StateGrowth::default()
        };
        assert_eq!(preflight_decision(&state, duplicate), Ok(()));

        let mut one_below = state.clone();
        one_below.retry.timer_firings.remove(&full_timer_key());
        one_below.child_preparations.remove(&full_child_key());
        one_below
            .budget_reservations
            .remove(&full_reservation_key());
        one_below.budget_charges.remove(&full_charge_key());
        assert_eq!(preflight_decision(&one_below, duplicate), Ok(()));

        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    timer_firings: Some(effect_id(20_000)),
                    ..StateGrowth::default()
                },
            ),
            Err(KernelError::StateCapacityExceeded {
                field: "timer_firings"
            })
        );
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    child_preparations: Some(effect_id(20_001)),
                    ..StateGrowth::default()
                },
            ),
            Err(KernelError::StateCapacityExceeded {
                field: "child_preparations"
            })
        );
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    budget_reservations: Some(reservation_id(20_002)),
                    ..StateGrowth::default()
                },
            ),
            Err(KernelError::StateCapacityExceeded {
                field: "budget_reservations"
            })
        );
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    budget_charges: Some(effect_id(20_003)),
                    ..StateGrowth::default()
                },
            ),
            Err(KernelError::StateCapacityExceeded {
                field: "budget_charges"
            })
        );
    }

    #[test]
    fn four_map_preflight_reports_field_precedence() {
        let state = four_map_capacity_state();
        let error = preflight_decision(
            &state,
            StateGrowth {
                timer_firings: Some(effect_id(20_000)),
                child_preparations: Some(effect_id(20_001)),
                budget_reservations: Some(reservation_id(20_002)),
                budget_charges: Some(effect_id(20_003)),
                ..StateGrowth::default()
            },
        )
        .expect_err("compound four-map growth reports the first overflowing field");
        assert_eq!(
            error,
            KernelError::StateCapacityExceeded {
                field: "timer_firings"
            }
        );
    }

    #[test]
    fn preflight_decision_timer_bound_rejects_unhashable_growth() {
        let state = four_map_capacity_state();
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    timer_firings: Some(full_timer_key()),
                    ..StateGrowth::default()
                },
            ),
            Ok(())
        );
        assert_eq!(
            preflight_decision(
                &state,
                StateGrowth {
                    timer_firings: Some(effect_id(20_000)),
                    ..StateGrowth::default()
                },
            ),
            Err(KernelError::StateCapacityExceeded {
                field: "timer_firings"
            })
        );
    }

    fn four_map_capacity_state() -> KernelState {
        KernelState {
            retry: crate::RetryState {
                timer_firings: (0..SEMANTIC_MAP_MAX_ENTRIES)
                    .map(|ordinal| {
                        let effect_id =
                            effect_id(u64::try_from(ordinal + 800).expect("ordinal fits u64"));
                        (
                            effect_id,
                            crate::TimerFired {
                                effect_id,
                                due_at: Timestamp::from_unix_ms(1_000).expect("due"),
                                fired_at: Timestamp::from_unix_ms(1_000).expect("fired"),
                            },
                        )
                    })
                    .collect(),
                ..crate::RetryState::default()
            },
            child_preparations: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|ordinal| {
                    let ordinal = u64::try_from(ordinal).expect("ordinal fits u64");
                    let prepared = child_prepared(ordinal);
                    (prepared.parent_effect_id, prepared)
                })
                .collect(),
            budget_reservations: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|ordinal| {
                    let ordinal = u64::try_from(ordinal).expect("ordinal fits u64");
                    let replay = budget_replay(ordinal);
                    (replay.request.reservation_id, replay)
                })
                .collect(),
            budget_charges: (0..SEMANTIC_MAP_MAX_ENTRIES)
                .map(|ordinal| {
                    let ordinal = u64::try_from(ordinal + 900).expect("ordinal fits u64");
                    (
                        effect_id(ordinal),
                        crate::BudgetChargeReceipt {
                            scope_id: scope_id(ordinal),
                            reservation_id: reservation_id(ordinal),
                            effect_id: effect_id(ordinal),
                            charged_usage: crate::Usage::empty(),
                            cumulative_usage: crate::Usage::empty(),
                            usage_digest: Digest::raw_json(b"usage"),
                            receipt_digest: Digest::raw_json(b"charge"),
                        },
                    )
                })
                .collect(),
            ..KernelState::default()
        }
    }

    fn full_timer_key() -> EffectId {
        effect_id(800)
    }

    fn full_child_key() -> EffectId {
        effect_id(1_000)
    }

    fn full_reservation_key() -> crate::BudgetReservationId {
        reservation_id(0)
    }

    fn full_charge_key() -> EffectId {
        effect_id(900)
    }

    fn child_prepared(ordinal: u64) -> crate::ChildRunPrepared {
        let parent_run = run_id(ordinal);
        crate::ChildRunPrepared {
            parent_run_id: parent_run,
            parent_effect_id: effect_id(ordinal + 1_000),
            child: crate::ChildRunLocator {
                operation: crate::OperationLocator::try_new(
                    "tenant-a",
                    session_id(ordinal),
                    lane_id(ordinal),
                    run_id(ordinal + 50_000),
                )
                .expect("child locator"),
                remote: None,
            },
            request_digest: Digest::raw_json(b"child-request"),
            placement: crate::ChildPlacement::CompatibleLaneInParentSession,
            budget_reservation_id: None,
        }
    }

    fn budget_replay(ordinal: u64) -> crate::BudgetReservationReplay {
        let reservation_id = reservation_id(ordinal);
        let run_id = run_id(ordinal + 50_000);
        let amount = crate::BudgetRequest::default();
        let request_digest = crate::BudgetReserveRequest::compute_digest(
            scope_id(ordinal),
            reservation_id,
            run_id,
            &amount,
        )
        .expect("request digest");
        crate::BudgetReservationReplay {
            request: crate::BudgetReserveRequest {
                scope_id: scope_id(ordinal),
                reservation_id,
                run_id,
                amount,
                request_digest,
            },
            settlement: None,
            release: None,
        }
    }

    fn reservation_id(ordinal: u64) -> crate::BudgetReservationId {
        typed_id(ordinal)
    }

    fn scope_id(ordinal: u64) -> crate::BudgetScopeId {
        typed_id(ordinal)
    }

    fn run_id(ordinal: u64) -> RunId {
        typed_id(ordinal)
    }

    fn session_id(ordinal: u64) -> SessionId {
        typed_id(ordinal)
    }

    fn lane_id(ordinal: u64) -> LaneId {
        typed_id(ordinal)
    }

    fn typed_id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn tool_call_identity(ordinal: u64) -> ToolCallIdentity {
        let call = crate::ToolCallBlock::try_new(
            tool_call_id(ordinal),
            "fixture_tool",
            RawJson::parse(format!(r#"{{"ordinal":{ordinal}}}"#)).expect("arguments"),
        )
        .expect("tool call");
        ToolCallIdentity {
            cycle: 0,
            turn_id: TurnId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("turn"),
            source_message_id: MessageId::parse("01234567-89ab-7cde-89ab-0123456789a4")
                .expect("message"),
            tool_batch_id: None,
            effect_id: None,
            call,
        }
    }
}
