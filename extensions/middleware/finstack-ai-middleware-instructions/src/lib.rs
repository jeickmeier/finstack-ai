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
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::context::{
    ContextAuthority, ContextItem, ContextItemKind, ContextProvenance,
};
use finstack_ai_runtime::ports::middleware::{
    Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError, MiddlewareOrder,
    MiddlewareRole, OrderTier, StageInput, StageMask, StageOutcome,
};
use serde::Serialize;
use thiserror::Error;

/// Maximum number of policy entries one middleware may inject.
pub const MAX_POLICY_ENTRIES: usize = 16;

/// Longest permitted `PolicyEntry::label`, in bytes.
///
/// The label becomes the item's provenance source id as `policy:{label}`, and
/// the runtime caps source ids at 256 bytes; `POLICY_SOURCE_ID_PREFIX`
/// occupies 7 of them. A test pins the `prefix + label == 256` relationship.
pub const MAX_POLICY_LABEL_BYTES: usize = 249;

/// Prefix prepended to every entry label to form the provenance source id.
const POLICY_SOURCE_ID_PREFIX: &str = "policy:";

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
    /// Construction ([`InstructionsMiddleware::try_new`]) is the sole public
    /// validation path; this helper is crate-private.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration reason for empty, oversized, or blank input.
    fn validate(&self) -> Result<(), InstructionsError> {
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
            // The runtime caps provenance source ids at 256 bytes; the label
            // lands there as `policy:{label}`, so reject early with a stable
            // reason instead of failing item construction downstream.
            if entry.label.len() > MAX_POLICY_LABEL_BYTES {
                return Err(InstructionsError::Configuration {
                    reason: "entry_label_too_long",
                });
            }
            if entry.label.as_bytes().contains(&0) {
                return Err(InstructionsError::Configuration {
                    reason: "entry_label_invalid",
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
    /// This is the sole public validation path for
    /// [`PolicyInstructionsConfig`].
    ///
    /// # Errors
    ///
    /// Returns a stable configuration reason for invalid entries or
    /// non-encodable content.
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
            // Provider-neutral fallback: round up by four UTF-8 bytes per
            // token and never report zero for emitted content.
            let estimated_tokens = u64::try_from(entry.text.len())
                .unwrap_or(u64::MAX)
                .div_ceil(4)
                .max(1);
            let item = ContextItem::try_new(
                ContextItemKind::Instruction,
                vec![block],
                ContextProvenance {
                    source_id: Arc::from(format!("{POLICY_SOURCE_ID_PREFIX}{}", entry.label)),
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
                    finstack_ai_runtime::ports::middleware::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
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
