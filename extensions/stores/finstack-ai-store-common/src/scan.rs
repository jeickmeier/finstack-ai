//! Scan-request validation and paging semantics shared by all backends.

use finstack_ai_kernel::RecordEnvelope;
use finstack_ai_runtime::{SCAN_PAGE_MAX_RECORDS, StoreError};

/// Validate a scan page size against the port contract.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] with `scan_limit_zero` or
/// `scan_limit_exceeded`.
pub fn validate_scan_limit(limit: u32) -> Result<(), StoreError> {
    if limit == 0 {
        return Err(StoreError::InvalidRequest {
            reason_code: "scan_limit_zero",
        });
    }
    if limit > SCAN_PAGE_MAX_RECORDS {
        return Err(StoreError::InvalidRequest {
            reason_code: "scan_limit_exceeded",
        });
    }
    Ok(())
}

/// Normalize a scan start: `0` means the first committed record.
#[must_use]
pub const fn scan_start(from_sequence: u64) -> u64 {
    if from_sequence == 0 { 1 } else { from_sequence }
}

/// Next sequence to request after this page, or `None` at the session end.
#[must_use]
pub fn scan_next_sequence(records: &[RecordEnvelope], has_more: bool) -> Option<u64> {
    if !has_more {
        return None;
    }
    records
        .last()
        .and_then(|record| record.sequence().checked_add(1))
}

#[cfg(test)]
mod tests {
    use finstack_ai_runtime::SCAN_PAGE_MAX_RECORDS;

    use super::*;
    use crate::append::build_committed_batch;
    use finstack_ai_test::store_fixtures::{draft, request};

    #[test]
    fn scan_validation_and_paging_match_the_contract() {
        assert!(matches!(
            validate_scan_limit(0),
            Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_zero"
            })
        ));
        assert!(matches!(
            validate_scan_limit(SCAN_PAGE_MAX_RECORDS + 1),
            Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_exceeded"
            })
        ));
        validate_scan_limit(SCAN_PAGE_MAX_RECORDS).expect("at limit");
        assert_eq!(scan_start(0), 1);
        assert_eq!(scan_start(7), 7);
        let batch = build_committed_batch(&request(1, 1, 1, vec![draft(1, 1)]), None)
            .expect("batch");
        let records: Vec<RecordEnvelope> = batch.records.iter().cloned().collect();
        assert_eq!(scan_next_sequence(&records, true), Some(2));
        assert_eq!(scan_next_sequence(&records, false), None);
        assert_eq!(scan_next_sequence(&[], true), None);
    }
}
