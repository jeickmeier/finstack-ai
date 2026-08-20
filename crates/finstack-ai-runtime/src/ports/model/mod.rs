//! Provider-neutral model port, immutable context profiles, and stream assembly.

mod context;
mod error;
mod identity;
mod port;
mod profile;
mod provider_util;
mod request;
mod stream;

#[cfg(test)]
mod tests;

/// Stable invalid-request adapter code.
pub const MODEL_REQUEST_INVALID: &str = "model_request_invalid";
/// Stable invalid-profile adapter code.
pub const MODEL_PROFILE_INVALID: &str = "model_profile_invalid";
/// Stable profile-relaxation adapter code.
pub const MODEL_PROFILE_RELAXATION: &str = "model_profile_relaxation";
/// Stable forbidden-run-override adapter code.
pub const MODEL_PROFILE_OVERRIDE_NOT_ALLOWED: &str = "model_profile_override_not_allowed";
/// Stable estimator mismatch adapter code.
pub const MODEL_ESTIMATOR_MISMATCH: &str = "model_estimator_mismatch";
/// Stable context-limit adapter code.
pub const MODEL_CONTEXT_LIMIT_EXCEEDED: &str = "model_context_limit_exceeded";
/// Stable missing-terminal adapter code.
pub const MODEL_STREAM_MISSING_COMPLETION: &str = "model_stream_missing_completion";
/// Stable duplicate-terminal adapter code.
pub const MODEL_STREAM_DUPLICATE_COMPLETION: &str = "model_stream_duplicate_completion";
/// Stable post-terminal item adapter code.
pub const MODEL_STREAM_ITEM_AFTER_COMPLETION: &str = "model_stream_item_after_completion";
/// Stable post-terminal error adapter code.
pub const MODEL_STREAM_ERROR_AFTER_COMPLETION: &str = "model_stream_error_after_completion";
/// Stable stream-bound adapter code.
pub const MODEL_STREAM_LIMIT_EXCEEDED: &str = "model_stream_limit_exceeded";
/// Stable malformed tool-call delta adapter code.
pub const MODEL_TOOL_CALL_DELTA_INVALID: &str = "model_tool_call_delta_invalid";
/// Stable incomplete tool-call adapter code.
pub const MODEL_TOOL_CALL_INCOMPLETE: &str = "model_tool_call_incomplete";
/// Stable invalid tool arguments adapter code.
pub const MODEL_TOOL_CALL_ARGUMENTS_INVALID: &str = "model_tool_call_arguments_invalid";
/// Stable invalid usage adapter code.
pub const MODEL_USAGE_INVALID: &str = "model_usage_invalid";
/// Stable response/stream mismatch adapter code.
pub const MODEL_RESPONSE_MISMATCH: &str = "model_response_mismatch";
/// Stable non-resumable model-reconciliation adapter code.
pub const MODEL_RECONCILIATION_UNSUPPORTED: &str = "model_reconciliation_unsupported";

pub use context::{
    AuthorizationContext, CancellationSignal, ModelCallContext, ModelReconcileResult, ModelRequest,
    ModelResumeAction, ModelWarmupContext, ReconcileContext, RunCallContext,
    map_model_reconcile_result, model_resume_action, model_retry_allowed,
};
pub use error::ModelError;
pub use identity::{ModelDescriptor, ModelName};
pub use port::{Model, validate_model_request};
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) use port::{parse_committed_model_request, stable_model_dispatch_code};
pub use profile::{
    InputCapabilities, LockedModelContextProfile, ModelCapabilities, ModelContextProfile,
    ModelContextProfileOverride, StructuredOutputCapability, TokenEstimatorRef,
    TokenEstimatorSource, resolve_model_context_profile,
};
pub use provider_util::{
    AnthropicMessagesAssembly, Authentication, CredentialReference, CredentialRejected,
    CredentialStore, GEMINI_CACHED_TOKENS_KEY, GEMINI_CODE_RESULT_MEDIA_TYPE,
    GEMINI_CONTINUATION_PROVIDER, GEMINI_EXECUTABLE_CODE_MEDIA_TYPE, GEMINI_GROUNDING_MEDIA_TYPE,
    GEMINI_THOUGHTS_TOKENS_KEY, GeminiGenerateContentAssembly, MediaResolveError, MediaResolveKind,
    MediaResolver, NdjsonError, NdjsonParser, OllamaChatAssembly, OllamaReplayEntry,
    OpenAiResponsesAssembly, ResolveDraftMediaError, ResolvedMedia, SECRET_MAX_BYTES,
    SecretRejected, SecretString, SseEvent, SseEventParser, SseParseError, StreamNormError,
    StreamNormKind, resolve_draft_media, secret_is_valid, thinking_level_budget,
};
pub use request::{
    ApprovalGrantMode, ApprovalMetadata, ApprovalRequirement, ModelDeferral, ModelRequestDraft,
    ModelRequestLimits, ModelResponse, ModelSettings, ModelTokenEstimate, ModelToolCall,
    SideEffectClass, ToolDeferralSupport, ToolSpec,
};
pub use stream::{
    AssembledModelStream, ModelEventStream, ModelProgress, ModelStreamAssembler, ModelStreamItem,
    ModelStreamLimits, ModelTerminal, OpaqueProviderEvent, ReasoningDelta, TextDelta,
    ToolCallDelta, UsageDelta,
};
