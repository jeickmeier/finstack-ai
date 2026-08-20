//! Tenant/policy instruction injection middleware (`prepare_context`).

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
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, ErrorCategory, InvocationRecovery,
    Metadata, Sensitivity, Stage, TextBlock, Version,
};
use finstack_ai_runtime::{
    ContextAuthority, ContextItem, ContextItemKind, ContextProvenance, Middleware,
    MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder, MiddlewareRole,
    OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
};
use serde::Serialize;
use thiserror::Error;

/// Maximum number of policy entries one middleware may inject.
pub const MAX_POLICY_ENTRIES: usize = 16;

const INSTRUCTIONS_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Instructions-leaf construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstructionsError {
    /// Configuration is malformed.
    #[error("instructions_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// One policy instruction entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyEntry {
    /// Stable label; becomes the item's provenance source id (`policy:{label}`).
    pub label: String,
    /// Instruction text injected as a System message.
    pub text: String,
}

/// Ordered policy instruction set frozen at construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyInstructionsConfig {
    /// Entries injected in order at `prepare_context`.
    pub entries: Vec<PolicyEntry>,
}

impl PolicyInstructionsConfig {
    /// Validate entry count and per-entry label/text.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration reason for empty, oversized, or blank input.
    pub fn validate(&self) -> Result<(), InstructionsError> {
        if self.entries.is_empty() {
            return Err(InstructionsError::Configuration {
                reason: "entries_empty",
            });
        }
        if self.entries.len() > MAX_POLICY_ENTRIES {
            return Err(InstructionsError::Configuration {
                reason: "entries_exceed_maximum",
            });
        }
        for entry in &self.entries {
            if entry.label.trim().is_empty() {
                return Err(InstructionsError::Configuration {
                    reason: "entry_label_empty",
                });
            }
            if entry.text.trim().is_empty() {
                return Err(InstructionsError::Configuration {
                    reason: "entry_text_empty",
                });
            }
        }
        Ok(())
    }
}

/// Pure `prepare_context` middleware that injects frozen policy instructions.
#[derive(Debug, Clone)]
pub struct InstructionsMiddleware {
    descriptor: MiddlewareDescriptor,
    items: Arc<[ContextItem]>,
}

impl InstructionsMiddleware {
    /// Construct the middleware, freezing one protected item per entry.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration reason for invalid entries or
    /// non-encodable content.
    // `config` is taken by value to match the public constructor contract;
    // the body only borrows it and does not need to retain ownership.
    #[allow(clippy::needless_pass_by_value)]
    pub fn try_new(config: PolicyInstructionsConfig) -> Result<Self, InstructionsError> {
        config.validate()?;
        let mut items = Vec::with_capacity(config.entries.len());
        for entry in &config.entries {
            let block = ContentBlock::Text(TextBlock::try_new(&entry.text).map_err(|_| {
                InstructionsError::Configuration {
                    reason: "entry_text_invalid",
                }
            })?);
            let estimated_tokens = u64::try_from(entry.text.len() / 4).unwrap_or(u64::MAX);
            let item = ContextItem::try_new(
                ContextItemKind::Instruction,
                vec![block],
                ContextProvenance {
                    source_id: Arc::from(format!("policy:{}", entry.label)),
                    source_ref: None,
                    external: false,
                },
                ContextAuthority::TrustedApplication,
                0,
                estimated_tokens,
                Sensitivity::Internal,
                true,
            )
            .map_err(|_| InstructionsError::Configuration {
                reason: "entry_item_invalid",
            })?;
            items.push(item);
        }
        let digest_bytes =
            serde_json::to_vec(&config).map_err(|_| InstructionsError::Configuration {
                reason: "configuration_not_serializable",
            })?;
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.middleware.instructions").map_err(
                        |_| InstructionsError::Configuration {
                            reason: "invalid_component_id",
                        },
                    )?,
                    version: INSTRUCTIONS_VERSION,
                    configuration_digest: Digest::raw_json(&digest_bytes),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages: StageMask::from_stages([Stage::PrepareContext]),
                order: MiddlewareOrder {
                    tier: OrderTier::Standard,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            items: items.into(),
        })
    }
}

impl Middleware for InstructionsMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let items = Arc::clone(&self.items);
        Box::pin(async move {
            if !matches!(input, StageInput::PrepareContext { .. }) {
                return Err(MiddlewareError::try_new(
                    finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                    ErrorCategory::Middleware,
                    "instructions only run at prepare_context",
                    Metadata::empty(),
                )
                .unwrap_or_else(Into::into));
            }
            Ok(StageOutcome::AddInstructions(items))
        })
    }
}

#[cfg(test)]
mod tests;
