//! Payload-digest and envelope-checksum calculation and verification.

use finstack_ai_kernel::{
    APPEND_BATCH_MAX_RECORDS, DOMAIN_RECORD_ENVELOPE, DOMAIN_RECORD_PAYLOAD, Digest,
    RECORD_ENVELOPE_DIGEST_SCHEMA_VERSION, RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION, RecordBody,
    RecordDraft, RecordEnvelope, RecordId, SessionId, Timestamp,
};
use serde::Serialize;

use crate::error::ProtocolError;
use crate::{APPEND_BATCH_MAX_BYTES, encode};

/// Replay fields hashed into the envelope checksum (contract section 12.1).
///
/// Excludes diagnostic `committed_at` and the checksum field itself.
/// Field names and serde order are the hashed input; do not rename or reorder
/// them.
#[derive(Debug, Clone, Serialize)]
struct EnvelopeChecksumView<'a> {
    format_version: u16,
    kind_version: u16,
    record_id: RecordId,
    session_id: finstack_ai_kernel::SessionId,
    lane_id: finstack_ai_kernel::LaneId,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<finstack_ai_kernel::RunId>,
    sequence: u64,
    timestamp: Timestamp,
    payload_digest: Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_checksum: Option<Digest>,
    derived_event_ids: &'a [finstack_ai_kernel::EventId],
    body: &'a RecordBody,
}

/// Domain-separated SHA-256 of canonical-CBOR `RecordBody` bytes.
///
/// # Errors
///
/// Returns codec or digest-domain failures.
pub fn payload_digest(body: &RecordBody) -> Result<Digest, ProtocolError> {
    domain_digest(
        DOMAIN_RECORD_PAYLOAD,
        RECORD_PAYLOAD_DIGEST_SCHEMA_VERSION,
        &encode(body)?,
    )
}

/// Domain-separated SHA-256 of the replay-field checksum projection.
///
/// # Errors
///
/// Returns codec or digest-domain failures.
pub fn envelope_checksum(envelope: &RecordEnvelope) -> Result<Digest, ProtocolError> {
    checksum_of(&EnvelopeChecksumView {
        format_version: envelope.format_version(),
        kind_version: envelope.kind_version(),
        record_id: envelope.record_id(),
        session_id: envelope.session_id(),
        lane_id: envelope.lane_id(),
        run_id: envelope.run_id(),
        sequence: envelope.sequence(),
        timestamp: envelope.timestamp(),
        payload_digest: envelope.payload_digest(),
        previous_checksum: envelope.previous_checksum(),
        derived_event_ids: envelope.derived_event_ids(),
        body: envelope.body(),
    })
}

/// Assign sequence and fill payload digest plus envelope checksum.
///
/// # Errors
///
/// Returns codec, digest, or record-construction failures.
pub fn commit_record(
    draft: &RecordDraft,
    sequence: u64,
    previous_checksum: Option<Digest>,
    committed_at: Option<Timestamp>,
) -> Result<RecordEnvelope, ProtocolError> {
    commit_record_with_len(draft, sequence, previous_checksum, committed_at)
        .map(|(envelope, _)| envelope)
}

fn commit_record_with_len(
    draft: &RecordDraft,
    sequence: u64,
    previous_checksum: Option<Digest>,
    committed_at: Option<Timestamp>,
) -> Result<(RecordEnvelope, usize), ProtocolError> {
    let payload_digest = payload_digest(draft.body())?;
    let checksum = checksum_of(&EnvelopeChecksumView {
        format_version: draft.format_version(),
        kind_version: draft.kind_version(),
        record_id: draft.record_id(),
        session_id: draft.session_id(),
        lane_id: draft.lane_id(),
        run_id: draft.run_id(),
        sequence,
        timestamp: draft.timestamp(),
        payload_digest,
        previous_checksum,
        derived_event_ids: draft.derived_event_ids(),
        body: draft.body(),
    })?;
    let envelope = RecordEnvelope::try_new(
        draft.format_version(),
        draft.kind_version(),
        draft.record_id(),
        draft.session_id(),
        draft.lane_id(),
        draft.run_id(),
        sequence,
        draft.timestamp(),
        committed_at,
        payload_digest,
        previous_checksum,
        checksum,
        draft.derived_event_ids().to_vec(),
        draft.body().clone(),
    )
    .map_err(|error| ProtocolError::codec(error.to_string()))?;
    let canonical_len = encode(&envelope)?.len();
    Ok((envelope, canonical_len))
}

/// Commit a contiguous sequence of drafts, chaining `previous_checksum`.
///
/// # Errors
///
/// Returns codec, digest, record-construction, or batch-limit failures.
pub fn commit_records(
    drafts: &[RecordDraft],
    first_sequence: u64,
    mut previous_checksum: Option<Digest>,
    committed_at: Option<Timestamp>,
) -> Result<Vec<RecordEnvelope>, ProtocolError> {
    if drafts.len() > APPEND_BATCH_MAX_RECORDS {
        return Err(ProtocolError::limit(
            "batch_records",
            APPEND_BATCH_MAX_RECORDS,
        ));
    }
    let mut records = Vec::with_capacity(drafts.len());
    let mut canonical_bytes = 0_usize;
    for (offset, draft) in drafts.iter().enumerate() {
        let offset =
            u64::try_from(offset).map_err(|_| ProtocolError::integrity("sequence_overflow"))?;
        let sequence = first_sequence
            .checked_add(offset)
            .ok_or_else(|| ProtocolError::integrity("sequence_exhausted"))?;
        let (envelope, canonical_len) =
            commit_record_with_len(draft, sequence, previous_checksum, committed_at)?;
        canonical_bytes = canonical_bytes
            .checked_add(canonical_len)
            .ok_or_else(|| ProtocolError::limit("append_batch", APPEND_BATCH_MAX_BYTES))?;
        if canonical_bytes > APPEND_BATCH_MAX_BYTES {
            return Err(ProtocolError::limit("append_batch", APPEND_BATCH_MAX_BYTES));
        }
        previous_checksum = Some(envelope.checksum());
        records.push(envelope);
    }
    Ok(records)
}

/// Recalculate and compare payload digest and envelope checksum.
///
/// # Errors
///
/// Returns [`ProtocolError::Integrity`] on mismatch.
pub fn verify_envelope(envelope: &RecordEnvelope) -> Result<(), ProtocolError> {
    let expected_payload = payload_digest(envelope.body())?;
    if expected_payload != envelope.payload_digest() {
        return Err(ProtocolError::integrity("payload_digest_mismatch"));
    }
    let expected_checksum = envelope_checksum(envelope)?;
    if expected_checksum != envelope.checksum() {
        return Err(ProtocolError::integrity("envelope_checksum_mismatch"));
    }
    let _ = encode(envelope)?;
    Ok(())
}

/// Verify each envelope and the session checksum/sequence chain.
///
/// Returns the last envelope checksum, which is the session head checksum.
///
/// # Errors
///
/// Returns integrity failures before any caller `apply`.
pub fn verify_chain(records: &[RecordEnvelope]) -> Result<Option<Digest>, ProtocolError> {
    let Some(first) = records.first() else {
        return Ok(None);
    };
    verify_chain_from(records, ChainAnchor::root(first.session_id()))
}

/// Trusted starting point for verification of a complete or truncated chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainAnchor {
    /// Session whose records may appear in the chain.
    pub session_id: SessionId,
    /// Exact sequence expected on the first record.
    pub next_sequence: u64,
    /// Trusted checksum the first record must cite.
    pub previous_checksum: Option<Digest>,
}

impl ChainAnchor {
    /// Anchor an unpruned journal at its canonical root.
    #[must_use]
    pub const fn root(session_id: SessionId) -> Self {
        Self {
            session_id,
            next_sequence: 1,
            previous_checksum: None,
        }
    }

    /// Construct a trusted anchor for a chain whose prefix is unavailable.
    ///
    /// # Errors
    ///
    /// Sequence zero, a checksum at sequence one, or a missing checksum after
    /// sequence one are rejected as invalid anchors.
    pub fn try_new(
        session_id: SessionId,
        next_sequence: u64,
        previous_checksum: Option<Digest>,
    ) -> Result<Self, ProtocolError> {
        if next_sequence == 0 || (next_sequence == 1) != previous_checksum.is_none() {
            return Err(ProtocolError::integrity("invalid_chain_anchor"));
        }
        Ok(Self {
            session_id,
            next_sequence,
            previous_checksum,
        })
    }
}

/// Verify a contiguous record chain from a trusted root or prune checkpoint.
///
/// An empty slice returns the trusted previous checksum.
///
/// # Errors
///
/// Returns integrity failures before any caller `apply`.
pub fn verify_chain_from(
    records: &[RecordEnvelope],
    anchor: ChainAnchor,
) -> Result<Option<Digest>, ProtocolError> {
    let mut previous = anchor.previous_checksum;
    let mut expected_sequence = anchor.next_sequence;
    for record in records {
        verify_envelope(record)?;
        if record.session_id() != anchor.session_id {
            return Err(ProtocolError::integrity("session_chain_mismatch"));
        }
        if record.previous_checksum() != previous {
            return Err(ProtocolError::integrity("checksum_chain_break"));
        }
        if record.sequence() != expected_sequence {
            return Err(ProtocolError::integrity("sequence_gap"));
        }
        expected_sequence = record
            .sequence()
            .checked_add(1)
            .ok_or_else(|| ProtocolError::integrity("sequence_exhausted"))?;
        previous = Some(record.checksum());
    }
    Ok(previous)
}

fn checksum_of(view: &EnvelopeChecksumView<'_>) -> Result<Digest, ProtocolError> {
    domain_digest(
        DOMAIN_RECORD_ENVELOPE,
        RECORD_ENVELOPE_DIGEST_SCHEMA_VERSION,
        &encode(view)?,
    )
}

/// Known-answer hex observed by bindings through the one Rust engine.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct JournalKnownAnswer {
    /// Domain-separated payload digest.
    pub payload_digest: String,
    /// Envelope checksum when `kind` is a committed envelope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// Canonical-CBOR lowercase hex.
    pub cbor_hex: String,
}

/// Compute payload digest / checksum / CBOR hex from diagnostic JSON.
///
/// `kind` is `record_body` or `record_envelope`. This is not a second CBOR
/// implementation; it calls the project encoder.
///
/// # Errors
///
/// Returns codec failures for unknown kinds or invalid diagnostic JSON.
pub fn journal_known_answer(
    kind: &str,
    diagnostic_json: &str,
) -> Result<JournalKnownAnswer, ProtocolError> {
    match kind {
        "record_body" => {
            let body: RecordBody = crate::from_diagnostic_json(diagnostic_json)?;
            let bytes = encode(&body)?;
            Ok(JournalKnownAnswer {
                payload_digest: payload_digest(&body)?.to_hex(),
                checksum: None,
                cbor_hex: hex_lower(&bytes),
            })
        }
        "record_envelope" => {
            let envelope: RecordEnvelope = crate::from_diagnostic_json(diagnostic_json)?;
            verify_envelope(&envelope)?;
            let bytes = encode(&envelope)?;
            Ok(JournalKnownAnswer {
                payload_digest: envelope.payload_digest().to_hex(),
                checksum: Some(envelope.checksum().to_hex()),
                cbor_hex: hex_lower(&bytes),
            })
        }
        _ => Err(ProtocolError::codec(format!(
            "unsupported known-answer kind: {kind}"
        ))),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn domain_digest(
    domain: &str,
    schema_version: u32,
    canonical_bytes: &[u8],
) -> Result<Digest, ProtocolError> {
    Digest::domain_separated(domain, schema_version, canonical_bytes)
        .map_err(|error| ProtocolError::codec(error.to_string()))
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{Metadata, RecordBody, SessionCreated};

    use super::{journal_known_answer, payload_digest};
    use crate::to_diagnostic_json;

    #[test]
    fn journal_known_answer_matches_direct_digest() {
        let body = RecordBody::SessionCreated(SessionCreated::new(Metadata::empty()));
        let json = to_diagnostic_json(&body).expect("json");
        let answer = journal_known_answer("record_body", &json).expect("answer");
        assert_eq!(
            answer.payload_digest,
            payload_digest(&body).expect("digest").to_hex()
        );
        assert!(answer.checksum.is_none());
        assert!(!answer.cbor_hex.is_empty());
    }
}
