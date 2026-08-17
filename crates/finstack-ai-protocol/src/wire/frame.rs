//! Shared 4-byte big-endian length prefix (ADR-021 / TDD §28.2).
//!
//! The declared length is untrusted and is rejected before a payload buffer
//! is allocated.

use crate::error::ProtocolError;

/// Pre-authentication frame ceiling (TDD §28.2).
pub const PRE_AUTH_FRAME_MAX_BYTES: usize = 16 * 1024;
/// Default post-authentication frame ceiling. Still checked before allocation.
///
/// This is not the 8 MiB journal envelope limit.
pub const POST_AUTH_FRAME_MAX_BYTES: usize = 256 * 1024;
/// Length-prefix width in bytes.
pub const FRAME_LENGTH_BYTES: usize = 4;

/// Encode `payload` as `u32` big-endian length plus the payload bytes.
///
/// # Errors
///
/// Returns [`ProtocolError::LimitExceeded`] when `payload` exceeds `ceiling`.
///
/// # Examples
///
/// ```
/// use finstack_ai_protocol::{PRE_AUTH_FRAME_MAX_BYTES, decode_frame_len, encode_frame};
///
/// let frame = encode_frame(b"hi", PRE_AUTH_FRAME_MAX_BYTES).expect("frame");
/// assert_eq!(&frame[..4], &[0, 0, 0, 2]);
/// assert_eq!(
///     decode_frame_len(frame[..4].try_into().expect("hdr"), PRE_AUTH_FRAME_MAX_BYTES)
///         .expect("len"),
///     2
/// );
/// ```
pub fn encode_frame(payload: &[u8], ceiling: usize) -> Result<Vec<u8>, ProtocolError> {
    let declared = checked_len(payload.len(), ceiling)?;
    let mut out = Vec::with_capacity(FRAME_LENGTH_BYTES + payload.len());
    out.extend_from_slice(&declared.to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Read the declared payload length from a 4-byte header without allocating.
///
/// # Errors
///
/// Returns [`ProtocolError::LimitExceeded`] when the declared length exceeds
/// `ceiling`. Callers must not allocate a payload buffer until this succeeds.
pub fn decode_frame_len(
    header: [u8; FRAME_LENGTH_BYTES],
    ceiling: usize,
) -> Result<usize, ProtocolError> {
    let declared = usize::try_from(u32::from_be_bytes(header))
        .map_err(|_| ProtocolError::limit("frame_payload", ceiling))?;
    checked_len(declared, ceiling)?;
    Ok(declared)
}

/// Split a complete frame into payload bytes after the length check.
///
/// # Errors
///
/// Returns a limit or truncated-frame failure.
pub fn decode_frame(bytes: &[u8], ceiling: usize) -> Result<&[u8], ProtocolError> {
    if bytes.len() < FRAME_LENGTH_BYTES {
        return Err(ProtocolError::invalid("truncated_frame"));
    }
    let header: [u8; FRAME_LENGTH_BYTES] = bytes[..FRAME_LENGTH_BYTES]
        .try_into()
        .map_err(|_| ProtocolError::invalid("truncated_frame"))?;
    let declared = decode_frame_len(header, ceiling)?;
    let rest = &bytes[FRAME_LENGTH_BYTES..];
    if rest.len() != declared {
        return Err(ProtocolError::invalid("frame_length_mismatch"));
    }
    Ok(rest)
}

fn checked_len(len: usize, ceiling: usize) -> Result<u32, ProtocolError> {
    if len > ceiling {
        return Err(ProtocolError::limit("frame_payload", ceiling));
    }
    u32::try_from(len).map_err(|_| ProtocolError::limit("frame_payload", ceiling))
}

#[cfg(test)]
mod tests {
    use super::{
        FRAME_LENGTH_BYTES, PRE_AUTH_FRAME_MAX_BYTES, decode_frame, decode_frame_len, encode_frame,
    };
    use crate::ProtocolError;

    #[test]
    fn one_over_pre_auth_ceiling_fails_before_payload_allocation() {
        let header = u32::try_from(PRE_AUTH_FRAME_MAX_BYTES + 1)
            .expect("fits")
            .to_be_bytes();
        assert!(matches!(
            decode_frame_len(header, PRE_AUTH_FRAME_MAX_BYTES),
            Err(ProtocolError::LimitExceeded {
                resource: "frame_payload",
                limit: PRE_AUTH_FRAME_MAX_BYTES
            })
        ));
    }

    #[test]
    fn exact_pre_auth_ceiling_is_accepted() {
        let payload = vec![0x61; PRE_AUTH_FRAME_MAX_BYTES];
        let frame = encode_frame(&payload, PRE_AUTH_FRAME_MAX_BYTES).expect("encode");
        assert_eq!(frame.len(), FRAME_LENGTH_BYTES + PRE_AUTH_FRAME_MAX_BYTES);
        assert_eq!(
            decode_frame(&frame, PRE_AUTH_FRAME_MAX_BYTES).expect("decode"),
            payload
        );
    }

    #[test]
    fn truncated_header_fails_closed() {
        assert!(matches!(
            decode_frame(&[0, 0, 0], PRE_AUTH_FRAME_MAX_BYTES),
            Err(ProtocolError::InvalidCbor {
                reason_code: "truncated_frame"
            })
        ));
    }
}
