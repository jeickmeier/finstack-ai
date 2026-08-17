use std::collections::BTreeMap;
use std::sync::Arc;

use crate::digest::Digest;
use crate::entries::{Stage, StageCursor};
use crate::ids::{EffectId, ToolCallId};
use crate::tools::{ToolCallIdentity, ToolSettlementFingerprint};

use super::{
    CompletionIdentity, CompletionIdentityHashEntryV1, ModelSettlementFingerprint,
    ModelSettlementHashEntryV1, ResolutionIdentity, ResolutionIdentityHashEntryV6,
    StageSettlementHashEntryV1, ToolCallIdentityHashEntryV2, ToolSettlementHashEntryV2,
};

pub(super) fn stage_hash_entries(
    entries: &BTreeMap<StageCursor, Digest>,
) -> Vec<StageSettlementHashEntryV1> {
    let mut values = entries
        .iter()
        .map(|(cursor, digest)| StageSettlementHashEntryV1 {
            cycle: cursor.cycle,
            stage: cursor.stage,
            settlement_digest: *digest,
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.cycle
            .cmp(&right.cycle)
            .then_with(|| stage_name(left.stage).cmp(stage_name(right.stage)))
    });
    values
}

pub(super) fn model_hash_entries(
    entries: &BTreeMap<EffectId, ModelSettlementFingerprint>,
) -> Vec<ModelSettlementHashEntryV1> {
    entries
        .iter()
        .map(|(effect_id, settlement)| ModelSettlementHashEntryV1 {
            effect_id: *effect_id,
            kind: settlement.kind,
            settlement_digest: settlement.digest,
        })
        .collect()
}

pub(super) fn completion_hash_entries(
    entries: &BTreeMap<Arc<str>, CompletionIdentity>,
) -> Vec<CompletionIdentityHashEntryV1> {
    entries
        .iter()
        .map(|(completion_id, identity)| CompletionIdentityHashEntryV1 {
            completion_id: Arc::clone(completion_id),
            effect_id: identity.effect_id,
            settlement_digest: identity.settlement_digest,
        })
        .collect()
}

pub(super) fn resolution_hash_entries(
    entries: &BTreeMap<Arc<str>, ResolutionIdentity>,
) -> Vec<ResolutionIdentityHashEntryV6> {
    entries
        .iter()
        .map(|(resolution_id, identity)| ResolutionIdentityHashEntryV6 {
            resolution_id: Arc::clone(resolution_id),
            interaction_id: identity.interaction_id,
            settlement_digest: identity.settlement_digest,
        })
        .collect()
}

pub(super) fn tool_call_hash_entries(
    entries: &BTreeMap<ToolCallId, ToolCallIdentity>,
) -> Vec<ToolCallIdentityHashEntryV2> {
    entries
        .iter()
        .map(|(tool_call_id, identity)| ToolCallIdentityHashEntryV2 {
            tool_call_id: *tool_call_id,
            cycle: identity.cycle,
            turn_id: identity.turn_id,
            source_message_id: identity.source_message_id,
            tool_batch_id: identity.tool_batch_id,
            effect_id: identity.effect_id,
            call: identity.call.clone(),
        })
        .collect()
}

pub(super) fn tool_settlement_hash_entries(
    entries: &BTreeMap<EffectId, ToolSettlementFingerprint>,
) -> Vec<ToolSettlementHashEntryV2> {
    entries
        .iter()
        .map(|(effect_id, settlement)| ToolSettlementHashEntryV2 {
            effect_id: *effect_id,
            kind: settlement.kind,
            settlement_digest: settlement.digest,
        })
        .collect()
}

pub(super) const fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::BeforeRun => "before_run",
        Stage::PrepareContext => "prepare_context",
        Stage::BeforeModel => "before_model",
        Stage::AfterModel => "after_model",
        Stage::BeforeToolBatch => "before_tool_batch",
        Stage::AfterToolBatch => "after_tool_batch",
        Stage::BeforeFinalize => "before_finalize",
    }
}
