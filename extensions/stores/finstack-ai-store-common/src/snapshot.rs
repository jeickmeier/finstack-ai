//! Snapshot encode/decode helpers and prune/snapshot admission semantics
//! shared by all store backends.

use finstack_ai_protocol::{decode_opaque_snapshot, encode_snapshot};
use finstack_ai_runtime::ports::journal::{
    AcceleratedRestore, OpaqueSnapshot, StateSnapshotRequest, StoreError,
};

/// Encode a [`StateSnapshotRequest`] into an [`OpaqueSnapshot`], enforcing
/// the caller-supplied byte ceiling.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] (`snapshot_encode_failed`) if the state
/// cannot be canonically encoded, or an error from
/// [`OpaqueSnapshot::try_new`] if the encoded payload exceeds `max_bytes`.
pub fn encode_state_request(
    request: &StateSnapshotRequest,
    max_bytes: usize,
) -> Result<OpaqueSnapshot, StoreError> {
    let (bytes, digest) = encode_snapshot(
        &request.state,
        request.state.last_applied_sequence,
        request.head_checksum,
        request.pending_timer_scheduled_at,
        request.last_model_continuation.as_ref(),
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "snapshot_encode_failed",
    })?;
    OpaqueSnapshot::try_new(
        request.state.last_applied_sequence,
        digest,
        bytes,
        max_bytes,
    )
}

/// Decode an [`OpaqueSnapshot`] into an [`AcceleratedRestore`], returning
/// `None` when the payload cannot be decoded so callers can fall back to a
/// full journal replay.
#[must_use]
pub fn accelerated_from(snapshot: &OpaqueSnapshot) -> Option<AcceleratedRestore> {
    let decoded =
        decode_opaque_snapshot(snapshot.sequence(), snapshot.digest(), snapshot.bytes()).ok()?;
    Some(AcceleratedRestore {
        sequence: decoded.sequence,
        head_checksum: decoded.head_checksum,
        pending_timer_scheduled_at: decoded.pending_timer_scheduled_at,
        last_model_continuation: decoded.last_model_continuation,
        state: decoded.state,
    })
}

/// Count the outstanding (unsettled) effect and interaction slots in a
/// restored state.
#[must_use]
pub fn outstanding_count(restored: &AcceleratedRestore) -> u64 {
    u64::from(restored.state.pending_model_effect.is_some())
        .saturating_add(u64::from(restored.state.pending_interaction.is_some()))
}

/// Count the idempotency tombstones retained in a restored state.
#[must_use]
pub fn tombstone_count(restored: &AcceleratedRestore) -> u64 {
    u64::try_from(
        restored
            .state
            .completion_identities
            .len()
            .saturating_add(restored.state.resolution_identities.len())
            .saturating_add(restored.state.model_settlements.len())
            .saturating_add(restored.state.tool_settlements.len()),
    )
    .unwrap_or(u64::MAX)
}

/// Reject snapshot payloads over the configured byte ceiling.
///
/// # Errors
///
/// Returns [`StoreError::LimitExceeded`] (`snapshot_bytes`).
pub fn check_snapshot_size(bytes: usize, limit: usize) -> Result<(), StoreError> {
    if bytes > limit {
        return Err(StoreError::LimitExceeded {
            resource: "snapshot_bytes",
            limit,
        });
    }
    Ok(())
}

/// Enforce snapshot-sequence monotonicity against the journal head.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] with `snapshot_ahead_of_journal`
/// or `snapshot_sequence_regression`.
pub fn admit_snapshot_sequence(
    snapshot_sequence: u64,
    head_sequence: u64,
    current_snapshot_sequence: Option<u64>,
) -> Result<(), StoreError> {
    if snapshot_sequence > head_sequence {
        return Err(StoreError::InvalidRequest {
            reason_code: "snapshot_ahead_of_journal",
        });
    }
    if current_snapshot_sequence.is_some_and(|current| current > snapshot_sequence) {
        return Err(StoreError::InvalidRequest {
            reason_code: "snapshot_sequence_regression",
        });
    }
    Ok(())
}

/// Require a prune's snapshot to cover a real, committed prefix.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] (`prune_snapshot_not_aligned`).
pub fn admit_prune_snapshot(snapshot_sequence: u64, head_sequence: u64) -> Result<(), StoreError> {
    if snapshot_sequence == 0 || snapshot_sequence > head_sequence {
        return Err(StoreError::InvalidRequest {
            reason_code: "prune_snapshot_not_aligned",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use finstack_ai_kernel::Digest;
    use finstack_ai_runtime::ports::journal::OpaqueSnapshot;

    use super::*;

    #[test]
    fn snapshot_admission_enforces_size_head_and_regression() {
        check_snapshot_size(3, 3).expect("at limit");
        assert!(matches!(
            check_snapshot_size(4, 3),
            Err(StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: 3
            })
        ));
        admit_snapshot_sequence(2, 5, None).expect("first snapshot");
        admit_snapshot_sequence(3, 5, Some(2)).expect("advance");
        admit_snapshot_sequence(3, 5, Some(3)).expect("same sequence rewrites");
        assert!(matches!(
            admit_snapshot_sequence(6, 5, None),
            Err(StoreError::InvalidRequest {
                reason_code: "snapshot_ahead_of_journal"
            })
        ));
        assert!(matches!(
            admit_snapshot_sequence(2, 5, Some(3)),
            Err(StoreError::InvalidRequest {
                reason_code: "snapshot_sequence_regression"
            })
        ));
    }

    #[test]
    fn prune_admission_requires_an_aligned_covering_snapshot() {
        admit_prune_snapshot(3, 5).expect("covered");
        for (sequence, head) in [(0, 5), (6, 5)] {
            assert!(matches!(
                admit_prune_snapshot(sequence, head),
                Err(StoreError::InvalidRequest {
                    reason_code: "prune_snapshot_not_aligned"
                })
            ));
        }
    }

    #[test]
    fn undecodable_snapshot_bytes_yield_no_accelerated_restore() {
        let snapshot = OpaqueSnapshot::try_new(1, Digest::raw_json(b"{}"), b"junk".as_slice(), 16)
            .expect("snapshot");
        assert!(accelerated_from(&snapshot).is_none());
    }
}
