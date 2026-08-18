use core::future::{Future, poll_fn, ready};
use std::sync::Arc;

use finstack_ai_kernel::{
    ContentBlock, JsonBlock, ToolCallId, ToolProgress, ToolResultBlock, Usage, ValidationOutcome,
};

use super::error::{
    TOOL_DEFERRAL_INVALID, TOOL_DEFERRAL_NOT_DECLARED, TOOL_OUTPUT_INVALID,
    TOOL_RESULT_LIMIT_EXCEEDED, TOOL_STREAM_INVALID, TOOL_STREAM_LIMIT_EXCEEDED, ToolError,
};
use super::types::{ToolDeferral, ToolEventStream, ToolResult, ToolStreamItem};
use super::validator::ToolValidator;
use crate::ToolDeferralSupport;

/// Target-neutral stream limits applied before durable settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolStreamLimits {
    /// Maximum emitted items including the terminal result.
    pub max_items: usize,
    /// Maximum encoded progress and usage bytes.
    pub max_stream_bytes: usize,
}

impl Default for ToolStreamLimits {
    fn default() -> Self {
        Self {
            max_items: 4_096,
            max_stream_bytes: 1_048_576,
        }
    }
}

/// Validated direct-tool stream terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolTerminal {
    /// Successful tool result.
    Completed(ToolResult),
    /// External suspension.
    Deferred(ToolDeferral),
}

/// Fully normalized direct tool stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledToolStream {
    /// Transient progress updates in arrival order.
    pub progress: Arc<[ToolProgress]>,
    /// Final cumulative usage, when supplied.
    pub usage: Option<Usage>,
    /// Exactly one validated terminal.
    pub terminal: ToolTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AssembledToolTerminal {
    pub(crate) usage: Option<Usage>,
    pub(crate) terminal: ToolTerminal,
}

/// Target-neutral strict tool stream driver.
#[derive(Debug, Clone, Copy)]
pub struct ToolStreamAssembler {
    limits: ToolStreamLimits,
}

impl ToolStreamAssembler {
    /// Construct with explicit stream limits.
    #[must_use]
    pub const fn new(limits: ToolStreamLimits) -> Self {
        Self { limits }
    }

    /// Consume through EOF and normalize one tool stream.
    ///
    /// # Errors
    ///
    /// Rejects missing/duplicate terminals, post-terminal items or errors,
    /// invalid deferrals, regressing usage, oversized streams/results, and
    /// invalid successful output.
    pub async fn assemble(
        self,
        stream: ToolEventStream,
        output_validator: Option<&dyn ToolValidator>,
        max_result_bytes: u64,
        deferral: ToolDeferralSupport,
    ) -> Result<AssembledToolStream, ToolError> {
        let mut progress = Vec::new();
        let terminal = self
            .assemble_incremental(
                stream,
                output_validator,
                max_result_bytes,
                deferral,
                |item| {
                    progress.push(item);
                    ready(Ok(()))
                },
            )
            .await?;
        Ok(AssembledToolStream {
            progress: progress.into(),
            usage: terminal.usage,
            terminal: terminal.terminal,
        })
    }

    pub(crate) async fn assemble_incremental<F, Fut>(
        self,
        mut stream: ToolEventStream,
        output_validator: Option<&dyn ToolValidator>,
        max_result_bytes: u64,
        deferral: ToolDeferralSupport,
        mut emit_progress: F,
    ) -> Result<AssembledToolTerminal, ToolError>
    where
        F: FnMut(ToolProgress) -> Fut,
        Fut: Future<Output = Result<(), ToolError>>,
    {
        let mut count = 0_usize;
        let mut stream_bytes = 0_usize;
        let mut usage: Option<Usage> = None;
        let mut terminal = None;
        while let Some(item) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            if terminal.is_some() {
                let message = if item.is_err() {
                    "tool stream emitted an error after its terminal item"
                } else {
                    "tool stream emitted data after its terminal item"
                };
                return Err(ToolError::stable(TOOL_STREAM_INVALID, message));
            }
            count = count.checked_add(1).ok_or_else(stream_limit)?;
            if count > self.limits.max_items {
                return Err(stream_limit());
            }
            let item = item?;
            match item {
                ToolStreamItem::Progress(value) => {
                    add_stream_bytes(
                        &mut stream_bytes,
                        serde_json_canonicalizer::to_vec(&value)
                            .map_err(|_| stream_invalid())?
                            .len(),
                        self.limits.max_stream_bytes,
                    )?;
                    emit_progress(value).await?;
                }
                ToolStreamItem::Usage(value) => {
                    validate_usage(&value.usage, usage.as_ref())?;
                    add_stream_bytes(
                        &mut stream_bytes,
                        value
                            .usage
                            .canonical_bytes()
                            .map_err(|_| stream_invalid())?
                            .len(),
                        self.limits.max_stream_bytes,
                    )?;
                    usage = Some(value.usage);
                }
                ToolStreamItem::Completed(value) => {
                    if value.output.as_bytes().len() as u64 > max_result_bytes {
                        return Err(ToolError::stable(
                            TOOL_RESULT_LIMIT_EXCEEDED,
                            "tool result exceeds the registered byte limit",
                        ));
                    }
                    if !value.is_error
                        && output_validator.is_some_and(|validator| {
                            matches!(
                                validator.validate(&value.output),
                                ValidationOutcome::Invalid { .. }
                            )
                        })
                    {
                        return Err(ToolError::stable(
                            TOOL_OUTPUT_INVALID,
                            "successful tool output does not satisfy the registered schema",
                        ));
                    }
                    terminal = Some(ToolTerminal::Completed(value));
                }
                ToolStreamItem::Deferred(value) => {
                    if deferral == ToolDeferralSupport::Never {
                        return Err(ToolError::stable(
                            TOOL_DEFERRAL_NOT_DECLARED,
                            "tool returned an undeclared deferral",
                        ));
                    }
                    if value.handle.handle().is_empty()
                        || matches!(
                            (value.next_poll_at, value.expires_at),
                            (Some(next_poll_at), Some(expires_at)) if next_poll_at > expires_at
                        )
                    {
                        return Err(ToolError::stable(
                            TOOL_DEFERRAL_INVALID,
                            "tool returned an invalid deferral",
                        ));
                    }
                    terminal = Some(ToolTerminal::Deferred(value));
                }
            }
        }
        let terminal = terminal.ok_or_else(|| {
            ToolError::stable(
                TOOL_STREAM_INVALID,
                "tool stream ended without a terminal item",
            )
        })?;
        Ok(AssembledToolTerminal { usage, terminal })
    }
}

impl Default for ToolStreamAssembler {
    fn default() -> Self {
        Self::new(ToolStreamLimits::default())
    }
}

/// Inject a committed call id and canonical single-JSON-block framework envelope.
///
/// # Errors
///
/// Returns a validation error only if the fixed envelope violates kernel content bounds.
pub fn normalize_tool_result(
    tool_call_id: ToolCallId,
    result: ToolResult,
) -> Result<ToolResultBlock, ToolError> {
    ToolResultBlock::try_new(
        tool_call_id,
        vec![ContentBlock::Json(JsonBlock::new(result.output))],
        result.is_error,
    )
    .map_err(|_| ToolError::stable(TOOL_OUTPUT_INVALID, "tool result envelope is invalid"))
}

fn validate_usage(current: &Usage, previous: Option<&Usage>) -> Result<(), ToolError> {
    current.validate().map_err(|_| stream_invalid())?;
    if let (Some(input), Some(output), Some(total)) = (
        current.input_tokens(),
        current.output_tokens(),
        current.total_tokens(),
    ) && input.checked_add(output) != Some(total)
    {
        return Err(stream_invalid());
    }
    if let Some(previous) = previous
        && (regressed(previous.input_tokens(), current.input_tokens())
            || regressed(previous.output_tokens(), current.output_tokens())
            || regressed(previous.total_tokens(), current.total_tokens())
            || cost_regressed(previous, current)
            || previous.extension_counters().iter().any(|(key, value)| {
                current
                    .extension_counters()
                    .get(key)
                    .is_none_or(|current| current < value)
            }))
    {
        return Err(stream_invalid());
    }
    Ok(())
}

fn cost_regressed(previous: &Usage, current: &Usage) -> bool {
    match (previous.cost(), current.cost()) {
        (Some(_), None) => true,
        (Some(previous), Some(current)) => {
            previous.unit() != current.unit()
                || previous.pricing_policy_version() != current.pricing_policy_version()
                || previous.micros() > current.micros()
        }
        _ => false,
    }
}

fn regressed(previous: Option<u64>, current: Option<u64>) -> bool {
    match (previous, current) {
        (Some(_), None) => true,
        (Some(previous), Some(current)) => current < previous,
        _ => false,
    }
}

fn add_stream_bytes(total: &mut usize, add: usize, max: usize) -> Result<(), ToolError> {
    *total = total.checked_add(add).ok_or_else(stream_limit)?;
    if *total > max {
        return Err(stream_limit());
    }
    Ok(())
}

fn stream_limit() -> ToolError {
    ToolError::stable(
        TOOL_STREAM_LIMIT_EXCEEDED,
        "tool stream exceeds its configured limit",
    )
}

fn stream_invalid() -> ToolError {
    ToolError::stable(TOOL_STREAM_INVALID, "tool stream is malformed")
}
