//! Reconstruction consumes committed envelopes and verified snapshots only.
use crate::{
    JournalSearchSource,
    indexing::{Checkpoint, IndexedEntry},
};
use finstack_ai_embeddings::vector::truncate_to_bytes;
use finstack_ai_kernel::{
    ContentBlock, ConversationEntry, EntryBody, EntryId, Kernel, Message, RecordBody,
    RecordEnvelope, RunAccepted, Sensitivity, SessionId, Timestamp,
};
use finstack_ai_protocol::{ChainAnchor, verify_chain_from, verify_envelope};
use finstack_ai_runtime::ports::journal::LoadedSession;
use finstack_ai_search_core::{
    SearchError, SearchEvidence, SearchHit, SearchProvenance, SourceRef, configuration_digest,
    highest_sensitivity, preview,
};
use std::{collections::BTreeMap, sync::Arc};

pub(crate) struct LoadedView {
    pub(crate) state: Checkpoint,
    pub(crate) entries: Vec<IndexedEntry>,
    pub(crate) examined: usize,
}
pub(crate) fn authorize_record(
    source: &JournalSearchSource,
    record: &RecordEnvelope,
) -> Result<(), SearchError> {
    source.authorize_session(record.session_id())?;
    if let RecordBody::RunAccepted(accepted) = record.body() {
        authorize_accepted(source, accepted)?;
    }
    Ok(())
}
fn authorize_accepted(
    source: &JournalSearchSource,
    accepted: &RunAccepted,
) -> Result<(), SearchError> {
    if accepted.security().tenant_scope() != source.config.scope.tenant.as_ref()
        || accepted
            .security()
            .principal()
            .tenant_scope()
            .is_some_and(|scope| scope != source.config.scope.tenant.as_ref())
    {
        return Err(SearchError::SearchScopeDenied);
    }
    Ok(())
}
pub(crate) fn records(
    source: &JournalSearchSource,
    records: &[RecordEnvelope],
) -> Result<Vec<IndexedEntry>, SearchError> {
    let mut entries = Vec::new();
    for record in records {
        authorize_record(source, record)?;
        let value = match record.body() {
            RecordBody::ConversationEntry(entry) => {
                if entry.lane_id() != record.lane_id() {
                    return Err(SearchError::invalid("journal_entry_lane"));
                }
                match entry.body() {
                    EntryBody::Message(message) => Some((entry.id(), message)),
                }
            }
            RecordBody::EntryAppended(entry) => Some((
                EntryId::from_bytes(entry.message.id().to_bytes()),
                &entry.message,
            )),
            RecordBody::ToolCallSettled(settled) => Some((
                EntryId::from_bytes(settled.message.id().to_bytes()),
                &settled.message,
            )),
            _ => None,
        };
        if let Some((entry, message)) = value {
            let reference = SourceRef::JournalSpan {
                session: record.session_id(),
                lane: record.lane_id(),
                entry,
                first_record: Some(record.record_id()),
                last_record: Some(record.record_id()),
            };
            entries.push(indexed(
                source,
                message,
                reference,
                Some(record.timestamp()),
                vec![format!("committed_sequence:{}", record.sequence()).into()],
            )?);
        }
    }
    Ok(entries)
}
pub(crate) fn loaded(
    source: &JournalSearchSource,
    session: SessionId,
    loaded: &LoadedSession,
    max_records: usize,
) -> Result<LoadedView, SearchError> {
    if loaded.session_id != session {
        return Err(SearchError::SearchScopeDenied);
    }
    let records: Vec<RecordEnvelope> = loaded
        .committed_batches
        .iter()
        .flat_map(|b| b.records.iter())
        .take(max_records + 1)
        .cloned()
        .collect();
    if records.len() > max_records {
        return Err(SearchError::SearchCapacityExceeded {
            resource: "journal_load_records".into(),
        });
    }
    if loaded.head_sequence == 0 {
        return Err(SearchError::invalid("journal_session_missing"));
    }
    let first = records.first();
    let pruned = first.is_none_or(|record| record.sequence() != 1);
    let mut entries = BTreeMap::new();
    if pruned {
        snapshot_entries(source, session, loaded, &records, &mut entries)?;
    } else if verify_chain_from(&records, ChainAnchor::root(session))
        .map_err(|_| SearchError::invalid("journal_chain"))?
        != loaded.head_checksum
    {
        return Err(SearchError::invalid("journal_head"));
    }
    if records
        .last()
        .is_some_and(|record| record.sequence() != loaded.head_sequence)
    {
        return Err(SearchError::invalid("journal_head"));
    }
    for entry in self::records(source, &records)? {
        entries.insert(entry_key(&entry)?, entry);
    }
    let (anchor_sequence, anchor_checksum) = first
        .map_or((loaded.head_sequence, loaded.head_checksum), |record| {
            (record.sequence(), Some(record.checksum()))
        });
    Ok(LoadedView {
        state: Checkpoint {
            next_sequence: loaded
                .head_sequence
                .checked_add(1)
                .ok_or_else(|| SearchError::invalid("journal_sequence"))?,
            checksum: loaded.head_checksum,
            anchor_sequence,
            anchor_checksum,
            historical_complete: !pruned,
            complete: true,
        },
        entries: entries.into_values().collect(),
        examined: records.len(),
    })
}
fn snapshot_entries(
    source: &JournalSearchSource,
    session: SessionId,
    loaded: &LoadedSession,
    records: &[RecordEnvelope],
    entries: &mut BTreeMap<String, IndexedEntry>,
) -> Result<(), SearchError> {
    let snapshot = loaded
        .snapshot
        .as_ref()
        .and_then(finstack_ai_store_common::accelerated_from)
        .ok_or_else(|| SearchError::invalid("journal_snapshot_unverifiable"))?;
    if snapshot.state.session_id() != Some(session)
        || snapshot.sequence != snapshot.state.last_applied_sequence()
        || snapshot.sequence > loaded.head_sequence
    {
        return Err(SearchError::invalid("journal_snapshot_identity"));
    }
    Kernel::try_restore(snapshot.state.clone())
        .map_err(|_| SearchError::invalid("journal_snapshot_state"))?;
    if let Some(accepted) = snapshot.state.accepted() {
        authorize_accepted(source, accepted)?;
    } else {
        return Err(SearchError::SearchScopeDenied);
    }
    let tail = if let Some(first) = records.first()
        && first.sequence() == snapshot.sequence
    {
        verify_envelope(first).map_err(|_| SearchError::invalid("journal_snapshot_anchor"))?;
        if first.checksum() != snapshot.head_checksum {
            return Err(SearchError::invalid("journal_snapshot_anchor"));
        }
        records
            .get(1..)
            .ok_or_else(|| SearchError::invalid("journal_snapshot_tail"))?
    } else {
        records
    };
    let anchor = ChainAnchor::try_new(
        session,
        snapshot
            .sequence
            .checked_add(1)
            .ok_or_else(|| SearchError::invalid("journal_sequence"))?,
        Some(snapshot.head_checksum),
    )
    .map_err(|_| SearchError::invalid("journal_snapshot_anchor"))?;
    if verify_chain_from(tail, anchor).map_err(|_| SearchError::invalid("journal_snapshot_tail"))?
        != loaded.head_checksum
    {
        return Err(SearchError::invalid("journal_head"));
    }
    let lane = snapshot
        .state
        .lane_id()
        .ok_or(SearchError::SearchScopeDenied)?;
    // KernelState.messages contains committed final assistant messages. Prepared
    // context may include injected references and is deliberately not indexed.
    for message in snapshot.state.messages().iter() {
        let canonical = ConversationEntry::from_message(message, None, lane, 0)
            .map_err(|_| SearchError::invalid("journal_snapshot_entry"))?;
        let reference = SourceRef::JournalSpan {
            session,
            lane,
            entry: canonical.id(),
            first_record: None,
            last_record: None,
        };
        let entry = indexed(
            source,
            message,
            reference,
            None,
            vec![
                format!("snapshot_sequence:{}", snapshot.sequence).into(),
                format!("snapshot_checksum:{}", snapshot.head_checksum.to_hex()).into(),
            ],
        )?;
        entries.insert(entry_key(&entry)?, entry);
    }
    Ok(())
}
fn entry_key(entry: &IndexedEntry) -> Result<String, SearchError> {
    let SourceRef::JournalSpan {
        session,
        lane,
        entry,
        ..
    } = &entry.evidence.hit.reference
    else {
        return Err(SearchError::invalid("journal_reference"));
    };
    Ok(format!("{session}/{lane}/{entry}"))
}
fn indexed(
    source: &JournalSearchSource,
    message: &Message,
    reference: SourceRef,
    timestamp: Option<Timestamp>,
    locators: Vec<Arc<str>>,
) -> Result<IndexedEntry, SearchError> {
    let mut text = String::new();
    let mut complete = true;
    let mut sensitivity = source.config.sensitivity;
    classify_json(message.metadata().as_bytes(), &mut sensitivity)?;
    append_blocks(
        message.content(),
        &mut text,
        &mut complete,
        &mut sensitivity,
        source.config.max_entry_bytes,
    )?;
    let evidence = SearchEvidence {
        hit: SearchHit {
            entity_label: None,
            source: source.config.source_id.clone(),
            reference,
            score: 0,
            preview: preview(&text, source.config.limits.max_preview_chars),
            sensitivity,
            provenance: SearchProvenance {
                scope: source.config.scope.clone(),
                content_digest: configuration_digest("journal-entry-message", message)?,
                locators,
                citations: vec![],
            },
        },
        text: text.into(),
        complete,
    };
    evidence.validate(&source.config.limits)?;
    Ok(IndexedEntry {
        evidence,
        timestamp,
    })
}
fn append_blocks(
    blocks: &[ContentBlock],
    text: &mut String,
    complete: &mut bool,
    sensitivity: &mut Sensitivity,
    limit: usize,
) -> Result<(), SearchError> {
    for block in blocks {
        match block {
            ContentBlock::Text(block) => append(text, block.text(), complete, limit),
            ContentBlock::Json(block) => {
                classify_json(block.value().as_bytes(), sensitivity)?;
                append(
                    text,
                    std::str::from_utf8(block.value().as_bytes())
                        .map_err(|_| SearchError::invalid("journal_json"))?,
                    complete,
                    limit,
                );
            }
            ContentBlock::ToolResult(result) => {
                append_blocks(result.content(), text, complete, sensitivity, limit)?;
            }
            ContentBlock::ToolCall(call) => {
                append(text, call.tool_name(), complete, limit);
                append(
                    text,
                    std::str::from_utf8(call.arguments().as_bytes())
                        .map_err(|_| SearchError::invalid("journal_arguments"))?,
                    complete,
                    limit,
                );
            }
            _ => *complete = false,
        }
    }
    Ok(())
}
fn append(text: &mut String, value: &str, complete: &mut bool, limit: usize) {
    if !text.is_empty() && text.len() < limit {
        text.push('\n');
    }
    let bounded = truncate_to_bytes(value, limit.saturating_sub(text.len()));
    *complete &= bounded.len() == value.len();
    text.push_str(bounded);
}
fn classify_json(bytes: &[u8], sensitivity: &mut Sensitivity) -> Result<(), SearchError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| SearchError::invalid("journal_metadata"))?;
    let mut stack = vec![&value];
    let mut visited = 0;
    while let Some(value) = stack.pop() {
        visited += 1;
        if visited > 10_000 {
            return Err(SearchError::invalid("journal_classification_limit"));
        }
        match value {
            serde_json::Value::Object(map) => {
                if let Some(label) = map.get("sensitivity")
                    && let Ok(level) = serde_json::from_value::<Sensitivity>(label.clone())
                {
                    *sensitivity = highest_sensitivity(*sensitivity, level);
                }
                if stack
                    .len()
                    .saturating_add(map.len())
                    .saturating_add(visited)
                    > 10_000
                {
                    return Err(SearchError::invalid("journal_classification_limit"));
                }
                stack.extend(map.values());
            }
            serde_json::Value::Array(values) => {
                if stack
                    .len()
                    .saturating_add(values.len())
                    .saturating_add(visited)
                    > 10_000
                {
                    return Err(SearchError::invalid("journal_classification_limit"));
                }
                stack.extend(values);
            }
            _ => {}
        }
    }
    if *sensitivity == Sensitivity::Credential {
        return Err(SearchError::SearchScopeDenied);
    }
    Ok(())
}
