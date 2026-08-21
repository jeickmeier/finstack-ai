use std::sync::Arc;

use finstack_ai_kernel::PendingModelEffect;
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
use finstack_ai_kernel::RawJson;

use crate::{PortFuture, PortObject};

use super::context::{ModelReconcileResult, ModelRequest, ModelWarmupContext, ReconcileContext};
use super::error::ModelError;
use super::identity::{ModelDescriptor, ModelName};
#[cfg(any(test, feature = "native-tokio", feature = "wasm-host"))]
use super::profile::LockedModelContextProfile;
use super::profile::ModelCapabilities;
use super::request::ModelTokenEstimate;
#[cfg(any(test, feature = "native-tokio", feature = "wasm-host"))]
use super::request::{ModelRequestDraft, ModelRequestValidation};
use super::stream::ModelEventStream;
#[cfg(any(test, feature = "native-tokio", feature = "wasm-host"))]
use super::{
    MODEL_CONTEXT_LIMIT_EXCEEDED, MODEL_ESTIMATOR_MISMATCH, MODEL_PROFILE_INVALID,
    MODEL_REQUEST_INVALID,
};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
use super::{MODEL_PROFILE_OVERRIDE_NOT_ALLOWED, MODEL_PROFILE_RELAXATION};

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

/// Proof that a retained model completed construction warmup successfully.
///
/// The SDK caches this value with the resolved component. Run-task owners
/// accept the proof instead of calling [`Model::warmup`] themselves, so one
/// retained model is prepared exactly once and the proof is shared by every
/// run that uses it.
#[derive(Clone)]
pub struct ReadyModel {
    model: Arc<dyn Model>,
}

impl ReadyModel {
    /// Validate and warm one retained model.
    ///
    /// A failed or cancelled warmup does not produce a readiness proof, so the
    /// caller may retry preparation with a fresh construction context.
    ///
    /// # Errors
    ///
    /// Returns the descriptor or adapter error produced before readiness.
    pub async fn prepare(model: Arc<dyn Model>) -> Result<Self, ModelError> {
        Self::prepare_with_context(model, ModelWarmupContext::default()).await
    }

    /// Validate and warm one retained model with an explicit construction context.
    ///
    /// # Errors
    ///
    /// Returns the descriptor or adapter error produced before readiness.
    pub async fn prepare_with_context(
        model: Arc<dyn Model>,
        context: ModelWarmupContext,
    ) -> Result<Self, ModelError> {
        model.descriptor().validate()?;
        model.warmup(context).await?;
        Ok(Self { model })
    }

    /// Borrow the prepared model port.
    #[must_use]
    pub fn as_model(&self) -> &dyn Model {
        self.model.as_ref()
    }

    /// Clone the prepared model port for an owned execution path.
    #[must_use]
    pub fn shared_model(&self) -> Arc<dyn Model> {
        Arc::clone(&self.model)
    }
}

impl Model for ReadyModel {
    fn descriptor(&self) -> ModelDescriptor {
        self.model.descriptor()
    }

    fn capabilities(&self, model: &ModelName) -> ModelCapabilities {
        self.model.capabilities(model)
    }

    fn estimate_input_tokens(
        &self,
        model: &ModelName,
        canonical_request: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        self.model.estimate_input_tokens(model, canonical_request)
    }

    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        self.model.request(request)
    }

    fn reconcile(
        &self,
        context: ReconcileContext,
        effect: PendingModelEffect,
    ) -> PortFuture<Result<ModelReconcileResult, ModelError>> {
        self.model.reconcile(context, effect)
    }
}

/// Validate canonical bytes, estimator binding, and effective token/output limits.
///
/// # Errors
///
/// Returns stable request/profile/estimator/context-limit errors.
#[cfg(any(test, feature = "native-tokio", feature = "wasm-host"))]
pub(crate) fn validate_model_request(
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
        || draft.limits.max_output_tokens > locked.profile.reserved_output_tokens
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
        || draft.limits.max_output_tokens > locked.profile.max_output_tokens
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

/// Decode a committed model-request DTO and run [`validate_model_request`].
///
/// Native and host dispatchers share this so a later validation change cannot
/// drift between owners.
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) fn parse_committed_model_request(
    model: &dyn Model,
    profile: &LockedModelContextProfile,
    raw: &RawJson,
) -> Result<ModelRequestDraft, ModelError> {
    let draft: ModelRequestDraft = serde_json::from_slice(raw.as_bytes()).map_err(|_| {
        ModelError::validation(
            MODEL_REQUEST_INVALID,
            "committed model request draft is invalid",
        )
    })?;
    if draft.canonical_bytes()?.as_slice() != raw.as_bytes() {
        return Err(ModelError::validation(
            MODEL_REQUEST_INVALID,
            "model request draft is not the canonical committed DTO",
        ));
    }
    validate_model_request(model, &draft, profile)?;
    Ok(draft)
}

/// Map a model-adapter code onto the dispatch table both owners share.
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[must_use]
pub(crate) fn stable_model_dispatch_code(code: &str) -> &'static str {
    match code {
        MODEL_PROFILE_INVALID => MODEL_PROFILE_INVALID,
        MODEL_PROFILE_RELAXATION => MODEL_PROFILE_RELAXATION,
        MODEL_PROFILE_OVERRIDE_NOT_ALLOWED => MODEL_PROFILE_OVERRIDE_NOT_ALLOWED,
        MODEL_ESTIMATOR_MISMATCH => MODEL_ESTIMATOR_MISMATCH,
        MODEL_CONTEXT_LIMIT_EXCEEDED => MODEL_CONTEXT_LIMIT_EXCEEDED,
        _ => MODEL_REQUEST_INVALID,
    }
}
