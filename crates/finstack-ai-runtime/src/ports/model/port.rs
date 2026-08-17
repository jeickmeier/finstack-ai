use finstack_ai_kernel::PendingModelEffect;

use crate::{PortFuture, PortObject};

use super::context::{ModelReconcileResult, ModelRequest, ModelWarmupContext, ReconcileContext};
use super::error::ModelError;
use super::identity::{ModelDescriptor, ModelName};
use super::profile::LockedModelContextProfile;
use super::profile::ModelCapabilities;
use super::request::{ModelRequestDraft, ModelRequestValidation, ModelTokenEstimate};
use super::stream::ModelEventStream;
use super::{
    MODEL_CONTEXT_LIMIT_EXCEEDED, MODEL_ESTIMATOR_MISMATCH, MODEL_PROFILE_INVALID,
    MODEL_REQUEST_INVALID,
};

/// Object-safe provider-neutral model port.
///
/// Implementors fulfill one committed model request after the runtime has
/// recorded intent. Native objects are `Send + Sync`. Browser-WASM hosts stay
/// local and must not be marked thread-safe.
///
/// # Required methods
///
/// - [`descriptor`](Self::descriptor): immutable provider/model identity
/// - [`capabilities`](Self::capabilities): flags for one model name
/// - [`estimate_input_tokens`](Self::estimate_input_tokens): synchronous estimate, no I/O
/// - [`request`](Self::request): start one normalized event stream
///
/// `warmup` and `reconcile` have default implementations.
pub trait Model: PortObject {
    /// Immutable provider/model descriptor.
    fn descriptor(&self) -> ModelDescriptor;

    /// Capabilities for one descriptor model name.
    ///
    /// # Arguments
    ///
    /// * `model` - Name advertised by [`Self::descriptor`].
    fn capabilities(&self, model: &ModelName) -> ModelCapabilities;

    /// Perform one-time construction warmup.
    fn warmup(&self, _ctx: ModelWarmupContext) -> PortFuture<Result<(), ModelError>> {
        Box::pin(async { Ok(()) })
    }

    /// Estimate canonical request input synchronously without I/O.
    ///
    /// # Arguments
    ///
    /// * `model` - Name whose estimator is bound at warmup.
    /// * `canonical_request` - Canonical UTF-8 request bytes, not provider JSON.
    ///
    /// # Errors
    ///
    /// Returns a provider or validation error when the named estimator cannot
    /// produce a bound estimate for the canonical request.
    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError>;

    /// Start one normalized model stream.
    ///
    /// # Arguments
    ///
    /// * `request` - Committed, immutable model request. Do not mutate caller state.
    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>>;

    /// Reconcile one previously committed outstanding effect.
    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingModelEffect,
    ) -> PortFuture<Result<ModelReconcileResult, ModelError>> {
        Box::pin(async { Ok(ModelReconcileResult::Unknown) })
    }
}

/// Validate canonical bytes, estimator binding, and effective token/output limits.
///
/// # Errors
///
/// Returns stable request/profile/estimator/context-limit errors.
pub fn validate_model_request(
    model: &dyn Model,
    draft: &ModelRequestDraft,
    locked: &LockedModelContextProfile,
) -> Result<ModelRequestValidation, ModelError> {
    if draft.model != locked.profile.model {
        return Err(ModelError::validation(
            MODEL_REQUEST_INVALID,
            "model request does not match the locked profile",
        ));
    }
    let canonical = draft.canonical_bytes()?;
    let canonical_len = u64::try_from(canonical.len()).map_err(|_| {
        ModelError::limit(MODEL_CONTEXT_LIMIT_EXCEEDED, "request byte length overflow")
    })?;
    let byte_limit = draft
        .limits
        .max_input_bytes
        .min(locked.profile.hard_input_bytes);
    if byte_limit == 0 || canonical_len > byte_limit {
        return Err(ModelError::limit(
            MODEL_CONTEXT_LIMIT_EXCEEDED,
            "model request exceeds the canonical byte ceiling",
        ));
    }
    if draft.limits.max_input_bytes > locked.profile.hard_input_bytes
        || draft.limits.max_output_tokens > locked.profile.max_output_tokens
        || draft.limits.max_input_tokens == 0
        || draft.limits.max_output_tokens == 0
    {
        return Err(ModelError::limit(
            MODEL_CONTEXT_LIMIT_EXCEEDED,
            "model request limits exceed the locked profile",
        ));
    }
    let estimate = model.estimate_input_tokens(&draft.model, &canonical)?;
    if estimate.estimator != locked.profile.estimator {
        return Err(ModelError::validation(
            MODEL_ESTIMATOR_MISMATCH,
            "model token estimator does not match the locked profile",
        ));
    }
    let margins = locked
        .profile
        .reserved_output_tokens
        .checked_add(locked.profile.provider_overhead_tokens)
        .ok_or_else(|| {
            ModelError::validation(MODEL_PROFILE_INVALID, "model profile margin overflow")
        })?;
    let available = locked
        .profile
        .context_window_tokens
        .checked_sub(margins)
        .ok_or_else(|| {
            ModelError::validation(MODEL_PROFILE_INVALID, "model profile margins are invalid")
        })?;
    let input_limit = draft.limits.max_input_tokens.min(available);
    if estimate.input_tokens > input_limit
        || draft.limits.max_output_tokens > locked.profile.reserved_output_tokens
    {
        return Err(ModelError::limit(
            MODEL_CONTEXT_LIMIT_EXCEEDED,
            "model request exceeds the effective token budget",
        ));
    }
    Ok(ModelRequestValidation {
        canonical_request: canonical.into(),
        estimated_input_tokens: estimate.input_tokens,
        available_input_tokens: available,
    })
}
