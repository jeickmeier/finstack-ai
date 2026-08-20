//! Deterministic record/request builders shared by the journal-store test
//! suites (`finstack-ai-store-common`, `finstack-ai-store-memory`,
//! `finstack-ai-store-sqlite`).
//!
//! The sqlite golden test `append_identity_encoding_is_stable` pins the CBOR
//! bytes of requests built from these fixtures. Changing the id-encoding
//! scheme or any draft field below changes those bytes — do not alter them
//! without revisiting that pin.

#![expect(
    clippy::expect_used,
    reason = "fixture builders are test-only and fail loudly on invalid inputs"
)]

use finstack_ai_kernel::{
    AppendRequest, AuthorizationEvidence, Digest, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, Id, IdTag, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordTag, RunTag, SessionTag, Timestamp,
};

/// Deterministic UUID-shaped id derived from `ordinal`.
#[must_use]
pub fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

/// Committable `ExternalCommandRejected` draft for `record_ordinal` within
/// `session_ordinal`'s session.
///
/// # Panics
///
/// Panics when the fixed fixture inputs are rejected by kernel validation.
#[must_use]
pub fn draft(record_ordinal: u64, session_ordinal: u64) -> RecordDraft {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    let authorization =
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        format!("completion-{record_ordinal}"),
        ExternalCommandTarget::Effect(id(record_ordinal + 1000)),
        principal,
        authorization,
        "conflicting_completion",
        Digest::raw_json(b"{}"),
        None,
    )
    .expect("rejection");
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(record_ordinal),
        id::<SessionTag>(session_ordinal),
        id::<LaneTag>(session_ordinal + 100),
        Some(id::<RunTag>(session_ordinal + 200)),
        Timestamp::from_unix_ms(i64::try_from(record_ordinal).expect("timestamp"))
            .expect("timestamp"),
        Vec::new(),
        RecordBody::ExternalCommandRejected(rejection),
    )
    .expect("draft")
}

/// Frozen append request carrying `drafts` at `expected_sequence`.
///
/// # Panics
///
/// Panics when the fixture inputs are rejected by kernel validation.
#[must_use]
pub fn request(
    batch_ordinal: u64,
    session_ordinal: u64,
    expected_sequence: u64,
    drafts: Vec<RecordDraft>,
) -> AppendRequest {
    AppendRequest::try_new(
        id(batch_ordinal),
        id::<SessionTag>(session_ordinal),
        expected_sequence,
        drafts,
    )
    .expect("append request")
}
