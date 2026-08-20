//! Protocol-to-store error mapping shared by all backends.

use finstack_ai_protocol::ProtocolError;
use finstack_ai_runtime::StoreError;

/// Map a protocol failure onto the stable store error surface.
#[must_use]
pub fn protocol_error(error: ProtocolError) -> StoreError {
    match error {
        ProtocolError::LimitExceeded { resource, limit } => {
            StoreError::LimitExceeded { resource, limit }
        }
        ProtocolError::Integrity { reason_code } | ProtocolError::InvalidCbor { reason_code } => {
            StoreError::Integrity { reason_code }
        }
        ProtocolError::Codec { .. } => StoreError::Integrity {
            reason_code: "canonical_codec",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_errors_map_to_stable_store_codes() {
        assert!(matches!(
            protocol_error(ProtocolError::LimitExceeded {
                resource: "records",
                limit: 3
            }),
            StoreError::LimitExceeded {
                resource: "records",
                limit: 3
            }
        ));
        assert!(matches!(
            protocol_error(ProtocolError::Integrity {
                reason_code: "chain_broken"
            }),
            StoreError::Integrity {
                reason_code: "chain_broken"
            }
        ));
    }
}
