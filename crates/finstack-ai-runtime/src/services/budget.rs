//! Optional idempotent shared-budget ledger contract.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BudgetChargeReceipt, BudgetChargeRequest, BudgetReleaseReceipt, BudgetReleaseRequest,
    BudgetReservationId, BudgetReservationReceipt, BudgetReserveRequest, BudgetScopeId, Digest,
    PortFuture, PortObject,
};

/// Stable code for an unavailable budget service.
pub const BUDGET_UNAVAILABLE: &str = "budget_unavailable";
/// Stable code for an unknown reconciliation outcome.
pub const BUDGET_UNKNOWN: &str = "budget_unknown";
/// Stable code for conflicting idempotency-key reuse.
pub const BUDGET_CONFLICT: &str = "budget_conflict";
/// Stable code for a malformed or mismatched receipt.
pub const BUDGET_INVALID_RECEIPT: &str = "budget_invalid_receipt";

/// Reconciled reservation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "receipt", rename_all = "snake_case")]
pub enum BudgetReservationState {
    /// Active reservation.
    Reserved(BudgetReservationReceipt),
    /// Released reservation; prior charges remain authoritative.
    Released(BudgetReleaseReceipt),
    /// No reservation was found.
    NotFound,
    /// The backend cannot safely determine the outcome.
    Unknown,
}

/// Optional application-owned shared-budget service.
pub trait BudgetLedger: PortObject {
    /// Reserve one exact amount idempotently.
    fn reserve(
        &self,
        request: BudgetReserveRequest,
    ) -> PortFuture<Result<BudgetReservationReceipt, BudgetError>>;

    /// Reconcile one exact reservation identity.
    fn reconcile(
        &self,
        scope_id: BudgetScopeId,
        reservation_id: BudgetReservationId,
    ) -> PortFuture<Result<BudgetReservationState, BudgetError>>;

    /// Charge one committed effect usage idempotently.
    fn charge(
        &self,
        request: BudgetChargeRequest,
    ) -> PortFuture<Result<BudgetChargeReceipt, BudgetError>>;

    /// Release unused allowance after terminal commitment.
    fn release(
        &self,
        request: BudgetReleaseRequest,
    ) -> PortFuture<Result<BudgetReleaseReceipt, BudgetError>>;
}

/// Shared-budget service failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetError {
    /// Service unavailable or outcome cannot be known safely.
    #[error("{}: {message}", BUDGET_UNAVAILABLE)]
    Unavailable {
        /// Bounded diagnostic.
        message: Arc<str>,
    },
    /// Idempotency key reused with a different normalized request.
    #[error("{}: budget operation digest conflict", BUDGET_CONFLICT)]
    Conflict {
        /// Existing request digest.
        existing: Digest,
        /// Submitted request digest.
        submitted: Digest,
    },
    /// Backend explicitly reports an unsafe unknown outcome.
    #[error("{}: budget outcome unknown", BUDGET_UNKNOWN)]
    Unknown,
    /// Request or receipt violates the normalized contract.
    #[error("{}: {message}", BUDGET_INVALID_RECEIPT)]
    InvalidRequest {
        /// Stable diagnostic.
        message: Arc<str>,
    },
}

impl BudgetError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => BUDGET_UNAVAILABLE,
            Self::Conflict { .. } => BUDGET_CONFLICT,
            Self::Unknown => BUDGET_UNKNOWN,
            Self::InvalidRequest { .. } => BUDGET_INVALID_RECEIPT,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::{BudgetRequest, LimitKey};

    #[test]
    fn request_bounds_fail_closed() {
        let request = BudgetRequest {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cost: None,
            extension_counters: BTreeMap::new(),
        };
        let encoded = serde_json::to_vec(&request).expect("JSON");
        let decoded: BudgetRequest = serde_json::from_slice(&encoded).expect("request");
        assert_eq!(request, decoded);

        let mut oversized = BudgetRequest::default();
        for index in 0..=32 {
            let key =
                LimitKey::parse(format!("finstack.counter.{index}")).expect("namespaced counter");
            oversized.extension_counters.insert(key, 1);
        }
        assert!(oversized.validate().is_err());
    }
}
