//! Idempotent non-kernel child-agent invocation service.

use std::sync::Arc;

use finstack_ai_kernel::label_is_valid;

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
    agent: AgentRef,
    input: Arc<[ContentBlock]>,
    placement: ChildPlacement,
    locator: ChildRunLocator,
    requested_deadline: Option<Timestamp>,
    requested_budget: BudgetRequest,
    delegation_id: Option<Arc<str>>,
    metadata: Metadata,
    request_digest: Digest,
}

impl ChildRunRequest {
    /// Construct and validate a digest-bound child request.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error when placement, budget, delegation, or
    /// canonical digest construction fails.
    #[expect(
        clippy::too_many_arguments,
        reason = "the request digest binds these eight independent protocol fields"
    )]
    pub fn try_new(
        agent: AgentRef,
        input: Arc<[ContentBlock]>,
        placement: ChildPlacement,
        locator: ChildRunLocator,
        requested_deadline: Option<Timestamp>,
        requested_budget: BudgetRequest,
        delegation_id: Option<Arc<str>>,
        metadata: Metadata,
    ) -> Result<Self, AgentInvokeError> {
        Self::validate_fields(
            placement,
            &locator,
            &requested_budget,
            delegation_id.as_deref(),
        )?;
        let request_digest = Self::digest_fields(
            &agent,
            &input,
            placement,
            &locator,
            requested_deadline,
            &requested_budget,
            delegation_id.as_ref(),
            &metadata,
        )?;
        Ok(Self {
            agent,
            input,
            placement,
            locator,
            requested_deadline,
            requested_budget,
            delegation_id,
            metadata,
            request_digest,
        })
    }

    /// Borrow the exact locked agent.
    #[must_use]
    pub const fn agent(&self) -> &AgentRef {
        &self.agent
    }

    /// Borrow the child input blocks without copying their shared storage.
    #[must_use]
    pub const fn input(&self) -> &Arc<[ContentBlock]> {
        &self.input
    }

    /// Return the selected placement.
    #[must_use]
    pub const fn placement(&self) -> ChildPlacement {
        self.placement
    }

    /// Borrow the complete parent-prepared locator.
    #[must_use]
    pub const fn locator(&self) -> &ChildRunLocator {
        &self.locator
    }

    /// Return the optional attenuated deadline.
    #[must_use]
    pub const fn requested_deadline(&self) -> Option<Timestamp> {
        self.requested_deadline
    }

    /// Borrow the optional shared-budget request.
    #[must_use]
    pub const fn requested_budget(&self) -> &BudgetRequest {
        &self.requested_budget
    }

    /// Borrow the optional non-secret delegation reference.
    #[must_use]
    pub fn delegation_id(&self) -> Option<&str> {
        self.delegation_id.as_deref()
    }

    /// Borrow the bounded non-authoritative metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Return the canonical digest binding every normalized request field.
    #[must_use]
    pub const fn request_digest(&self) -> Digest {
        self.request_digest
    }

    /// Compute the current canonical digest binding every normalized field.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error if canonical serialization fails.
    pub fn canonical_digest(&self) -> Result<Digest, AgentInvokeError> {
        Self::digest_fields(
            &self.agent,
            &self.input,
            self.placement,
            &self.locator,
            self.requested_deadline,
            &self.requested_budget,
            self.delegation_id.as_ref(),
            &self.metadata,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the digest schema intentionally binds all request fields"
    )]
    fn digest_fields(
        agent: &AgentRef,
        input: &[ContentBlock],
        placement: ChildPlacement,
        locator: &ChildRunLocator,
        requested_deadline: Option<Timestamp>,
        requested_budget: &BudgetRequest,
        delegation_id: Option<&Arc<str>>,
        metadata: &Metadata,
    ) -> Result<Digest, AgentInvokeError> {
        let canonical = serde_json_canonicalizer::to_vec(&(
            agent,
            input,
            placement,
            locator,
            requested_deadline,
            requested_budget,
            delegation_id,
            metadata,
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

    fn validate_fields(
        placement: ChildPlacement,
        locator: &ChildRunLocator,
        requested_budget: &BudgetRequest,
        delegation_id: Option<&str>,
    ) -> Result<(), AgentInvokeError> {
        locator
            .validate_for(placement)
            .map_err(|error| AgentInvokeError::InvalidRequest {
                message: Arc::from(error.to_string()),
            })?;
        requested_budget
            .validate()
            .map_err(|error| AgentInvokeError::InvalidRequest {
                message: Arc::from(error.to_string()),
            })?;
        if delegation_id.is_some_and(|value| !label_is_valid(value)) {
            return Err(AgentInvokeError::InvalidRequest {
                message: Arc::from("invalid_delegation_id"),
            });
        }
        Ok(())
    }

    /// Validate placement, budget, delegation, and digest integrity.
    ///
    /// # Errors
    ///
    /// Returns a strict request error before calling an invoker.
    pub fn validate(&self) -> Result<(), AgentInvokeError> {
        Self::validate_fields(
            self.placement,
            &self.locator,
            &self.requested_budget,
            self.delegation_id.as_deref(),
        )?;
        if self.request_digest != self.canonical_digest()? {
            return Err(AgentInvokeError::InvalidRequest {
                message: Arc::from("child request digest does not bind normalized request"),
            });
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

    #[test]
    fn child_request_rejects_oversized_delegation_id() {
        let locator = ChildRunLocator {
            operation: operation(),
            remote: None,
        };
        let error = ChildRunRequest::try_new(
            AgentRef {
                id: crate::AgentId::parse("test.child").expect("agent"),
                bundle: None,
                spec_digest: Digest::raw_json(b"agent"),
            },
            Arc::<[ContentBlock]>::from([]),
            ChildPlacement::CompatibleLaneInParentSession,
            locator,
            None,
            BudgetRequest::default(),
            Some(Arc::from(
                "x".repeat(finstack_ai_kernel::LABEL_MAX_BYTES + 1),
            )),
            Metadata::empty(),
        )
        .expect_err("oversized delegation id");
        assert!(error.to_string().contains("invalid_delegation_id"));
    }
}
