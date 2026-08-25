use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    ActiveCapability, ArtifactRef, CapabilityId, ContentBlock, ErrorCode, ErrorDescriptor, Message,
    OperationLocator, RawJson, RunSecurityContext,
};
use finstack_ai_runtime::ids::IdGenerationError;
use finstack_ai_runtime::ports::model::{ModelError, ModelName, ModelSettings};
use finstack_ai_runtime::run::RunHandleError;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::session::SessionError;
use thiserror::Error;

/// Invalid public run configuration.
pub const AGENT_RUN_INVALID_CONFIGURATION: &str = "agent_run_invalid_configuration";
/// The resolved plan contains a stage not supported by the native preview driver.
pub const AGENT_RUN_UNSUPPORTED_PLAN: &str = "agent_run_unsupported_plan";
/// The commit-before-effect runtime failed.
pub const AGENT_RUN_RUNTIME_FAILURE: &str = "agent_run_runtime_failure";
/// The operational run deadline elapsed.
pub const AGENT_RUN_TIMEOUT: &str = "agent_run_timeout";
/// The run reached its durable cancelled terminal state.
pub const AGENT_RUN_CANCELLED: &str = "agent_run_cancelled";

pub(super) const DEFAULT_QUEUE_CAPACITY: usize = 32;
/// Default maximum model cycles for one run.
pub const DEFAULT_MAX_CYCLES: u64 = 16;
/// Largest caller-configured model-cycle limit.
pub const MAX_CONFIGURED_CYCLES: u64 = 1_024;
/// Default maximum structured-output validation retries.
pub const DEFAULT_MAX_OUTPUT_RETRIES: u32 = 1;
/// Largest caller-configured structured-output retry limit.
pub const MAX_CONFIGURED_OUTPUT_RETRIES: u32 = 1_024;
/// Default operational deadline for one run.
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const DEFAULT_EVENT_BATCH_COUNT: usize = 32;
pub(super) const DEFAULT_EVENT_BATCH_BYTES: usize = 64 * 1_024;
pub(super) const DEFAULT_EVENT_BATCH_INTERVAL: Duration = Duration::from_millis(10);
pub(super) const MAX_COMPACT_CATALOG_BYTES: usize = 8 * 1_024;
/// Maximum attachments accepted per run.
pub const MAX_RUN_ATTACHMENTS: usize = 8;

/// One pre-staged run attachment.
///
/// The artifact must already be durably staged in the run's `ArtifactStore`
/// scope; the run API never accepts raw bytes. Media type, length, digest,
/// and display name live on `artifact.blob()` — there is no separate
/// `media_type` field here, avoiding a second source of truth.
#[derive(Debug, Clone)]
pub struct AttachmentInput {
    /// Staged artifact whose blob carries media type, length, digest, name.
    pub artifact: ArtifactRef,
}

/// One compact model-visible capability catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCatalogEntry {
    pub(super) id: CapabilityId,
    pub(super) description: Arc<str>,
}

impl CapabilityCatalogEntry {
    /// Borrow the stable capability identity.
    #[must_use]
    pub const fn id(&self) -> &CapabilityId {
        &self.id
    }

    /// Borrow the compact non-secret description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }
}

impl fmt::Display for CapabilityCatalogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.description)
    }
}

/// One bounded native run request.
#[derive(Debug, Clone)]
pub struct AgentRunRequest {
    /// Provider model name selected from the resolved model descriptor.
    pub model: ModelName,
    /// Plain-text user input.
    pub input: Arc<str>,
    /// Explicit validated security context captured durably at acceptance.
    pub security: RunSecurityContext,
    /// Canonical provider-specific settings.
    pub settings: ModelSettings,
    /// Operational deadline for the complete run.
    pub timeout: Duration,
    /// Maximum number of model cycles.
    pub max_cycles: u64,
    /// Maximum structured-output validation retries.
    pub max_output_retries: u32,
    /// Optional model-activated capability to run instead of `self`.
    ///
    /// `None` keeps the `Agent` that was called. `Some` must name a
    /// model-activation catalog entry or the start fails closed.
    pub capability: Option<CapabilityId>,
    /// Pre-staged input attachments mapped to `File` blocks on the user message.
    pub attachments: Arc<[AttachmentInput]>,
}

impl AgentRunRequest {
    /// Construct a request with conservative defaults.
    ///
    /// `capability` defaults to `None` (run the `Agent` that was called).
    /// Timeout is 30 seconds, `max_cycles` is 16, and `max_output_retries` is 1.
    ///
    /// # Arguments
    ///
    /// * `model` - Provider model name advertised by the resolved model descriptor.
    /// * `input` - Non-empty plain-text user input. NUL bytes are rejected.
    /// * `security` - Explicit validated security context captured durably at acceptance.
    ///
    /// # Errors
    ///
    /// Returns a configuration error if the input is empty or invalid JSON
    /// defaults cannot be constructed.
    pub fn try_new(
        model: ModelName,
        input: impl Into<Arc<str>>,
        security: RunSecurityContext,
    ) -> Result<Self, AgentRunError> {
        let request = Self {
            model,
            input: input.into(),
            security,
            settings: ModelSettings {
                values: RawJson::parse(b"{}")
                    .map_err(|error| AgentRunError::runtime_message(error.to_string()))?,
            },
            timeout: DEFAULT_RUN_TIMEOUT,
            max_cycles: DEFAULT_MAX_CYCLES,
            max_output_retries: DEFAULT_MAX_OUTPUT_RETRIES,
            capability: None,
            attachments: Arc::from([]),
        };
        request.validate()?;
        Ok(request)
    }

    pub(super) fn validate(&self) -> Result<(), AgentRunError> {
        if self.input.is_empty()
            || self.input.as_bytes().contains(&0)
            || self.timeout.is_zero()
            || self.max_cycles == 0
            || self.max_cycles > MAX_CONFIGURED_CYCLES
            || self.max_output_retries > MAX_CONFIGURED_OUTPUT_RETRIES
        {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "input, timeout, or max_cycles is invalid",
            ));
        }
        if self.attachments.len() > MAX_RUN_ATTACHMENTS {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "run attachments exceed MAX_RUN_ATTACHMENTS",
            ));
        }
        Ok(())
    }
}

/// Successful native run output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunOutput {
    /// Complete durable locator for the run.
    pub locator: OperationLocator,
    /// Final committed assistant message.
    pub message: Message,
    /// Durable retry attempts consumed by the completed run.
    pub retry_attempts: u32,
    /// Complete sorted active capability set committed for this run.
    pub active_capabilities: Arc<[ActiveCapability]>,
    /// Stable committed record-kind trace in journal order.
    pub record_kinds: Arc<[Arc<str>]>,
}

impl AgentRunOutput {
    /// Concatenate final plain-text blocks in source order.
    #[must_use]
    pub fn text(&self) -> String {
        self.message
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text()),
                _ => None,
            })
            .collect()
    }

    /// Borrow the structured JSON result when the final message contains one.
    #[must_use]
    pub fn structured_json(&self) -> Option<&RawJson> {
        self.message.content().iter().find_map(|block| match block {
            ContentBlock::Json(value) => Some(value.value()),
            _ => None,
        })
    }

    /// Return the durable retry-attempt count.
    #[must_use]
    pub const fn retry_attempts(&self) -> u32 {
        self.retry_attempts
    }

    /// Borrow the complete committed active capability set.
    #[must_use]
    pub fn active_capabilities(&self) -> &[ActiveCapability] {
        &self.active_capabilities
    }

    /// Borrow the stable committed record-kind trace in journal order.
    #[must_use]
    pub fn record_kinds(&self) -> &[Arc<str>] {
        &self.record_kinds
    }
}

/// Stable native facade failure.
#[derive(Debug, Clone, Error)]
pub enum AgentRunError {
    /// Invalid or unsupported resolved/run configuration.
    #[error("{code}: {message}")]
    Configuration {
        /// Stable error code.
        code: ErrorCode,
        /// Non-secret explanation.
        message: String,
    },
    /// Runtime or provider failure.
    ///
    /// `code` is the runtime's own code where one was reported, so the
    /// failure stays machine-readable rather than collapsing into one
    /// generic string.
    #[error("{code}: {message}")]
    Runtime {
        /// Stable code reported by the runtime.
        code: ErrorCode,
        /// Non-secret explanation.
        message: String,
    },
    /// Operational deadline elapsed.
    #[error("{}: run exceeded {timeout:?}", AGENT_RUN_TIMEOUT)]
    Timeout {
        /// Configured deadline.
        timeout: Duration,
    },
    /// Explicit durable cancellation reached its terminal state.
    #[error("{}: run was cancelled", AGENT_RUN_CANCELLED)]
    Cancelled,
    /// A port reported a structured failure.
    ///
    /// Carries the full [`ErrorDescriptor`] -- code, category, retryability,
    /// identifiers and safe details -- instead of flattening it into a
    /// message. Read it with [`AgentRunError::descriptor`].
    #[error("{descriptor}")]
    Failed {
        /// Structured failure reported by the port that produced it.
        descriptor: Box<ErrorDescriptor>,
    },
}

impl AgentRunError {
    /// Stable machine-readable error code.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Failed { descriptor } => descriptor.code.as_str(),
            Self::Runtime { code, .. } | Self::Configuration { code, .. } => code.as_str(),
            Self::Timeout { .. } => AGENT_RUN_TIMEOUT,
            Self::Cancelled => AGENT_RUN_CANCELLED,
        }
    }

    #[cfg(feature = "native-tokio")]
    pub(crate) fn owned_code(&self) -> ErrorCode {
        match self {
            Self::Failed { descriptor } => descriptor.code.clone(),
            Self::Runtime { code, .. } | Self::Configuration { code, .. } => code.clone(),
            Self::Timeout { .. } => finstack_ai_kernel::static_error_code!(AGENT_RUN_TIMEOUT),
            Self::Cancelled => finstack_ai_kernel::static_error_code!(AGENT_RUN_CANCELLED),
        }
    }

    /// Whether an identical frontend call is safe to retry automatically.
    ///
    /// Native run errors remain non-retryable because committed acceptance or
    /// external-effect state may already exist.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        false
    }

    pub(super) fn configuration(code: impl AsRef<str>, message: impl Into<String>) -> Self {
        Self::Configuration {
            code: ErrorCode::new(code).unwrap_or_else(|_| {
                finstack_ai_kernel::static_error_code!(AGENT_RUN_INVALID_CONFIGURATION)
            }),
            message: message.into(),
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "used directly as a Result::map_err adapter"
    )]
    pub(super) fn runtime(error: RunHandleError) -> Self {
        ErrorCode::new(error.code()).map_or_else(
            |_| Self::runtime_message(error.to_string()),
            |code| Self::Runtime {
                code,
                message: error.to_string(),
            },
        )
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "used directly as a Result::map_err adapter"
    )]
    pub(super) fn model(error: ModelError) -> Self {
        error.to_descriptor().map_or_else(
            |fallback| Self::runtime_message(fallback.to_string()),
            |descriptor| Self::Failed {
                descriptor: Box::new(descriptor),
            },
        )
    }

    #[cfg(feature = "native-tokio")]
    pub(super) fn session(error: &SessionError) -> Self {
        let code = ErrorCode::new(error.code())
            .unwrap_or_else(|_| finstack_ai_kernel::static_error_code!(AGENT_RUN_RUNTIME_FAILURE));
        Self::Runtime {
            code,
            message: error.to_string(),
        }
    }

    /// Structured failure descriptor, when a port reported one.
    ///
    /// Present for [`AgentRunError::Failed`]; `None` for configuration,
    /// timeout and cancellation failures, which the SDK raises itself.
    #[must_use]
    pub fn descriptor(&self) -> Option<&ErrorDescriptor> {
        match self {
            Self::Failed { descriptor } => Some(descriptor),
            _ => None,
        }
    }

    pub(super) fn runtime_message(message: impl Into<String>) -> Self {
        Self::Runtime {
            code: finstack_ai_kernel::static_error_code!(AGENT_RUN_RUNTIME_FAILURE),
            message: message.into(),
        }
    }
}

impl From<IdGenerationError> for AgentRunError {
    fn from(error: IdGenerationError) -> Self {
        Self::runtime_message(error.to_string())
    }
}
