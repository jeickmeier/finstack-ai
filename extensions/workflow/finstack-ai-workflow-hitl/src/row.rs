//! Inbox row and lifecycle status for one HITL interaction.

use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, LaneId, PrincipalRef, RunId, SessionId, Timestamp,
};

use crate::error::HitlError;

/// Lifecycle of an inbox row. Canonical tokens: `"open"`, `"buffered"`,
/// `"accepted"`, `"rejected"`, and `"closed"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionStatus {
    /// Awaiting resolution; visible to callers polling the inbox.
    Open,
    /// Resolution durably buffered for worker consumption.
    Buffered,
    /// Resolution admitted by the runtime interaction ingress.
    Accepted,
    /// Resolution durably rejected by the runtime interaction ingress.
    Rejected,
    /// Terminal: no further transitions expected.
    Closed,
}

impl InteractionStatus {
    /// Stable lowercase wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Buffered => "buffered",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Closed => "closed",
        }
    }

    /// Parse the wire representation produced by [`InteractionStatus::as_str`].
    ///
    /// # Errors
    ///
    /// Returns [`HitlError::StoreIntegrity`] with code `"interaction_status"`
    /// for any unrecognized value.
    pub fn parse(value: &str) -> Result<Self, HitlError> {
        match value {
            "open" => Ok(Self::Open),
            "buffered" => Ok(Self::Buffered),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "closed" => Ok(Self::Closed),
            _ => Err(HitlError::StoreIntegrity {
                code: "interaction_status",
            }),
        }
    }
}

/// One pending or settled interaction, keyed by `(tenant_scope, interaction_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionRow {
    /// Tenant that owns the interaction.
    pub tenant_scope: Arc<str>,
    /// Session the interaction was requested from.
    pub session_id: SessionId,
    /// Lane the session runs in.
    pub lane_id: LaneId,
    /// Run the session belongs to.
    pub run_id: RunId,
    /// Canonical `InteractionId` string; equals the wake row's `pending_id`.
    pub interaction_id: Arc<str>,
    /// Interaction kind token (e.g. `"approval"`) for filtering without
    /// deserializing.
    pub kind: Arc<str>,
    /// When the interaction was requested.
    pub requested_at: Timestamp,
    /// Deadline after which the interaction is considered expired, if any.
    pub expires_at: Option<Timestamp>,
    /// `serde_json` bytes of the committed `InteractionRequest` envelope.
    pub request: Arc<[u8]>,
    /// Exact principal captured from the run's accepted security context.
    pub accepted_principal: PrincipalRef,
    /// Exact authorization evidence captured from the accepted run.
    pub accepted_evidence: AuthorizationEvidence,
    /// Current lifecycle status.
    pub status: InteractionStatus,
    /// Subject of the resolving principal once a response is buffered.
    pub resolved_by: Option<Arc<str>>,
    /// Stable ingress outcome or reconciliation code, when settled.
    pub outcome_code: Option<Arc<str>>,
    /// When the row was last updated.
    pub updated_at: Timestamp,
}

/// Identity-and-lifecycle projection of an [`InteractionRow`], without the
/// serialized request payload — everything a sweep's reconcile pass needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionSummary {
    /// Tenant that owns the interaction.
    pub tenant_scope: Arc<str>,
    /// Canonical `InteractionId` string; equals the wake row's `pending_id`.
    pub interaction_id: Arc<str>,
    /// Current row lifecycle state.
    pub status: InteractionStatus,
    /// Subject of the resolving principal once a response is buffered.
    pub resolved_by: Option<Arc<str>>,
    /// Stable ingress outcome or reconciliation code, when settled.
    pub outcome_code: Option<Arc<str>>,
}
