//! Idempotent non-kernel child-agent invocation service.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AgentId, AuthorizationContext, BudgetRequest, BundleId, ChildPlacement, ChildRunLocator,
    ContentBlock, Digest, EffectId, Metadata, OperationLocator, PortFuture, PortObject, Timestamp,
};

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

/// Runtime service that accepts or attaches to one exact precommitted child.
pub trait AgentInvoker: PortObject {
    /// Start the prepared child or attach to its existing equal acceptance.
    fn start_or_attach(
        &self,
        ctx: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>>;
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
