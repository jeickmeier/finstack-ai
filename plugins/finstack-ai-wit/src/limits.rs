//! TDD §6.5 payload ceilings enforced before allocation.

use crate::error::WitMapError;

/// Individual text or byte-string ceiling (4 MiB).
pub const MAX_STRING_BYTES: usize = 4 * 1024 * 1024;
/// `RawJson` source and canonical ceiling (1 MiB).
pub const MAX_RAW_JSON_BYTES: usize = 1_048_576;
/// Metadata object ceiling (64 KiB).
pub const MAX_METADATA_BYTES: usize = 64 * 1024;

/// Reject an oversized payload before the caller copies or parses it.
///
/// # Errors
///
/// Returns [`WitMapError::PayloadTooLarge`] when `bytes.len()` exceeds `max`.
pub fn reject_before_allocation(
    bytes: &[u8],
    max: usize,
    field: &'static str,
) -> Result<(), WitMapError> {
    if bytes.len() > max {
        return Err(WitMapError::PayloadTooLarge {
            field,
            len: bytes.len(),
            max,
        });
    }
    Ok(())
}

/// Reject a declared frame length before the host allocates an output buffer.
///
/// # Errors
///
/// Returns [`WitMapError::PayloadTooLarge`] when `len` exceeds `max`.
pub fn reject_declared_len(len: u64, max: usize, field: &'static str) -> Result<(), WitMapError> {
    let Ok(len) = usize::try_from(len) else {
        return Err(WitMapError::PayloadTooLarge {
            field,
            len: usize::MAX,
            max,
        });
    };
    if len > max {
        return Err(WitMapError::PayloadTooLarge { field, len, max });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_STRING_BYTES, reject_before_allocation, reject_declared_len};

    #[test]
    fn oversized_slice_is_rejected_without_copying() {
        let bytes = [0_u8; 8];
        let error =
            reject_before_allocation(&bytes, 4, "args-json").expect_err("oversize must fail");
        assert_eq!(error.code(), "plugin_payload_too_large");
    }

    #[test]
    fn declared_blob_length_is_rejected_before_allocation() {
        let error = reject_declared_len(u64::from(u32::MAX) + 1, MAX_STRING_BYTES, "blobs.read")
            .expect_err("declared oversize must fail");
        assert_eq!(error.code(), "plugin_payload_too_large");
    }
}
