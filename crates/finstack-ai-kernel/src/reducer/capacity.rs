//! Hard authoritative-state capacity preflight.

use std::collections::BTreeSet;

use crate::StageCursor;
use crate::bounds::{SEMANTIC_ARRAY_MAX_ITEMS, SEMANTIC_MAP_MAX_ENTRIES};
use crate::content::ContentBlock;
use crate::ids::{EffectId, ToolCallId};
use crate::records::{RecordBody, RecordEnvelope};
use crate::state::KernelState;

use super::KernelError;

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct StateGrowth<'a> {
    pub messages: usize,
    pub stage: Option<StageCursor>,
    pub model: Option<EffectId>,
    pub completion: Option<&'a str>,
    pub resolution: Option<&'a str>,
    pub tool_calls: &'a [ToolCallId],
    pub tool_settlements: &'a [EffectId],
}

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
        .len();
    check(
        "tool_settlements",
        state.tool_settlements.len(),
        tool_settlement_growth,
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
    let mut completion_keys = BTreeSet::new();
    let mut resolution_keys = BTreeSet::new();
    let mut tool_call_keys = BTreeSet::new();
    let mut tool_settlement_keys = BTreeSet::new();
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
            RecordBody::EffectFailed(value) => {
                if value.output_contract().kind == crate::EffectOutputKind::ToolResult {
                    if !state.tool_settlements.contains_key(&value.effect_id()) {
                        tool_settlement_keys.insert(value.effect_id());
                    }
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
    use crate::effects::{EffectCompleted, EffectOutputContract, EffectOutputKind};
    use crate::entries::EntryAppended;
    use crate::ids::{
        EffectId, EventId, LaneId, MessageId, ModelRequestId, RecordId, RunId, SessionId,
        ToolCallId, TurnId,
    };
    use crate::message::{Message, MessageRole, ProviderIds};
    use crate::raw_json::{Metadata, RawJson};
    use crate::state::{CompletionIdentity, ModelSettlementFingerprint, ModelSettlementKind};
    use crate::tools::{ToolCallIdentity, ToolSettlementFingerprint, ToolSettlementKind};
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
