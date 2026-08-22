//! Direct versioned kernel-state CBOR snapshot envelope (ADR-032 / snapshot replay contract).

use finstack_ai_kernel::{Digest, KernelState, RawJson, Timestamp};
use serde::{Deserialize, Serialize};

use crate::error::ProtocolError;
use crate::{CanonicalValue, decode_value, encode};

/// Envelope format version accepted by this crate.
pub const SNAPSHOT_ENVELOPE_FORMAT_VERSION: u32 = 2;

/// Decoded snapshot envelope after digest and version checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSnapshot {
    /// Journal sequence covered by the snapshot.
    pub sequence: u64,
    /// Envelope checksum of the record at [`Self::sequence`].
    pub head_checksum: Digest,
    /// `kernel-state` digest recorded at write time.
    pub state_hash: Digest,
    /// Semantic timestamp of a pending `RetryScheduled` record, when present.
    pub pending_timer_scheduled_at: Option<Timestamp>,
    /// Opaque provider continuation from the last successful model settlement.
    pub last_model_continuation: Option<RawJson>,
    /// Existing kernel state wire. This is not a second snapshot DTO.
    pub state: KernelState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotEnvelopeV1 {
    format_version: u32,
    sequence: u64,
    head_checksum: Digest,
    state_hash: Digest,
    pending_timer_scheduled_at: Option<Timestamp>,
    state: KernelState,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotEnvelopeV2 {
    format_version: u32,
    sequence: u64,
    head_checksum: Digest,
    state_hash: Digest,
    pending_timer_scheduled_at: Option<Timestamp>,
    #[serde(default)]
    last_model_continuation: Option<RawJson>,
    state: KernelState,
}

#[derive(Serialize)]
struct SnapshotEnvelopeV2Ref<'a> {
    format_version: u32,
    sequence: u64,
    head_checksum: Digest,
    state_hash: Digest,
    pending_timer_scheduled_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_model_continuation: Option<&'a RawJson>,
    state: &'a KernelState,
}

/// Encode one disposable snapshot envelope as canonical CBOR.
///
/// # Errors
///
/// Returns codec, limit, or `state_hash` failures.
pub fn encode_snapshot(
    state: &KernelState,
    sequence: u64,
    head_checksum: Digest,
    pending_timer_scheduled_at: Option<Timestamp>,
    last_model_continuation: Option<&RawJson>,
) -> Result<(Vec<u8>, Digest), ProtocolError> {
    if sequence != state.last_applied_sequence() {
        return Err(ProtocolError::integrity("snapshot_sequence_mismatch"));
    }
    let state_hash = state
        .state_hash()
        .map_err(|error| ProtocolError::codec(error.to_string()))?;
    let bytes = encode(&SnapshotEnvelopeV2Ref {
        format_version: SNAPSHOT_ENVELOPE_FORMAT_VERSION,
        sequence,
        head_checksum,
        state_hash,
        pending_timer_scheduled_at,
        last_model_continuation,
        state,
    })?;
    let digest = Digest::snapshot_state(&bytes);
    Ok((bytes, digest))
}

/// Decode snapshot envelope bytes and check internal validity.
///
/// # Errors
///
/// Returns codec, limit, version, digest, or state-hash failures.
pub fn decode_snapshot(bytes: &[u8]) -> Result<DecodedSnapshot, ProtocolError> {
    let value = decode_value(bytes)?;
    let format_version = snapshot_format_version(&value)?;
    let envelope = match format_version {
        1 => {
            let v1: SnapshotEnvelopeV1 = crate::cbor::from_canonical(value)?;
            SnapshotEnvelopeV2 {
                format_version: v1.format_version,
                sequence: v1.sequence,
                head_checksum: v1.head_checksum,
                state_hash: v1.state_hash,
                pending_timer_scheduled_at: v1.pending_timer_scheduled_at,
                last_model_continuation: None,
                state: v1.state,
            }
        }
        SNAPSHOT_ENVELOPE_FORMAT_VERSION => crate::cbor::from_canonical(value)?,
        _ => return Err(ProtocolError::integrity("snapshot_format_unsupported")),
    };
    if envelope.format_version != format_version {
        return Err(ProtocolError::integrity("snapshot_format_mismatch"));
    }
    if envelope.sequence != envelope.state.last_applied_sequence() {
        return Err(ProtocolError::integrity("snapshot_sequence_mismatch"));
    }
    let state_hash = envelope
        .state
        .state_hash()
        .map_err(|error| ProtocolError::codec(error.to_string()))?;
    if state_hash != envelope.state_hash {
        return Err(ProtocolError::integrity("snapshot_state_hash_mismatch"));
    }
    Ok(DecodedSnapshot {
        sequence: envelope.sequence,
        head_checksum: envelope.head_checksum,
        state_hash,
        pending_timer_scheduled_at: envelope.pending_timer_scheduled_at,
        last_model_continuation: envelope.last_model_continuation,
        state: envelope.state,
    })
}

fn snapshot_format_version(value: &CanonicalValue) -> Result<u32, ProtocolError> {
    let CanonicalValue::Map(entries) = value else {
        return Err(ProtocolError::codec("snapshot envelope must be a map"));
    };
    let version = entries.iter().find_map(|(key, value)| {
        (*key == CanonicalValue::Text("format_version".into())).then_some(value)
    });
    match version {
        Some(CanonicalValue::Unsigned(version)) => u32::try_from(*version)
            .map_err(|_| ProtocolError::integrity("snapshot_format_unsupported")),
        Some(_) => Err(ProtocolError::codec(
            "snapshot format_version must be unsigned",
        )),
        None => Err(ProtocolError::codec("snapshot format_version missing")),
    }
}

/// Decode opaque snapshot cache bytes and verify the stored digest and sequence.
///
/// # Errors
///
/// Returns the [`decode_snapshot`] failures plus digest or sequence mismatches.
pub fn decode_opaque_snapshot(
    sequence: u64,
    digest: Digest,
    bytes: &[u8],
) -> Result<DecodedSnapshot, ProtocolError> {
    if Digest::snapshot_state(bytes) != digest {
        return Err(ProtocolError::integrity("snapshot_digest_mismatch"));
    }
    let decoded = decode_snapshot(bytes)?;
    if decoded.sequence != sequence {
        return Err(ProtocolError::integrity("snapshot_sequence_mismatch"));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::{Digest, KernelState, RawJson, Timestamp};
    use serde::Serialize;

    use super::{
        SNAPSHOT_ENVELOPE_FORMAT_VERSION, decode_opaque_snapshot, decode_snapshot, encode_snapshot,
    };

    #[test]
    fn default_state_round_trips_and_is_byte_stable() {
        let state = KernelState::default();
        let checksum = finstack_ai_kernel::Digest::raw_json(b"head");
        let (bytes, digest) = encode_snapshot(&state, 0, checksum, None, None).expect("encode");
        let again = encode_snapshot(&state, 0, checksum, None, None).expect("encode again");
        assert_eq!(bytes, again.0);
        assert_eq!(digest, again.1);
        let decoded = decode_opaque_snapshot(0, digest, &bytes).expect("decode");
        assert_eq!(decoded.sequence, 0);
        assert_eq!(decoded.head_checksum, checksum);
        assert_eq!(decoded.state_hash, state.state_hash().expect("hash"));
        assert_eq!(decoded.state, state);
        assert_eq!(SNAPSHOT_ENVELOPE_FORMAT_VERSION, 2);
    }

    #[test]
    fn borrowed_v2_encoder_preserves_the_owned_v2_wire_bytes() {
        #[derive(Serialize)]
        struct OwnedV2 {
            format_version: u32,
            sequence: u64,
            head_checksum: Digest,
            state_hash: Digest,
            pending_timer_scheduled_at: Option<Timestamp>,
            #[serde(skip_serializing_if = "Option::is_none")]
            last_model_continuation: Option<RawJson>,
            state: KernelState,
        }

        let state = KernelState::default();
        let head_checksum = Digest::raw_json(b"head");
        let state_hash = state.state_hash().expect("hash");
        let expected = crate::encode(&OwnedV2 {
            format_version: SNAPSHOT_ENVELOPE_FORMAT_VERSION,
            sequence: 0,
            head_checksum,
            state_hash,
            pending_timer_scheduled_at: None,
            last_model_continuation: None,
            state: state.clone(),
        })
        .expect("owned v2");
        let actual = encode_snapshot(&state, 0, head_checksum, None, None)
            .expect("borrowed v2")
            .0;
        assert_eq!(actual, expected);
    }

    #[test]
    fn v1_rejects_v2_only_continuation_field() {
        let state = KernelState::default();
        let checksum = Digest::raw_json(b"head");
        let v1 = super::SnapshotEnvelopeV1 {
            format_version: 1,
            sequence: 0,
            head_checksum: checksum,
            state_hash: state.state_hash().expect("hash"),
            pending_timer_scheduled_at: None,
            state,
        };
        let mut value = crate::decode_value(&crate::encode(&v1).expect("v1")).expect("tree");
        let crate::CanonicalValue::Map(entries) = &mut value else {
            panic!("map");
        };
        entries.push((
            crate::CanonicalValue::Text("last_model_continuation".into()),
            crate::CanonicalValue::Null,
        ));
        let bytes = crate::encode_value(&value).expect("canonical tamper");
        assert!(decode_snapshot(&bytes).is_err());
    }

    #[test]
    fn model_continuation_sidecar_round_trips() {
        let state = KernelState::default();
        let checksum = finstack_ai_kernel::Digest::raw_json(b"head");
        let continuation = finstack_ai_kernel::RawJson::parse(
            r#"{"provider":"openai.responses","replay_items":[],"version":1}"#,
        )
        .expect("continuation");
        let (bytes, digest) =
            encode_snapshot(&state, 0, checksum, None, Some(&continuation)).expect("encode");
        let decoded = decode_opaque_snapshot(0, digest, &bytes).expect("decode");
        assert_eq!(decoded.last_model_continuation, Some(continuation));
    }

    #[test]
    fn v2_empty_tool_indexes_round_trip() {
        let mut state = KernelState::default();
        state.set_state_version(2);
        state.set_last_applied_sequence(3);
        let checksum = finstack_ai_kernel::Digest::raw_json(b"v2-head");
        let (bytes, digest) = encode_snapshot(&state, 3, checksum, None, None).expect("encode");
        let decoded = decode_opaque_snapshot(3, digest, &bytes).expect("decode");
        assert_eq!(decoded.state.state_version(), 2);
        assert_eq!(decoded.state.last_applied_sequence(), 3);
        assert_eq!(
            decoded.state.state_hash().expect("hash"),
            state.state_hash().expect("hash")
        );
    }

    #[test]
    fn unknown_format_and_wrong_digest_are_rejected() {
        let state = KernelState::default();
        let checksum = finstack_ai_kernel::Digest::raw_json(b"head");
        let (bytes, digest) = encode_snapshot(&state, 0, checksum, None, None).expect("encode");
        let mut value = crate::decode_value(&bytes).expect("value");
        if let crate::CanonicalValue::Map(entries) = &mut value {
            for (key, item) in entries.iter_mut() {
                if *key == crate::CanonicalValue::Text("format_version".into()) {
                    *item = crate::CanonicalValue::Unsigned(3);
                }
            }
        }
        let tampered = crate::encode_value(&value).expect("tamper");
        assert_eq!(
            decode_snapshot(&tampered)
                .expect_err("unknown format")
                .code(),
            "snapshot_format_unsupported"
        );
        assert_eq!(
            decode_opaque_snapshot(0, digest, &bytes[1..])
                .expect_err("wrong bytes")
                .code(),
            "snapshot_digest_mismatch"
        );
    }

    #[test]
    fn message_bearing_state_round_trips() {
        use finstack_ai_kernel::{
            ContentBlock, Id, IdTag, Message, MessageRole, Metadata, ProviderIds, TextBlock,
            Timestamp,
        };

        fn id<T: IdTag>(ordinal: u64) -> Id<T> {
            let mut bytes = [0_u8; 16];
            bytes[6] = 0x70;
            bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            Id::from_bytes(bytes)
        }

        let message = Message::try_new(
            id(4),
            MessageRole::User,
            vec![ContentBlock::Text(
                TextBlock::try_new("Say hello.").expect("text"),
            )],
            Timestamp::from_unix_ms(900).expect("ts"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message");
        let mut state = KernelState::default();
        state.set_messages(std::sync::Arc::new(vec![message.clone()]));
        let checksum = finstack_ai_kernel::Digest::raw_json(b"head");
        let (bytes, digest) = encode_snapshot(&state, 0, checksum, None, None).expect("encode");
        let decoded = decode_opaque_snapshot(0, digest, &bytes).expect("decode");
        assert_eq!(
            decoded.state.messages().as_slice(),
            state.messages().as_slice()
        );
        assert_eq!(
            crate::decode::<Message>(&crate::encode(&message).expect("enc")).expect("msg"),
            message
        );
    }

    #[test]
    fn sequence_mismatch_is_rejected() {
        let state = KernelState::default();
        let checksum = finstack_ai_kernel::Digest::raw_json(b"head");
        encode_snapshot(&state, 1, checksum, None, None).expect_err("sequence");
    }
}
