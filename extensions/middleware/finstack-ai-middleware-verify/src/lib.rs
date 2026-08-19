//! Fixture `before_finalize` verification middleware.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, Digest, ErrorCategory, InteractionKind, InteractionRequest,
    InvocationRecovery, Metadata, RawJson, Stage, Version,
};
use finstack_ai_kernel::{ErrorDescriptor, InteractionId};
use finstack_ai_runtime::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRole, OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
};
use thiserror::Error;

const VERIFY_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Verification decision selected at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyDecision {
    /// Accept the candidate terminal value.
    Accept,
    /// Fail the run with a stable descriptor.
    Fail,
    /// Request an approval interaction.
    RequestInteraction,
}

/// Verify-leaf construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VerifyError {
    /// Configuration is malformed.
    #[error("verify_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Fixture `before_finalize` verifier. It never writes a store.
#[derive(Debug, Clone)]
pub struct VerifyMiddleware {
    descriptor: MiddlewareDescriptor,
    decision: VerifyDecision,
}

impl VerifyMiddleware {
    /// Construct a verifier that accepts the candidate.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity.
    pub fn try_accept() -> Result<Self, VerifyError> {
        Self::try_new(VerifyDecision::Accept)
    }

    /// Construct a verifier with an explicit decision.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity.
    pub fn try_new(decision: VerifyDecision) -> Result<Self, VerifyError> {
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.middleware.verify").map_err(|_| {
                        VerifyError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: VERIFY_VERSION,
                    configuration_digest: Digest::raw_json(match decision {
                        VerifyDecision::Accept => b"accept",
                        VerifyDecision::Fail => b"fail",
                        VerifyDecision::RequestInteraction => b"interact",
                    }),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages: StageMask::from_stages([Stage::BeforeFinalize]),
                order: MiddlewareOrder {
                    tier: OrderTier::Standard,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            decision,
        })
    }
}

impl Middleware for VerifyMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let decision = self.decision;
        Box::pin(async move {
            if !matches!(input, StageInput::BeforeFinalize { .. }) {
                return Err(MiddlewareError::try_new(
                    finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                    ErrorCategory::Middleware,
                    "verify only runs at before_finalize",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into));
            }
            match decision {
                VerifyDecision::Accept => Ok(StageOutcome::Continue),
                VerifyDecision::Fail => Ok(StageOutcome::Fail(Box::new(
                    ErrorDescriptor::new(
                        "verify_rejected",
                        "verifier rejected the candidate",
                        ErrorCategory::Validation,
                        false,
                    )
                    .map_err(|_| {
                        MiddlewareError::try_new(
                            "verify_rejected",
                            ErrorCategory::Validation,
                            "verifier rejected the candidate",
                            Metadata::empty(),
                        )
                        .unwrap_or_else(Into::into)
                    })?,
                ))),
                VerifyDecision::RequestInteraction => {
                    let request = InteractionRequest::try_new(
                        1,
                        InteractionId::from_bytes(ctx.run.effect_id.to_bytes()),
                        ctx.run.effect_id,
                        InteractionKind::Approval,
                        vec![finstack_ai_kernel::ContentBlock::Text(
                            finstack_ai_kernel::TextBlock::try_new("approve candidate")
                                .map_err(|_| interaction_error())?,
                        )],
                        RawJson::parse(b"{}").map_err(|_| interaction_error())?,
                        finstack_ai_kernel::ComponentRef::new(
                            ComponentId::parse("finstack.middleware.verify")
                                .map_err(|_| interaction_error())?,
                            Some(VERIFY_VERSION),
                        ),
                        VERIFY_VERSION,
                        None,
                        None,
                        false,
                        Metadata::empty(),
                    )
                    .map_err(|_| interaction_error())?;
                    Ok(StageOutcome::RequestInteraction(Box::new(request)))
                }
            }
        })
    }
}

fn interaction_error() -> MiddlewareError {
    MiddlewareError::try_new(
        finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
        ErrorCategory::Middleware,
        "verify interaction request is invalid",
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
