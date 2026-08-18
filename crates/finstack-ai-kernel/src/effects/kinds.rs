//! Effect kind, input, and output-contract types.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, ToolCallBlock};
use crate::primitives::Digest;
use crate::primitives::RawJson;
use crate::primitives::Timestamp;
use crate::primitives::{ComponentId, EffectId, EffectOutputKey, InteractionId};
use crate::primitives::{Version, validated_text};

use super::EffectError;

/// Effect kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Model call.
    Model,
    /// Tool call.
    Tool,
    /// Context provider.
    Context,
    /// Middleware stage.
    Middleware,
    /// Interaction.
    Interaction,
    /// Timer.
    Timer,
}

/// Effect execution retry safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrySafety {
    /// Safe to retry.
    SafeToRetry,
    /// Idempotent when keyed.
    IdempotentWithKey,
    /// At most once.
    AtMostOnce,
    /// Unknown.
    Unknown,
}

/// Invocation recovery class for a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvocationRecovery {
    /// Recompute-safe.
    RecomputeSafe,
    /// Reconcile required.
    Reconcile,
    /// Non-repeatable.
    NonRepeatable,
}

/// Component invocation metadata on an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentInvocation {
    /// Component id.
    pub component: ComponentId,
    /// Component version.
    pub version: Version,
    /// Configuration digest.
    pub configuration_digest: Digest,
    /// Recovery class.
    pub recovery: InvocationRecovery,
}

/// Middleware pipeline position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PipelinePosition {
    chain_digest: Digest,
    stage: Arc<str>,
    index: u32,
}

impl PipelinePosition {
    /// Construct a pipeline position.
    ///
    /// # Arguments
    ///
    /// * `chain_digest` - Digest of the immutable middleware chain.
    /// * `stage` - Stage name label.
    /// * `index` - Zero-based index of this stage in the chain.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::Refs`] when `stage` is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{Digest, PipelinePosition};
    ///
    /// let position = PipelinePosition::try_new(Digest::raw_json(b"chain"), "before_model", 0)
    ///     .expect("position");
    /// assert_eq!(position.stage(), "before_model");
    /// assert_eq!(position.index(), 0);
    /// ```
    pub fn try_new(
        chain_digest: Digest,
        stage: impl AsRef<str>,
        index: u32,
    ) -> Result<Self, EffectError> {
        Ok(Self {
            chain_digest,
            stage: validated_text(stage.as_ref(), "stage")?,
            index,
        })
    }

    /// Middleware-chain digest.
    #[must_use]
    pub fn chain_digest(&self) -> Digest {
        self.chain_digest
    }

    /// Stage name.
    #[must_use]
    pub fn stage(&self) -> &str {
        &self.stage
    }

    /// Zero-based stage index.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl<'de> Deserialize<'de> for PipelinePosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            chain_digest: Digest,
            stage: BoundedString<{ crate::content::TEXT_MAX_BYTES }>,
            index: u32,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.chain_digest, wire.stage.into_inner(), wire.index)
            .map_err(de::Error::custom)
    }
}

/// Parent effect relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRelation {
    /// Parent effect id.
    pub parent_effect_id: EffectId,
    /// Purpose.
    pub purpose: EffectPurpose,
}

/// Why a nested model effect exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NestedModelKind {
    /// MCP `sampling/createMessage` owned by an open parent tool.
    McpSampling,
}

/// Why a child effect exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectPurpose {
    /// Compaction summary owned by middleware.
    CompactionSummary {
        /// Middleware component id.
        middleware_component_id: ComponentId,
    },
    /// Nested model owned by an open parent effect (ADR-046).
    NestedModel {
        /// Why the nested model exists. Wire name avoids the purpose tag.
        #[serde(rename = "nested_kind")]
        kind: NestedModelKind,
    },
}

/// Normalized effect input.
///
/// Serialized with externally tagged `snake_case` variants so `RawJson` members
/// deserialize through the ordinary human-readable path (internally tagged
/// enums buffer content and break `RawJson`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectInput {
    /// Model request JSON.
    Model {
        /// Request payload.
        request: RawJson,
    },
    /// Tool call block.
    Tool {
        /// Call.
        call: ToolCallBlock,
    },
    /// Context request JSON.
    Context {
        /// Request payload.
        request: RawJson,
    },
    /// Middleware stage input.
    Middleware {
        /// Stage name.
        stage: Arc<str>,
        /// Input payload.
        input: RawJson,
    },
    /// Interaction paired by digest.
    Interaction {
        /// Interaction id.
        interaction_id: InteractionId,
        /// Digest of the paired interaction request.
        request_digest: Digest,
    },
    /// Timer due-at.
    Timer {
        /// Due timestamp.
        due_at: Timestamp,
    },
}

impl<'de> Deserialize<'de> for EffectInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "snake_case")]
        enum Wire {
            Model {
                request: RawJson,
            },
            Tool {
                call: ToolCallBlock,
            },
            Context {
                request: RawJson,
            },
            Middleware {
                stage: BoundedString<{ crate::content::TEXT_MAX_BYTES }>,
                input: RawJson,
            },
            Interaction {
                interaction_id: InteractionId,
                request_digest: Digest,
            },
            Timer {
                due_at: Timestamp,
            },
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Model { request } => Self::Model { request },
            Wire::Tool { call } => Self::Tool { call },
            Wire::Context { request } => Self::Context { request },
            Wire::Middleware { stage, input } => Self::Middleware {
                stage: Arc::from(stage.into_inner()),
                input,
            },
            Wire::Interaction {
                interaction_id,
                request_digest,
            } => Self::Interaction {
                interaction_id,
                request_digest,
            },
            Wire::Timer { due_at } => Self::Timer { due_at },
        })
    }
}

impl EffectInput {
    /// Canonical bytes for `effect-input` digests.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, EffectError> {
        serde_json_canonicalizer::to_vec(self).map_err(|error| EffectError::Serialize {
            detail: error.to_string(),
        })
    }

    /// Digest under the `effect-input` domain.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when canonicalization fails.
    pub fn digest(&self) -> Result<Digest, EffectError> {
        Ok(Digest::effect_input(&self.canonical_bytes()?))
    }

    /// Matching [`EffectKind`].
    #[must_use]
    pub const fn kind(&self) -> EffectKind {
        match self {
            Self::Model { .. } => EffectKind::Model,
            Self::Tool { .. } => EffectKind::Tool,
            Self::Context { .. } => EffectKind::Context,
            Self::Middleware { .. } => EffectKind::Middleware,
            Self::Interaction { .. } => EffectKind::Interaction,
            Self::Timer { .. } => EffectKind::Timer,
        }
    }
}

/// Output kind for an effect contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOutputKind {
    /// Model response.
    ModelResponse,
    /// Tool result.
    ToolResult,
    /// Context contribution.
    ContextContribution,
    /// Middleware outcome.
    MiddlewareOutcome,
    /// Interaction resolution.
    InteractionResolution,
    /// Timer firing.
    TimerFiring,
    /// Artifact receipt.
    ArtifactReceipt,
    /// Custom namespaced kind.
    Custom {
        /// Custom key.
        key: EffectOutputKey,
    },
}

/// Non-optional output contract carried through deferral/completion.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectOutputContract {
    /// Output kind.
    pub kind: EffectOutputKind,
    /// Schema version.
    pub schema_version: u16,
    /// Schema digest.
    pub schema_digest: Digest,
}
