//! Durable shared-budget request, receipt, and journal value types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, de};
use thiserror::Error;

use crate::{
    BudgetReservationId, BudgetScopeId, CostLimit, Digest, EffectId, LimitKey, RunId, Usage,
};

const MAX_EXTENSION_COUNTERS: usize = 32;

/// Optional shared-budget amount for a run or child reservation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetRequest {
    /// Input-token allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Output-token allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Exact cost allowance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostLimit>,
    /// Registered extension-counter allowances.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extension_counters: BTreeMap<LimitKey, u64>,
}

impl BudgetRequest {
    /// Validate the frozen extension-counter ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`BudgetRecordError::TooManyCounters`] rather than truncating.
    pub fn validate(&self) -> Result<(), BudgetRecordError> {
        if self.extension_counters.len() > MAX_EXTENSION_COUNTERS {
            return Err(BudgetRecordError::TooManyCounters {
                len: self.extension_counters.len(),
                max: MAX_EXTENSION_COUNTERS,
            });
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for BudgetRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            input_tokens: Option<u64>,
            #[serde(default)]
            output_tokens: Option<u64>,
            #[serde(default)]
            cost: Option<CostLimit>,
            #[serde(default)]
            extension_counters: BTreeMap<LimitKey, u64>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            input_tokens: wire.input_tokens,
            output_tokens: wire.output_tokens,
            cost: wire.cost,
            extension_counters: wire.extension_counters,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

/// One idempotent reservation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReserveRequest {
    /// Shared budget scope.
    pub scope_id: BudgetScopeId,
    /// Stable reservation identity.
    pub reservation_id: BudgetReservationId,
    /// Run consuming the reservation.
    pub run_id: RunId,
    /// Requested allowance.
    pub amount: BudgetRequest,
    /// Canonical normalized request digest.
    pub request_digest: Digest,
}

impl BudgetReserveRequest {
    /// Validate the amount and canonical request digest.
    ///
    /// # Errors
    ///
    /// Returns a stable durable-record error on mismatch.
    pub fn validate(&self) -> Result<(), BudgetRecordError> {
        self.amount.validate()?;
        let canonical = serde_json_canonicalizer::to_vec(&(
            self.scope_id,
            self.reservation_id,
            self.run_id,
            &self.amount,
        ))
        .map_err(|_| BudgetRecordError::Serialize)?;
        let expected = Digest::domain_separated("budget-reserve-request", 1, &canonical)
            .map_err(|_| BudgetRecordError::Serialize)?;
        if expected != self.request_digest {
            return Err(BudgetRecordError::DigestMismatch {
                field: "request_digest",
            });
        }
        Ok(())
    }

    /// Compute the request digest for normalized fields.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if canonical encoding fails.
    pub fn compute_digest(
        scope_id: BudgetScopeId,
        reservation_id: BudgetReservationId,
        run_id: RunId,
        amount: &BudgetRequest,
    ) -> Result<Digest, BudgetRecordError> {
        amount.validate()?;
        let canonical =
            serde_json_canonicalizer::to_vec(&(scope_id, reservation_id, run_id, amount))
                .map_err(|_| BudgetRecordError::Serialize)?;
        Digest::domain_separated("budget-reserve-request", 1, &canonical)
            .map_err(|_| BudgetRecordError::Serialize)
    }
}

/// Reservation receipt returned exactly on equal retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReservationReceipt {
    /// Shared budget scope.
    pub scope_id: BudgetScopeId,
    /// Stable reservation identity.
    pub reservation_id: BudgetReservationId,
    /// Exact reserved allowance.
    pub reserved: BudgetRequest,
    /// Remaining allowance after reservation.
    pub remaining: BudgetRequest,
    /// Bound request digest.
    pub request_digest: Digest,
    /// Canonical receipt digest supplied by the ledger.
    pub receipt_digest: Digest,
}

impl BudgetReservationReceipt {
    /// Validate bounded receipt values.
    ///
    /// # Errors
    ///
    /// Returns a durable-record error for malformed budget maps.
    pub fn validate(&self) -> Result<(), BudgetRecordError> {
        self.reserved.validate()?;
        self.remaining.validate()
    }
}

/// One idempotent usage charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetChargeRequest {
    /// Shared budget scope.
    pub scope_id: BudgetScopeId,
    /// Settled reservation identity.
    pub reservation_id: BudgetReservationId,
    /// Effect whose committed usage is charged.
    pub effect_id: EffectId,
    /// Normalized committed usage.
    pub usage: Usage,
    /// Canonical effect-output usage digest.
    pub usage_digest: Digest,
}

impl BudgetChargeRequest {
    /// Validate normalized usage and its digest.
    ///
    /// # Errors
    ///
    /// Returns a durable-record error on malformed usage or digest mismatch.
    pub fn validate(&self) -> Result<(), BudgetRecordError> {
        self.usage
            .validate()
            .map_err(|_| BudgetRecordError::InvalidUsage)?;
        let canonical = self
            .usage
            .canonical_bytes()
            .map_err(|_| BudgetRecordError::Serialize)?;
        if Digest::effect_output(&canonical) != self.usage_digest {
            return Err(BudgetRecordError::DigestMismatch {
                field: "usage_digest",
            });
        }
        Ok(())
    }
}

/// Usage-charge receipt returned exactly on equal retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetChargeReceipt {
    /// Shared budget scope.
    pub scope_id: BudgetScopeId,
    /// Settled reservation identity.
    pub reservation_id: BudgetReservationId,
    /// Charged effect.
    pub effect_id: EffectId,
    /// Usage charged by this operation.
    pub charged_usage: Usage,
    /// Cumulative reservation usage.
    pub cumulative_usage: Usage,
    /// Bound usage digest.
    pub usage_digest: Digest,
    /// Canonical receipt digest supplied by the ledger.
    pub receipt_digest: Digest,
}

impl BudgetChargeReceipt {
    /// Validate bounded usage values.
    ///
    /// # Errors
    ///
    /// Returns a durable-record error for malformed usage maps.
    pub fn validate(&self) -> Result<(), BudgetRecordError> {
        self.charged_usage
            .validate()
            .map_err(|_| BudgetRecordError::InvalidUsage)?;
        self.cumulative_usage
            .validate()
            .map_err(|_| BudgetRecordError::InvalidUsage)?;
        let canonical = self
            .charged_usage
            .canonical_bytes()
            .map_err(|_| BudgetRecordError::Serialize)?;
        if Digest::effect_output(&canonical) != self.usage_digest {
            return Err(BudgetRecordError::DigestMismatch {
                field: "usage_digest",
            });
        }
        Ok(())
    }
}

/// One idempotent release request after terminal commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReleaseRequest {
    /// Shared budget scope.
    pub scope_id: BudgetScopeId,
    /// Settled reservation identity.
    pub reservation_id: BudgetReservationId,
    /// Terminal run whose unused allowance is released.
    pub terminal_run_id: RunId,
    /// Canonical normalized release digest.
    pub request_digest: Digest,
}

impl BudgetReleaseRequest {
    /// Validate the canonical release digest.
    ///
    /// # Errors
    ///
    /// Returns a stable durable-record error when the digest does not bind the
    /// exact scope, reservation, and terminal run.
    pub fn validate(&self) -> Result<(), BudgetRecordError> {
        let expected =
            Self::compute_digest(self.scope_id, self.reservation_id, self.terminal_run_id)?;
        if expected != self.request_digest {
            return Err(BudgetRecordError::DigestMismatch {
                field: "request_digest",
            });
        }
        Ok(())
    }

    /// Compute the normalized release-request digest.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if canonical encoding fails.
    pub fn compute_digest(
        scope_id: BudgetScopeId,
        reservation_id: BudgetReservationId,
        terminal_run_id: RunId,
    ) -> Result<Digest, BudgetRecordError> {
        let canonical =
            serde_json_canonicalizer::to_vec(&(scope_id, reservation_id, terminal_run_id))
                .map_err(|_| BudgetRecordError::Serialize)?;
        Digest::domain_separated("budget-release-request", 1, &canonical)
            .map_err(|_| BudgetRecordError::Serialize)
    }
}

/// Release receipt returned exactly on equal retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReleaseReceipt {
    /// Shared budget scope.
    pub scope_id: BudgetScopeId,
    /// Settled reservation identity.
    pub reservation_id: BudgetReservationId,
    /// Terminal run.
    pub terminal_run_id: RunId,
    /// Unused allowance released by this operation.
    pub released_unused: BudgetRequest,
    /// Bound request digest.
    pub request_digest: Digest,
    /// Canonical receipt digest supplied by the ledger.
    pub receipt_digest: Digest,
}

impl BudgetReleaseReceipt {
    /// Validate bounded released values.
    ///
    /// # Errors
    ///
    /// Returns a durable-record error for malformed budget maps.
    pub fn validate(&self) -> Result<(), BudgetRecordError> {
        self.released_unused.validate()?;
        let expected = BudgetReleaseRequest::compute_digest(
            self.scope_id,
            self.reservation_id,
            self.terminal_run_id,
        )?;
        if expected != self.request_digest {
            return Err(BudgetRecordError::DigestMismatch {
                field: "request_digest",
            });
        }
        Ok(())
    }
}

/// Durable reservation intent committed with child preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReservationRequested {
    /// Exact reservation request.
    pub request: BudgetReserveRequest,
}

/// Durable reservation settlement committed before child acceptance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReservationSettled {
    /// Exact ledger receipt.
    pub receipt: BudgetReservationReceipt,
}

/// Durable charge receipt committed after an idempotent ledger charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetChargeRecorded {
    /// Exact ledger receipt.
    pub receipt: BudgetChargeReceipt,
}

/// Durable release receipt; prior charges remain authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReservationReleased {
    /// Exact ledger receipt.
    pub receipt: BudgetReleaseReceipt,
}

/// Durable shared-budget value validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetRecordError {
    /// Too many extension counters.
    #[error("budget request has {len} extension counters; maximum is {max}")]
    TooManyCounters {
        /// Observed count.
        len: usize,
        /// Maximum count.
        max: usize,
    },
    /// Digest does not match its normalized fields.
    #[error("{field} does not match normalized budget fields")]
    DigestMismatch {
        /// Mismatched digest field.
        field: &'static str,
    },
    /// Usage is invalid.
    #[error("budget usage is invalid")]
    InvalidUsage,
    /// Canonical encoding failed.
    #[error("budget canonical serialization failed")]
    Serialize,
}
