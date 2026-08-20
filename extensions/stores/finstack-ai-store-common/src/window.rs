//! Tail-window and full-journal chain verification shared by all backends.

use finstack_ai_kernel::{AppendBatchId, CommittedBatch, Digest, RecordEnvelope};
use finstack_ai_protocol::{verify_chain, verify_chain_from};
use finstack_ai_runtime::StoreError;

use crate::error::protocol_error;

/// Stable integrity reason codes for one load window flavor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowCodes {
    /// A required record is missing.
    pub gap: &'static str,
    /// The window start falls inside a committed batch.
    pub split: &'static str,
    /// The tail does not chain from the expected prior checksum.
    pub checksum: &'static str,
}

/// Codes for [`finstack_ai_runtime::LoadWindow::FromSequence`].
pub const FROM_SEQUENCE_WINDOW: WindowCodes = WindowCodes {
    gap: "load_from_sequence_gap",
    split: "load_from_splits_batch",
    checksum: "load_from_prior_checksum_mismatch",
};

/// Codes for [`finstack_ai_runtime::LoadWindow::SnapshotPlusTail`].
pub const SNAPSHOT_WINDOW: WindowCodes = WindowCodes {
    gap: "snapshot_missing_record",
    split: "snapshot_splits_batch",
    checksum: "snapshot_checksum_mismatch",
};

/// Select the batch-aligned tail starting at `start`.
///
/// Returns an empty slice when every batch ends before `start` (the caller's
/// tail verification decides whether that is the empty-at-head case or a gap).
///
/// Equivalence contract: this function and [`check_batch_alignment`] answer
/// the same question — "does `start` sit on a batch boundary?" — from
/// different evidence. Use this one when the candidate batches are in hand
/// (memory-style backends); use [`check_batch_alignment`] when only the batch
/// ids of the records at `start` and `start - 1` are cheap to fetch
/// (sql-style backends). On well-formed journals the two must classify every
/// start identically; a change to one alignment rule must change both.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] with `codes.split` when `start` falls
/// strictly inside a committed batch.
pub fn select_tail_batches(
    batches: &[CommittedBatch],
    start: u64,
    codes: WindowCodes,
) -> Result<&[CommittedBatch], StoreError> {
    let Some(index) = batches
        .iter()
        .position(|batch| batch.first_sequence >= start)
    else {
        if batches
            .last()
            .is_some_and(|batch| batch.last_sequence >= start)
        {
            return Err(StoreError::Integrity {
                reason_code: codes.split,
            });
        }
        return Ok(&[]);
    };
    if batches[index].first_sequence > start
        && index
            .checked_sub(1)
            .and_then(|prior| batches.get(prior))
            .is_some_and(|prior| prior.last_sequence >= start)
    {
        return Err(StoreError::Integrity {
            reason_code: codes.split,
        });
    }
    Ok(&batches[index..])
}

/// Require a window start to sit on a batch boundary.
///
/// `start_batch` is the batch holding the record at the window start;
/// `prior_batch` is the batch holding the record immediately before it, when
/// that record exists.
///
/// Equivalence contract: see [`select_tail_batches`] — the two functions are
/// alternate evidence shapes for the same alignment rule and must agree.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] with `codes.split` when both records
/// share a batch.
pub fn check_batch_alignment(
    start_batch: AppendBatchId,
    prior_batch: Option<AppendBatchId>,
    codes: WindowCodes,
) -> Result<(), StoreError> {
    if prior_batch == Some(start_batch) {
        return Err(StoreError::Integrity {
            reason_code: codes.split,
        });
    }
    Ok(())
}

/// Verify a tail window's chain and head against the stored session head.
///
/// `records` holds every committed record at or after `start`, in sequence
/// order. An empty `records` is valid only when `start` is exactly one past
/// the head and the stored head equals `prior_checksum`.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] with `codes.gap` for missing records,
/// `codes.checksum` for a broken prior link, and `head_checksum_mismatch`
/// when the verified chain does not land on `stored_head`.
pub fn verify_tail_records(
    records: &[RecordEnvelope],
    start: u64,
    prior_checksum: Digest,
    head_sequence: u64,
    stored_head: Option<Digest>,
    codes: WindowCodes,
) -> Result<(), StoreError> {
    if start > head_sequence.saturating_add(1) {
        return Err(StoreError::Integrity {
            reason_code: codes.gap,
        });
    }
    let Some(first) = records.first() else {
        if start != head_sequence.saturating_add(1) {
            return Err(StoreError::Integrity {
                reason_code: codes.gap,
            });
        }
        if stored_head != Some(prior_checksum) {
            return Err(StoreError::Integrity {
                reason_code: codes.checksum,
            });
        }
        return Ok(());
    };
    if first.sequence() != start {
        return Err(StoreError::Integrity {
            reason_code: codes.gap,
        });
    }
    if first.previous_checksum() != Some(prior_checksum) {
        return Err(StoreError::Integrity {
            reason_code: codes.checksum,
        });
    }
    let head =
        verify_chain_from(records, Some(prior_checksum), Some(start)).map_err(protocol_error)?;
    if head != stored_head {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    Ok(())
}

/// Verify a full (or prune-truncated) journal and return its verified head.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when the chain is broken or the verified
/// head differs from `stored_head` (`head_checksum_mismatch`).
pub fn verify_full_head(
    records: &[RecordEnvelope],
    stored_head: Option<Digest>,
) -> Result<Option<Digest>, StoreError> {
    let head = match records.first() {
        Some(first) if first.sequence() > 1 => {
            verify_chain_from(records, first.previous_checksum(), Some(first.sequence()))
                .map_err(protocol_error)?
        }
        _ => verify_chain(records).map_err(protocol_error)?,
    };
    if head != stored_head {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::append::build_committed_batch;
    use finstack_ai_test::store_fixtures::{draft, request};

    fn chained_batches() -> Vec<CommittedBatch> {
        // Batch A: sequences 1-2. Batch B: sequences 3-4.
        let first = build_committed_batch(&request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]), None)
            .expect("first");
        let prior = first.records[1].checksum();
        let second = build_committed_batch(
            &request(2, 1, 3, vec![draft(3, 1), draft(4, 1)]),
            Some(prior),
        )
        .expect("second");
        vec![first, second]
    }

    #[test]
    fn tail_selection_enforces_batch_alignment() {
        let batches = chained_batches();
        // Aligned start returns the suffix.
        let tail = select_tail_batches(&batches, 3, FROM_SEQUENCE_WINDOW).expect("aligned");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].first_sequence, 3);
        // Start inside batch A is a split.
        assert!(matches!(
            select_tail_batches(&batches, 2, FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_splits_batch"
            })
        ));
        // Start inside batch B (the last batch) is also a split.
        assert!(matches!(
            select_tail_batches(&batches, 4, FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_splits_batch"
            })
        ));
        // Start past every batch yields an empty tail (head checks happen later).
        assert!(
            select_tail_batches(&batches, 5, FROM_SEQUENCE_WINDOW)
                .expect("empty tail")
                .is_empty()
        );
    }

    #[test]
    fn tail_verification_checks_gap_checksum_and_head() {
        let batches = chained_batches();
        let head = batches[1].records[1].checksum();
        let prior = batches[0].records[1].checksum();
        let tail: Vec<RecordEnvelope> = batches[1].records.iter().cloned().collect();
        verify_tail_records(&tail, 3, prior, 4, Some(head), FROM_SEQUENCE_WINDOW)
            .expect("verified tail");
        // Empty tail at head+1 needs the stored head to equal the prior checksum.
        verify_tail_records(&[], 5, head, 4, Some(head), FROM_SEQUENCE_WINDOW)
            .expect("empty tail at head");
        assert!(matches!(
            verify_tail_records(&[], 5, prior, 4, Some(head), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_prior_checksum_mismatch"
            })
        ));
        // Start beyond head+1 is a gap.
        assert!(matches!(
            verify_tail_records(&[], 6, head, 4, Some(head), SNAPSHOT_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "snapshot_missing_record"
            })
        ));
        // First record after a hole is a gap.
        assert!(matches!(
            verify_tail_records(&tail, 2, prior, 4, Some(head), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_sequence_gap"
            })
        ));
        // Wrong prior checksum is a checksum mismatch.
        assert!(matches!(
            verify_tail_records(&tail, 3, head, 4, Some(head), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_prior_checksum_mismatch"
            })
        ));
        // Verified chain must land on the stored head.
        assert!(matches!(
            verify_tail_records(&tail, 3, prior, 4, Some(prior), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "head_checksum_mismatch"
            })
        ));
    }

    #[test]
    fn mid_batch_starts_are_rejected_by_alignment() {
        let batches = chained_batches();
        let batch_a = batches[0].batch_id;
        let batch_b = batches[1].batch_id;
        check_batch_alignment(batch_b, Some(batch_a), SNAPSHOT_WINDOW).expect("boundary");
        check_batch_alignment(batch_a, None, SNAPSHOT_WINDOW).expect("genesis");
        assert!(matches!(
            check_batch_alignment(batch_a, Some(batch_a), SNAPSHOT_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "snapshot_splits_batch"
            })
        ));
    }

    #[test]
    fn full_head_verification_accepts_prefixless_journals() {
        let batches = chained_batches();
        let head = batches[1].records[1].checksum();
        let all: Vec<RecordEnvelope> = batches
            .iter()
            .flat_map(|batch| batch.records.iter().cloned())
            .collect();
        assert_eq!(
            verify_full_head(&all, Some(head)).expect("full"),
            Some(head)
        );
        // Pruned journal: records start past sequence 1.
        let tail: Vec<RecordEnvelope> = batches[1].records.iter().cloned().collect();
        assert_eq!(
            verify_full_head(&tail, Some(head)).expect("tail"),
            Some(head)
        );
        assert!(matches!(
            verify_full_head(&all, None),
            Err(StoreError::Integrity {
                reason_code: "head_checksum_mismatch"
            })
        ));
    }
}
