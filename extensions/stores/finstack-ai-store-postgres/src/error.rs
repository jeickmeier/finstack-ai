// This module's helpers are exercised by unit tests but not yet called from
// non-test code: the schema/pool/append work that consumes them lands in a
// later task per the crate's incremental build-out. Silencing dead_code here
// (rather than adding a synthetic caller) keeps the interface exactly as
// specified without dead scaffolding code.
#![allow(dead_code, reason = "consumed by schema/pool/append in a later task")]

use finstack_ai_runtime::StoreError;

/// SQLSTATE class prefix for integrity-constraint violations (23xxx).
const CONSTRAINT_VIOLATION_CLASS: &str = "23";

/// SQLSTATE for disk-full (`disk_full`).
const DISK_FULL: &str = "53100";

/// SQLSTATE for serialization failures under concurrent transactions.
const SERIALIZATION_FAILURE: &str = "40001";

/// Convert an unsigned 64-bit value to the signed 64-bit representation used
/// for Postgres `BIGINT` columns.
pub(crate) fn i64_from_u64(value: u64, reason_code: &'static str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

/// Convert a stored `BIGINT` back to its unsigned representation.
pub(crate) fn u64_from_i64(value: i64, reason_code: &'static str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

/// Classify a SQLSTATE code (and whether the connection was already closed)
/// into a [`StoreError`].
///
/// Kept as a pure function, separate from [`map_postgres_error`], because
/// `tokio_postgres::Error` cannot be synthesized in unit tests (its
/// constructors are private to the driver); this function carries all the
/// mapping logic and is exercised directly.
pub(crate) fn classify_sqlstate(code: Option<&str>, is_closed: bool) -> StoreError {
    if is_closed {
        return StoreError::Unavailable {
            reason_code: "postgres_unavailable",
        };
    }
    match code {
        Some(DISK_FULL) => StoreError::Unavailable {
            reason_code: "postgres_disk_full",
        },
        Some(SERIALIZATION_FAILURE) => StoreError::Unavailable {
            reason_code: "postgres_serialization",
        },
        Some(sqlstate) if sqlstate.starts_with(CONSTRAINT_VIOLATION_CLASS) => {
            StoreError::Integrity {
                reason_code: "postgres_constraint",
            }
        }
        _ => StoreError::Unavailable {
            reason_code: "postgres_unavailable",
        },
    }
}

/// Map a `tokio_postgres::Error` to the store's stable [`StoreError`] taxonomy.
///
/// A thin adapter over [`classify_sqlstate`]: it exists only to extract the
/// SQLSTATE code and closed-connection flag from the driver's error type.
pub(crate) fn map_postgres_error(error: &tokio_postgres::Error) -> StoreError {
    let code = error.code().map(tokio_postgres::error::SqlState::code);
    classify_sqlstate(code, error.is_closed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_connection_is_unavailable() {
        let error = classify_sqlstate(None, true);
        assert!(matches!(
            error,
            StoreError::Unavailable {
                reason_code: "postgres_unavailable"
            }
        ));
    }

    #[test]
    fn disk_full_sqlstate_is_unavailable() {
        let error = classify_sqlstate(Some("53100"), false);
        assert!(matches!(
            error,
            StoreError::Unavailable {
                reason_code: "postgres_disk_full"
            }
        ));
    }

    #[test]
    fn serialization_failure_sqlstate_is_unavailable() {
        let error = classify_sqlstate(Some("40001"), false);
        assert!(matches!(
            error,
            StoreError::Unavailable {
                reason_code: "postgres_serialization"
            }
        ));
    }

    #[test]
    fn constraint_violation_class_is_integrity() {
        for sqlstate in ["23505", "23503", "23000"] {
            let error = classify_sqlstate(Some(sqlstate), false);
            assert!(
                matches!(
                    error,
                    StoreError::Integrity {
                        reason_code: "postgres_constraint"
                    }
                ),
                "{sqlstate} should map to Integrity"
            );
        }
    }

    #[test]
    fn unknown_sqlstate_defaults_to_unavailable() {
        let error = classify_sqlstate(Some("58030"), false);
        assert!(matches!(
            error,
            StoreError::Unavailable {
                reason_code: "postgres_unavailable"
            }
        ));
    }

    #[test]
    fn missing_sqlstate_defaults_to_unavailable() {
        let error = classify_sqlstate(None, false);
        assert!(matches!(
            error,
            StoreError::Unavailable {
                reason_code: "postgres_unavailable"
            }
        ));
    }

    #[test]
    fn i64_from_u64_rejects_out_of_range() {
        let error = i64_from_u64(u64::MAX, "overflow").unwrap_err();
        assert!(matches!(
            error,
            StoreError::Integrity {
                reason_code: "overflow"
            }
        ));
    }

    #[test]
    fn u64_from_i64_rejects_negative() {
        let error = u64_from_i64(-1, "negative").unwrap_err();
        assert!(matches!(
            error,
            StoreError::Integrity {
                reason_code: "negative"
            }
        ));
    }
}
