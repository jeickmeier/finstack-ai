use std::sync::Arc;

use finstack_ai_kernel::{
    CapabilityId, ContentBlock, LaneId, Message, RawJson, RunId, SEMANTIC_ARRAY_MAX_ITEMS,
    Sensitivity, SessionId,
};
use serde::{Deserialize, Serialize};

use super::error::{
    CONTEXT_CONFIGURATION_INVALID, CONTEXT_CONTRIBUTION_INVALID, ContextError, canonical_bytes,
    validate_label, validate_text,
};

/// Deterministic policy for a context-budget overrun.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextOverflowPolicy {
    /// Reject the contribution without changing it.
    Reject,
    /// Retain the highest-priority whole items and report every omitted item.
    TruncateWithDiagnostic,
}

/// Explicit item, token, and byte budget supplied to one context request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBudget {
    /// Maximum accepted item count.
    pub max_items: usize,
    /// Maximum accepted estimated input tokens.
    pub max_tokens: u64,
    /// Maximum accepted canonical item bytes.
    pub max_bytes: u64,
    /// Deterministic overrun behavior.
    pub overflow: ContextOverflowPolicy,
}

impl ContextBudget {
    pub(super) fn validate(self) -> Result<Self, ContextError> {
        if self.max_items > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "context item budget exceeds the semantic maximum",
            ));
        }
        Ok(self)
    }
}

/// Authority assigned to a provider contribution after descriptor policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextAuthority {
    /// Delimited data that cannot grant instruction authority.
    Untrusted,
    /// Application instruction explicitly authorized in the resolved descriptor.
    TrustedApplication,
}

/// Normalized context-item class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextItemKind {
    /// Application instruction, subject to descriptor authorization.
    Instruction,
    /// Quoted external content.
    QuotedSource,
    /// Reference to external content.
    Reference,
    /// Non-authoritative metadata.
    Metadata,
    /// Hidden application-owned context.
    HiddenApplicationContext,
    /// Derived, untrusted compaction summary.
    DerivedSummary,
}

/// Mandatory provenance for one context item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextProvenance {
    /// Stable source identifier safe to expose in diagnostics.
    pub source_id: Arc<str>,
    /// Optional source locator that contains no bearer credentials.
    pub source_ref: Option<Arc<str>>,
    /// Whether the content came from retrieval or another external source.
    pub external: bool,
}

impl ContextProvenance {
    fn validate(&self) -> Result<(), ContextError> {
        validate_label(&self.source_id, "context provenance source")?;
        if let Some(value) = &self.source_ref {
            validate_text(value, "context provenance reference")?;
        }
        Ok(())
    }
}

/// One typed, attributed, budgeted context item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextItem {
    /// Item class.
    pub kind: ContextItemKind,
    /// Normalized content blocks.
    pub content: Arc<[ContentBlock]>,
    /// Mandatory attribution.
    pub provenance: ContextProvenance,
    /// Authority after runtime normalization.
    pub authority: ContextAuthority,
    /// Higher values sort before lower values within one provider.
    pub priority: i32,
    /// Provider estimate used for deterministic budget accounting.
    pub estimated_tokens: u64,
    /// Canonical byte count of `content`.
    pub bytes: u64,
    /// Data sensitivity carried into model and observer policy.
    pub sensitivity: Sensitivity,
    /// Whether compaction is forbidden from removing this item.
    pub protected: bool,
}

impl ContextItem {
    /// Construct an item and compute its exact canonical content byte count.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for empty, invalid, or non-encodable content.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        kind: ContextItemKind,
        content: Vec<ContentBlock>,
        provenance: ContextProvenance,
        authority: ContextAuthority,
        priority: i32,
        estimated_tokens: u64,
        sensitivity: Sensitivity,
        protected: bool,
    ) -> Result<Self, ContextError> {
        let shared: Arc<[ContentBlock]> = content.into();
        let bytes = u64::try_from(canonical_bytes(&shared)?.len()).map_err(|_| {
            ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context item byte count overflowed",
            )
        })?;
        let item = Self {
            kind,
            content: shared,
            provenance,
            authority,
            priority,
            estimated_tokens,
            bytes,
            sensitivity,
            protected,
        };
        item.validate()?;
        Ok(item)
    }

    pub(super) fn validate(&self) -> Result<(), ContextError> {
        self.provenance.validate()?;
        if self.content.is_empty() {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context item content is empty",
            ));
        }
        let bytes = canonical_bytes(&self.content)?;
        if u64::try_from(bytes.len()).ok() != Some(self.bytes) {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context item byte estimate does not match canonical content",
            ));
        }
        if self.kind == ContextItemKind::DerivedSummary
            && self.authority != ContextAuthority::Untrusted
        {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "derived context cannot acquire instruction authority",
            ));
        }
        Ok(())
    }
}

/// Bounded request passed to one context provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequest {
    /// Session identity repeated in the committed provider input.
    pub session_id: SessionId,
    /// Lane identity repeated in the committed provider input.
    pub lane_id: LaneId,
    /// Run identity repeated in the committed provider input.
    pub run_id: RunId,
    /// Current user input blocks.
    pub user_input: Arc<[ContentBlock]>,
    /// Canonical recent history supplied by the runtime.
    pub recent_history: Arc<[Message]>,
    /// Provider-local budget.
    pub budget: ContextBudget,
    /// Active capabilities in locked activation order.
    pub active_capabilities: Arc<[CapabilityId]>,
}

impl ContextRequest {
    /// Canonical JSON bytes committed as `EffectInput::Context`.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error when the request is invalid or cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ContextError> {
        self.budget.validate()?;
        if self.user_input.len() > SEMANTIC_ARRAY_MAX_ITEMS
            || self.recent_history.len() > SEMANTIC_ARRAY_MAX_ITEMS
            || self.active_capabilities.len() > SEMANTIC_ARRAY_MAX_ITEMS
        {
            return Err(ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "context request exceeds a semantic array bound",
            ));
        }
        canonical_bytes(self)
    }

    /// Canonical raw JSON committed as the effect input.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error for invalid or non-encodable input.
    pub fn to_raw_json(&self) -> Result<RawJson, ContextError> {
        RawJson::parse(self.canonical_bytes()?).map_err(|_| {
            ContextError::stable(
                CONTEXT_CONFIGURATION_INVALID,
                "context request could not be normalized",
            )
        })
    }
}

/// Normalized provider output before committed assembly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextContribution {
    /// Source-ordered provider items.
    pub items: Arc<[ContextItem]>,
    /// Sum of item token estimates.
    pub estimated_tokens: u64,
    /// Sum of item canonical byte counts.
    pub bytes: u64,
    /// Optional bounded cache identity.
    pub cache_key: Option<Arc<str>>,
}

impl ContextContribution {
    /// Construct a contribution and compute exact aggregate item totals.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for invalid items or checked-sum overflow.
    pub fn try_new(
        items: Vec<ContextItem>,
        cache_key: Option<impl AsRef<str>>,
    ) -> Result<Self, ContextError> {
        let (tokens, bytes) = item_totals(&items)?;
        let contribution = Self {
            items: items.into(),
            estimated_tokens: tokens,
            bytes,
            cache_key: cache_key.map(|value| Arc::from(value.as_ref())),
        };
        contribution.validate()?;
        Ok(contribution)
    }

    pub(super) fn validate(&self) -> Result<(), ContextError> {
        if self.items.len() > SEMANTIC_ARRAY_MAX_ITEMS {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context contribution contains too many items",
            ));
        }
        if let Some(value) = &self.cache_key {
            validate_label(value, "context cache key")?;
        }
        let (tokens, bytes) = item_totals(&self.items)?;
        if tokens != self.estimated_tokens || bytes != self.bytes {
            return Err(ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context contribution totals do not match its items",
            ));
        }
        Ok(())
    }

    /// Convert the normalized contribution into canonical effect output.
    ///
    /// # Errors
    ///
    /// Returns a stable contribution error for invalid or non-encodable output.
    pub fn to_raw_json(&self) -> Result<RawJson, ContextError> {
        self.validate()?;
        RawJson::parse(canonical_bytes(self)?).map_err(|_| {
            ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context contribution could not be normalized",
            )
        })
    }
}

/// Validate every item and return its checked `(tokens, bytes)` totals.
fn item_totals(items: &[ContextItem]) -> Result<(u64, u64), ContextError> {
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for item in items {
        item.validate()?;
        tokens = tokens.checked_add(item.estimated_tokens).ok_or_else(|| {
            ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context token estimate overflowed",
            )
        })?;
        bytes = bytes.checked_add(item.bytes).ok_or_else(|| {
            ContextError::stable(
                CONTEXT_CONTRIBUTION_INVALID,
                "context byte estimate overflowed",
            )
        })?;
    }
    Ok((tokens, bytes))
}
