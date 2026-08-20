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

/// SQLSTATE for a server-detected deadlock. Like [`SERIALIZATION_FAILURE`]
/// this is a *retryable* concurrency outcome, not a durable failure: the
/// append transaction protocol (spec D4) can deadlock when two writers race
/// to create the same session (one holds the new session row, the other
/// holds `store_totals`). The store never retries internally — it reports
/// `Unavailable{postgres_serialization}` and the caller retries the same
/// `AppendRequest`, which the idempotency contract makes safe.
const DEADLOCK_DETECTED: &str = "40P01";

/// A failure plus the disposition of the connection it happened on.
///
/// Shared by the append (spec D4/D5) and load (spec D9) paths: both run
/// statements on a checked-out connection and must decide, for every error,
/// whether that connection may go back to the pool (spec D2).
pub(crate) struct Failure {
    /// The error to report to the caller.
    pub(crate) error: StoreError,
    /// `true` when the connection must never be reused (spec D2/D5).
    pub(crate) poison: bool,
}

impl From<StoreError> for Failure {
    /// A purely logical failure (admission, encoding, integrity): the
    /// connection itself is fine once any open transaction has been rolled
    /// back.
    fn from(error: StoreError) -> Self {
        Self {
            error,
            poison: false,
        }
    }
}

impl Failure {
    /// Classify a driver error: a server-reported failure (one carrying a
    /// SQLSTATE, on a connection that is still open) leaves a usable
    /// connection; anything else — a wire-level IO/protocol/codec failure,
    /// or *any* error observed on a connection the driver has already
    /// closed — poisons it.
    ///
    /// The `is_closed()` half matters on its own: a backend being shut down
    /// reports `57P01 admin_shutdown` as a `DbError` *with* a SQLSTATE, so
    /// the SQLSTATE test alone would hand that dying connection back to the
    /// pool.
    pub(crate) fn from_driver(error: &tokio_postgres::Error) -> Self {
        Self {
            error: map_postgres_error(error),
            poison: error.code().is_none() || error.is_closed(),
        }
    }
}

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
        Some(SERIALIZATION_FAILURE | DEADLOCK_DETECTED) => StoreError::Unavailable {
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
    fn deadlock_sqlstate_is_a_retryable_serialization_failure() {
        let error = classify_sqlstate(Some("40P01"), false);
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

    /// A logical (admission) failure must leave the connection reusable;
    /// only wire-level failures poison it.
    #[test]
    fn logical_failures_do_not_poison_the_connection() {
        let failure = Failure::from(StoreError::Conflict {
            expected_sequence: 1,
            actual_next_sequence: 2,
        });
        assert!(!failure.poison);
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
