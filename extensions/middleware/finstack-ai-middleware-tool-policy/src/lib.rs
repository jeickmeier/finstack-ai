//! Tool policy filter middleware: policy-narrows the model-visible tool set.

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
    ComponentId, ComponentInvocation, Digest, ErrorCategory, ErrorDescriptor, InvocationRecovery,
    Metadata, Stage, Version,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::middleware::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRole, OrderTier, StageInput, StageMask, StageOutcome,
};

mod config;
mod eval;

pub use config::{JailbreakAction, ToolPolicyConfig, ToolPolicyError};

use eval::{PolicyVerdict, evaluate_before_model, evaluate_before_tool_batch};

const TOOL_POLICY_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Stable code for a `before_model` jailbreak trigger fail outcome.
pub const TOOL_POLICY_JAILBREAK_TRIGGERED: &str = "tool_policy_jailbreak_triggered";
/// Stable code when a batch would exceed the committed write-call budget.
pub const TOOL_POLICY_WRITE_BUDGET_EXCEEDED: &str = "tool_policy_write_budget_exceeded";

/// Policy filter middleware that narrows the model-visible tool set at
/// `before_model` and `before_tool_batch`.
#[derive(Debug, Clone)]
pub struct ToolPolicyMiddleware {
    descriptor: MiddlewareDescriptor,
    config: ToolPolicyConfig,
}

impl ToolPolicyMiddleware {
    /// Construct a tool-policy leaf from a validated configuration.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity, an unencodable configuration,
    /// or a configuration with zero rules (a no-op policy).
    pub fn try_new(config: ToolPolicyConfig) -> Result<Self, ToolPolicyError> {
        if config.is_empty() {
            return Err(ToolPolicyError::Configuration {
                reason: "empty_policy",
            });
        }
        let configuration_digest =
            Digest::raw_json(&serde_json::to_vec(&config).map_err(|_| {
                ToolPolicyError::Configuration {
                    reason: "invalid_configuration_encoding",
                }
            })?);
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.middleware.tool-policy").map_err(
                        |_| ToolPolicyError::Configuration {
                            reason: "invalid_component_id",
                        },
                    )?,
                    version: TOOL_POLICY_VERSION,
                    configuration_digest,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages: StageMask::from_stages([Stage::BeforeModel, Stage::BeforeToolBatch]),
                order: MiddlewareOrder {
                    tier: OrderTier::RequestShaping,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            config,
        })
    }
}

impl Middleware for ToolPolicyMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let config = self.config.clone();
        Box::pin(async move {
            match input {
                StageInput::BeforeModel(before_model) => verdict_to_outcome(evaluate_before_model(
                    &config,
                    &before_model,
                    &ctx.run.authorization.roles,
                    ctx.run.relation_depth,
                )),
                StageInput::BeforeToolBatch(input) => {
                    verdict_to_outcome(evaluate_before_tool_batch(
                        &config,
                        &input,
                        &ctx.run.authorization.roles,
                        ctx.run.relation_depth,
                    ))
                }
                _ => Err(MiddlewareError::try_new(
                    finstack_ai_runtime::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                    ErrorCategory::Middleware,
                    "tool-policy only runs at before_model and before_tool_batch",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into)),
            }
        })
    }
}

/// Map a [`PolicyVerdict`] to the `Continue` / `FilterTools` / `Fail`
/// stage outcome shared by both filter stages.
fn verdict_to_outcome(verdict: PolicyVerdict) -> Result<StageOutcome, MiddlewareError> {
    match verdict {
        PolicyVerdict::Identity => Ok(StageOutcome::Continue),
        PolicyVerdict::Retain(tools) => Ok(StageOutcome::FilterTools(
            tools.into_iter().collect::<Vec<_>>().into(),
        )),
        PolicyVerdict::Fail { reason } => Ok(StageOutcome::Fail(Box::new(
            ErrorDescriptor::new(
                reason,
                if reason == TOOL_POLICY_WRITE_BUDGET_EXCEEDED {
                    "tool policy write-call budget would be exceeded"
                } else {
                    "tool policy jailbreak trigger matched"
                },
                ErrorCategory::Validation,
                false,
            )
            .map_err(|_| {
                MiddlewareError::try_new(
                    reason,
                    ErrorCategory::Validation,
                    "tool policy rejected the request",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into)
            })?,
        ))),
    }
}

#[cfg(test)]
mod tests;
