use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{Digest, LABEL_MAX_BYTES};
use serde::{Deserialize, Serialize};

use super::error::ModelError;
use super::identity::ModelName;
use super::{MODEL_PROFILE_INVALID, MODEL_PROFILE_OVERRIDE_NOT_ALLOWED, MODEL_PROFILE_RELAXATION};

/// Provider-neutral accepted input classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the frozen compatibility DTO is a set of independent input capabilities"
)]
pub struct InputCapabilities {
    /// Plain text messages are accepted.
    pub text: bool,
    /// Structured JSON blocks are accepted.
    pub json: bool,
    /// Image references are accepted.
    pub images: bool,
    /// Audio references are accepted.
    pub audio: bool,
    /// File references are accepted.
    pub files: bool,
}

/// Provider structured-output support level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputCapability {
    /// No structured-output support.
    Unsupported,
    /// Prompt-enforced structured output.
    Prompted,
    /// Provider-native structured output.
    Native,
}

/// Exact or conservative estimator source classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenEstimatorSource {
    /// Provider tokenizer implementation.
    ProviderTokenizer,
    /// Project-owned exact implementation.
    ProjectExact,
    /// Documented conservative upper bound.
    ConservativeUpperBound,
}

/// Versioned estimator identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenEstimatorRef {
    /// Stable estimator identifier.
    pub id: Arc<str>,
    /// Stable estimator version.
    pub version: Arc<str>,
    /// Exactness/source class.
    pub source: TokenEstimatorSource,
}

/// Provider context ceilings and safety margins for one model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelContextProfile {
    /// Resolved provider identity.
    pub provider: Arc<str>,
    /// Resolved provider model.
    pub model: ModelName,
    /// Maximum canonical request bytes.
    pub hard_input_bytes: u64,
    /// Total context window.
    pub context_window_tokens: u64,
    /// Provider maximum output tokens.
    pub max_output_tokens: u64,
    /// Reserved output safety margin.
    pub reserved_output_tokens: u64,
    /// Provider framing overhead margin.
    pub provider_overhead_tokens: u64,
    /// Bound estimator identity.
    pub estimator: TokenEstimatorRef,
}

/// Optional tightening overlay for one context profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelContextProfileOverride {
    /// Optional lower byte ceiling.
    pub hard_input_bytes: Option<u64>,
    /// Optional lower context ceiling.
    pub context_window_tokens: Option<u64>,
    /// Optional lower output ceiling.
    pub max_output_tokens: Option<u64>,
    /// Optional higher reserved-output margin.
    pub reserved_output_tokens: Option<u64>,
    /// Optional higher provider-overhead margin.
    pub provider_overhead_tokens: Option<u64>,
}

impl ModelContextProfileOverride {
    fn is_empty(&self) -> bool {
        self.hard_input_bytes.is_none()
            && self.context_window_tokens.is_none()
            && self.max_output_tokens.is_none()
            && self.reserved_output_tokens.is_none()
            && self.provider_overhead_tokens.is_none()
    }
}

/// Canonical locked model context profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedModelContextProfile {
    /// Effective immutable profile.
    pub profile: ModelContextProfile,
    /// Digest of its canonical JSON bytes.
    pub digest: Digest,
}

/// Provider capabilities for one named model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the frozen compatibility DTO exposes independent provider capabilities"
)]
pub struct ModelCapabilities {
    /// Supported input classes.
    pub input: InputCapabilities,
    /// Provider context profile.
    pub context_profile: ModelContextProfile,
    /// Native tool-call support.
    pub native_tool_calls: bool,
    /// Parallel tool-call support.
    pub parallel_tool_calls: bool,
    /// Structured-output support.
    pub structured_output: StructuredOutputCapability,
    /// Reasoning stream support.
    pub reasoning: bool,
    /// Prompt-cache support.
    pub prompt_cache: bool,
    /// Resumable stream support.
    pub resumable_stream: bool,
    /// Provider idempotency support.
    pub idempotent_requests: bool,
    /// Namespaced native capabilities.
    #[serde(default)]
    pub native_capabilities: BTreeSet<Arc<str>>,
}

/// Validate and lock one provider profile with tightening overlays.
///
/// # Errors
///
/// Rejects invalid profiles, relaxations, forbidden run overrides, and arithmetic overflow.
pub fn resolve_model_context_profile(
    provider: ModelContextProfile,
    agent: Option<&ModelContextProfileOverride>,
    run: Option<&ModelContextProfileOverride>,
    run_override_allowed: bool,
) -> Result<LockedModelContextProfile, ModelError> {
    validate_profile(&provider)?;
    let mut profile = provider;
    if let Some(overlay) = agent {
        apply_profile_override(&mut profile, overlay)?;
    }
    if let Some(overlay) = run {
        if !run_override_allowed && !overlay.is_empty() {
            return Err(ModelError::validation(
                MODEL_PROFILE_OVERRIDE_NOT_ALLOWED,
                "run model-profile override is not allowlisted",
            ));
        }
        apply_profile_override(&mut profile, overlay)?;
    }
    validate_profile(&profile)?;
    let bytes = serde_json_canonicalizer::to_vec(&profile).map_err(|_| {
        ModelError::validation(MODEL_PROFILE_INVALID, "model profile is not serializable")
    })?;
    Ok(LockedModelContextProfile {
        profile,
        digest: Digest::raw_json(&bytes),
    })
}

fn apply_profile_override(
    profile: &mut ModelContextProfile,
    overlay: &ModelContextProfileOverride,
) -> Result<(), ModelError> {
    tighten_ceiling(&mut profile.hard_input_bytes, overlay.hard_input_bytes)?;
    tighten_ceiling(
        &mut profile.context_window_tokens,
        overlay.context_window_tokens,
    )?;
    tighten_ceiling(&mut profile.max_output_tokens, overlay.max_output_tokens)?;
    tighten_margin(
        &mut profile.reserved_output_tokens,
        overlay.reserved_output_tokens,
    )?;
    tighten_margin(
        &mut profile.provider_overhead_tokens,
        overlay.provider_overhead_tokens,
    )?;
    validate_profile(profile)
}

fn tighten_ceiling(current: &mut u64, candidate: Option<u64>) -> Result<(), ModelError> {
    if let Some(candidate) = candidate {
        if candidate > *current {
            return Err(ModelError::validation(
                MODEL_PROFILE_RELAXATION,
                "model-profile ceiling override would relax the current profile",
            ));
        }
        *current = (*current).min(candidate);
    }
    Ok(())
}

fn tighten_margin(current: &mut u64, candidate: Option<u64>) -> Result<(), ModelError> {
    if let Some(candidate) = candidate {
        if candidate < *current {
            return Err(ModelError::validation(
                MODEL_PROFILE_RELAXATION,
                "model-profile margin override would relax the current profile",
            ));
        }
        *current = (*current).max(candidate);
    }
    Ok(())
}

fn validate_profile(profile: &ModelContextProfile) -> Result<(), ModelError> {
    if profile.provider.is_empty()
        || profile.provider.len() > LABEL_MAX_BYTES
        || profile.provider.as_bytes().contains(&0)
        || profile.hard_input_bytes == 0
        || profile.context_window_tokens == 0
        || profile.max_output_tokens == 0
        || profile.estimator.id.is_empty()
        || profile.estimator.version.is_empty()
        || profile.estimator.id.len() > LABEL_MAX_BYTES
        || profile.estimator.version.len() > LABEL_MAX_BYTES
        || profile.estimator.id.as_bytes().contains(&0)
        || profile.estimator.version.as_bytes().contains(&0)
    {
        return Err(ModelError::validation(
            MODEL_PROFILE_INVALID,
            "model profile requires non-zero ceilings and estimator identity",
        ));
    }
    let margins = profile
        .reserved_output_tokens
        .checked_add(profile.provider_overhead_tokens)
        .ok_or_else(|| {
            ModelError::validation(MODEL_PROFILE_INVALID, "model profile margin overflow")
        })?;
    if margins > profile.context_window_tokens {
        return Err(ModelError::validation(
            MODEL_PROFILE_INVALID,
            "model profile margins exceed the context window",
        ));
    }
    Ok(())
}
