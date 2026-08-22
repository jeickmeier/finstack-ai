//! Idempotent non-kernel child-agent invocation service.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ports::model::AuthorizationContext;
use crate::ports::{PortFuture, PortObject};
use crate::{
    AgentId, BudgetRequest, BundleId, ChildPlacement, ChildRunLocator, ContentBlock, Digest,
    EffectId, Metadata, OperationLocator, Timestamp,
};

const CHILD_RUN_REQUEST_DIGEST_DOMAIN: &str = "child-run-request";
const CHILD_RUN_REQUEST_DIGEST_SCHEMA_VERSION: u32 = 2;

/// Host-owned child-run admission policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ChildRunPolicy {
    /// Reject every child invocation.
    #[default]
    Deny,
    /// Allow children up to the configured inclusive depth.
    Allow {
        /// Maximum child depth accepted by this agent.
        max_depth: u16,
    },
}

/// Stable code for an unavailable child-agent service.
pub const AGENT_INVOKE_UNAVAILABLE: &str = "agent_invoke_unavailable";
/// Stable code for conflicting reuse of a prepared child request.
pub const AGENT_INVOKE_CONFLICT: &str = "agent_invoke_conflict";
/// Stable code for an acceptance that differs from the prepared locator/relation.
pub const AGENT_INVOKE_INVALID_ACCEPTANCE: &str = "agent_invoke_invalid_acceptance";

/// Exact agent identity used by a prepared child request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRef {
    /// Agent identity.
    pub id: AgentId,
    /// Bundle supplying the agent, when bundle-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<BundleId>,
    /// Canonical resolved specification digest.
    pub spec_digest: Digest,
}

/// Parent authorization context for one prepared child invocation.
#[derive(Debug, Clone)]
pub struct ChildRunContext {
    /// Exact parent operation.
    pub parent: OperationLocator,
    /// Parent effect authorizing this child.
    pub parent_effect_id: EffectId,
    /// Authorization captured at dispatch.
    pub authorization: AuthorizationContext,
}

/// Immutable child request paired with a precommitted locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRunRequest {
    /// Exact locked agent.
    pub agent: AgentRef,
    /// Child input blocks.
    pub input: Arc<[ContentBlock]>,
    /// Selected placement.
    pub placement: ChildPlacement,
    /// Complete parent-prepared locator.
    pub locator: ChildRunLocator,
    /// Optional attenuated deadline.
    pub requested_deadline: Option<Timestamp>,
    /// Optional shared-budget request.
    pub requested_budget: BudgetRequest,
    /// Optional non-secret delegation reference.
    pub delegation_id: Option<Arc<str>>,
    /// Bounded non-authoritative metadata.
    pub metadata: Metadata,
    /// Canonical digest binding every normalized request field.
    pub request_digest: Digest,
}

impl ChildRunRequest {
    /// Compute the current canonical digest binding every normalized field.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error if canonical serialization fails.
    pub fn canonical_digest(&self) -> Result<Digest, AgentInvokeError> {
        let canonical = serde_json_canonicalizer::to_vec(&(
            &self.agent,
            &self.input,
            self.placement,
            &self.locator,
            self.requested_deadline,
            &self.requested_budget,
            &self.delegation_id,
            &self.metadata,
        ))
        .map_err(|_| AgentInvokeError::InvalidRequest {
            message: Arc::from("child request is not canonically serializable"),
        })?;
        Digest::domain_separated(
            CHILD_RUN_REQUEST_DIGEST_DOMAIN,
            CHILD_RUN_REQUEST_DIGEST_SCHEMA_VERSION,
            &canonical,
        )
        .map_err(|_| AgentInvokeError::InvalidRequest {
            message: Arc::from("child request digest domain is invalid"),
        })
    }

    fn legacy_sdk_digest(&self) -> Result<Digest, AgentInvokeError> {
        let canonical = serde_json_canonicalizer::to_vec(&(
            &self.agent.id,
            &self.agent.bundle,
            &self.agent.spec_digest,
            &self.input,
            self.placement,
            &self.locator,
            self.locator.operation.tenant_scope.as_ref(),
        ))
        .map_err(|_| AgentInvokeError::InvalidRequest {
            message: Arc::from("legacy child request is not canonically serializable"),
        })?;
        Digest::domain_separated(CHILD_RUN_REQUEST_DIGEST_DOMAIN, 1, &canonical).map_err(|_| {
            AgentInvokeError::InvalidRequest {
                message: Arc::from("legacy child request digest domain is invalid"),
            }
        })
    }

    /// Validate intrinsic placement and budget bounds.
    ///
    /// # Errors
    ///
    /// Returns a strict request error before calling an invoker.
    pub fn validate(&self) -> Result<(), AgentInvokeError> {
        self.locator.validate_for(self.placement).map_err(|error| {
            AgentInvokeError::InvalidRequest {
                message: Arc::from(error.to_string()),
            }
        })?;
        self.requested_budget
            .validate()
            .map_err(|error| AgentInvokeError::InvalidRequest {
                message: Arc::from(error.to_string()),
            })?;
        if self
            .delegation_id
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.as_bytes().contains(&0))
        {
            return Err(AgentInvokeError::InvalidRequest {
                message: Arc::from("invalid_delegation_id"),
            });
        }
        let current = self.canonical_digest()?;
        if self.request_digest != current {
            let legacy_fields_are_default = self.requested_deadline.is_none()
                && self.requested_budget == BudgetRequest::default()
                && self.delegation_id.is_none()
                && self.metadata == Metadata::empty();
            if !legacy_fields_are_default || self.request_digest != self.legacy_sdk_digest()? {
                return Err(AgentInvokeError::InvalidRequest {
                    message: Arc::from("child request digest does not bind normalized request"),
                });
            }
        }
        Ok(())
    }
}

/// Child acceptance returned by an idempotent start-or-attach handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildRunHandle {
    /// Exact accepted locator.
    pub locator: ChildRunLocator,
    /// Digest of the immutable parent/child relation.
    pub relation_digest: Digest,
}

/// Current host-observed lifecycle state for an accepted child run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildRunStatus {
    /// The host accepted the child but has not started execution yet.
    Accepted,
    /// The child is executing.
    Running,
    /// The child completed successfully.
    Completed,
    /// The child reached a terminal failure.
    Failed,
    /// Cancellation was durably requested but is not yet terminal.
    CancellationRequested,
    /// The child was cancelled.
    Cancelled,
}

impl ChildRunStatus {
    /// Stable model-facing status name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::CancellationRequested => "cancellation_requested",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether the status is terminal and no longer needs local tracking.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Runtime service that accepts or attaches to one exact precommitted child.
pub trait AgentInvoker: PortObject {
    /// Start the prepared child or attach to its existing equal acceptance.
    fn start_or_attach(
        &self,
        ctx: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>>;

    /// Query the current host-observed lifecycle state of an accepted child.
    ///
    /// The default fails closed so hosts that do not implement lifecycle
    /// observation cannot report a fabricated status.
    fn status(
        &self,
        locator: &ChildRunLocator,
    ) -> PortFuture<Result<ChildRunStatus, AgentInvokeError>> {
        let _ = locator;
        Box::pin(async {
            Err(AgentInvokeError::Unavailable {
                message: Arc::from("child status is not implemented"),
            })
        })
    }

    /// Submit one durable cancel for a previously accepted child.
    ///
    /// Isolated and compatible placements must submit
    /// `CancellationRequested` for `locator.operation.run_id`. Remote
    /// stays fail-closed at the toolset. The default implementation
    /// returns [`AgentInvokeError::Unavailable`] so a host that does not
    /// implement cancel cannot claim success.
    fn cancel(&self, locator: &ChildRunLocator) -> PortFuture<Result<(), AgentInvokeError>> {
        let _ = locator;
        Box::pin(async {
            Err(AgentInvokeError::Unavailable {
                message: Arc::from("child cancel is not implemented"),
            })
        })
    }
}

/// Child-agent service failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentInvokeError {
    /// Service unavailable or outcome cannot be known safely.
    #[error("{}: {message}", AGENT_INVOKE_UNAVAILABLE)]
    Unavailable {
        /// Bounded diagnostic.
        message: Arc<str>,
    },
    /// The same prepared identity was reused with a different request digest.
    #[error("{}: prepared request digest conflict", AGENT_INVOKE_CONFLICT)]
    Conflict {
        /// Previously accepted digest.
        existing: Digest,
        /// Submitted digest.
        submitted: Digest,
    },
    /// Request or returned acceptance is incomplete or inconsistent.
    #[error("{}: {message}", AGENT_INVOKE_INVALID_ACCEPTANCE)]
    InvalidRequest {
        /// Stable diagnostic.
        message: Arc<str>,
    },
}

impl AgentInvokeError {
    /// Stable machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { .. } => AGENT_INVOKE_UNAVAILABLE,
            Self::Conflict { .. } => AGENT_INVOKE_CONFLICT,
            Self::InvalidRequest { .. } => AGENT_INVOKE_INVALID_ACCEPTANCE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ComponentId, ComponentRef, ExternalHandleRef, LaneId, RemoteRouteRef, RunId, SessionId,
        Version,
    };

    fn operation() -> OperationLocator {
        OperationLocator {
            tenant_scope: Arc::from("tenant-a"),
            session_id: SessionId::from_bytes([1; 16]),
            lane_id: LaneId::from_bytes([2; 16]),
            run_id: RunId::from_bytes([3; 16]),
        }
    }

    #[test]
    fn placement_requires_exact_remote_route_shape() {
        let local = ChildRunLocator {
            operation: operation(),
            remote: None,
        };
        local
            .validate_for(ChildPlacement::CompatibleLaneInParentSession)
            .expect("local locator");
        assert!(
            local
                .validate_for(ChildPlacement::RemoteChildSession)
                .is_err()
        );

        let remote = ChildRunLocator {
            operation: operation(),
            remote: Some(RemoteRouteRef {
                service: ComponentRef::new(
                    ComponentId::parse("finstack.remote.worker").expect("component"),
                    Some(Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    }),
                ),
                route: ExternalHandleRef::try_new(
                    ComponentId::parse("finstack.remote.worker").expect("provider"),
                    "route-1",
                    crate::RawJson::parse(br#"{"cluster":"a"}"#).expect("metadata"),
                )
                .expect("route"),
            }),
        };
        remote
            .validate_for(ChildPlacement::RemoteChildSession)
            .expect("remote locator");
        assert!(
            remote
                .validate_for(ChildPlacement::IsolatedChildSession)
                .is_err()
        );
    }
}
