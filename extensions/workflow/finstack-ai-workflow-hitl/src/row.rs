//! Inbox row and lifecycle status for one HITL interaction.

use std::sync::Arc;

use finstack_ai_kernel::{LaneId, RunId, SessionId, Timestamp};

use crate::error::HitlError;

/// Lifecycle of an inbox row. Canonical tokens: `"open"` | `"delivered"` |
/// `"expired"` | `"closed"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionStatus {
    /// Awaiting resolution; visible to callers polling the inbox.
    Open,
    /// Resolved and handed off to the worker for consumption.
    Delivered,
    /// Passed its deadline without resolution.
    Expired,
    /// Terminal: no further transitions expected.
    Closed,
}

impl InteractionStatus {
    /// Stable lowercase wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Delivered => "delivered",
            Self::Expired => "expired",
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
            "delivered" => Ok(Self::Delivered),
            "expired" => Ok(Self::Expired),
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
    /// Current lifecycle status.
    pub status: InteractionStatus,
    /// Subject of the resolving principal, once delivered/expired.
    pub resolved_by: Option<Arc<str>>,
    /// When the row was last updated.
    pub updated_at: Timestamp,
}
