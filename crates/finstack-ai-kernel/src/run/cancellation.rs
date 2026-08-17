//! Cancellation request and reconciliation records.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::primitives::{CancellationRequestId, EffectId, RunId};
use crate::refs::{AuthorizationEvidence, PrincipalRef, validated_label};

use super::error::RunError;

/// Authenticated source of a cancellation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CancellationInitiator {
    /// Explicit principal cancellation with persisted authorization evidence.
    Principal {
        /// Authenticated principal reference.
        principal: PrincipalRef,
        /// Exact authorization decision.
        authorization: AuthorizationEvidence,
    },
    /// Cancellation propagated from the accepted parent run.
    ParentRun {
        /// Parent run identity.
        parent_run_id: RunId,
    },
    /// Effective deadline expiry.
    Deadline,
    /// Runtime shutdown request.
    RuntimeShutdown,
}

/// Durable cancellation request identity and safe reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancellationRequest {
    /// Stable request identity.
    pub request_id: CancellationRequestId,
    /// Authenticated source.
    pub initiator: CancellationInitiator,
    /// Optional safe bounded reason label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Arc<str>>,
}

impl CancellationRequest {
    /// Construct a bounded cancellation request.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] when the optional reason is invalid.
    pub fn try_new(
        request_id: CancellationRequestId,
        initiator: CancellationInitiator,
        reason: Option<impl AsRef<str>>,
    ) -> Result<Self, RunError> {
        let reason = match reason {
            Some(value) => Some(validated_label(value.as_ref(), "cancellation_reason")?),
            None => None,
        };
        Ok(Self {
            request_id,
            initiator,
            reason,
        })
    }
}

impl<'de> Deserialize<'de> for CancellationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            request_id: CancellationRequestId,
            initiator: CancellationInitiator,
            #[serde(default)]
            reason: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.request_id,
            wire.initiator,
            wire.reason.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}

/// Durable cancellation-intent record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancellationRequested {
    /// Exact winning request.
    pub request: CancellationRequest,
}

/// Replay-derived cumulative cancellation reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancellationReconciled {
    /// Winning cancellation request.
    pub request_id: CancellationRequestId,
    /// Effects that completed before cancellation closure.
    pub completed_effects: Arc<[EffectId]>,
    /// Effects explicitly acknowledged cancelled.
    pub cancelled_effects: Arc<[EffectId]>,
    /// Effects whose external outcome remains uncertain.
    pub uncertain_effects: Arc<[EffectId]>,
}
