use core::future::{poll_fn, ready};
use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{ContentBlock, Metadata, RawJson, Usage};
use serde::{Deserialize, Serialize};

use crate::ports::PortStream;

use super::error::{ModelError, STREAM_REASONING_MAX_BYTES, STREAM_TEXT_MAX_BYTES};
use super::identity::validated_label;
use super::request::{ModelDeferral, ModelResponse, ModelToolCall};
use super::{
    MODEL_RESPONSE_MISMATCH, MODEL_STREAM_DUPLICATE_COMPLETION,
    MODEL_STREAM_ERROR_AFTER_COMPLETION, MODEL_STREAM_ITEM_AFTER_COMPLETION,
    MODEL_STREAM_LIMIT_EXCEEDED, MODEL_STREAM_MISSING_COMPLETION,
    MODEL_TOOL_CALL_ARGUMENTS_INVALID, MODEL_TOOL_CALL_DELTA_INVALID, MODEL_TOOL_CALL_INCOMPLETE,
    MODEL_USAGE_INVALID,
};

/// Boxed target-correct model event stream.
pub type ModelEventStream = PortStream<Result<ModelStreamItem, ModelError>>;

/// Text fragment emitted by a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextDelta {
    /// Non-empty UTF-8 fragment.
    pub text: Arc<str>,
}

/// Confidential provider reasoning fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningDelta {
    /// Non-empty UTF-8 fragment.
    pub text: Arc<str>,
}

/// Cumulative normalized usage snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageDelta {
    /// Cumulative usage.
    pub usage: Usage,
}

/// One source tool-call fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallDelta {
    /// Provider-local call index.
    pub index: u32,
    /// Name supplied on first or a later non-mutating fragment.
    pub name: Option<Arc<str>>,
    /// UTF-8 arguments fragment.
    pub arguments_delta: Arc<str>,
    /// Provider-native call identity, when the fragment supplies one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<Arc<str>>,
}

/// Opaque bounded provider event retained only inside the driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpaqueProviderEvent {
    /// Namespaced provider event kind.
    pub namespace: Arc<str>,
    /// Canonical bounded payload.
    pub payload: RawJson,
}

/// Normalized model stream item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelStreamItem {
    /// Assistant text fragment.
    TextDelta(TextDelta),
    /// Confidential reasoning fragment.
    ReasoningDelta(ReasoningDelta),
    /// Tool-call fragment.
    ToolCallDelta(ToolCallDelta),
    /// Cumulative usage snapshot.
    Usage(UsageDelta),
    /// Explicit provider heartbeat metadata.
    Heartbeat(Metadata),
    /// Driver-local opaque provider event.
    ProviderEvent(OpaqueProviderEvent),
    /// Successful terminal response.
    Completed(ModelResponse),
    /// Suspended terminal response.
    Deferred(ModelDeferral),
}

/// Aggregate stream limits applied before durable settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelStreamLimits {
    /// Maximum stream items including the terminal item.
    pub max_items: usize,
    /// Maximum aggregate observed payload bytes.
    pub max_bytes: usize,
    /// Maximum distinct source tool calls.
    pub max_tool_calls: usize,
}

impl Default for ModelStreamLimits {
    fn default() -> Self {
        Self {
            max_items: 4_096,
            max_bytes: 2 * 1_048_576,
            max_tool_calls: 1_024,
        }
    }
}

/// Existing transient-progress vocabulary emitted by the model driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelProgress {
    /// Assistant text fragment.
    Text(Arc<str>),
    /// Confidential reasoning fragment.
    Reasoning(Arc<str>),
    /// Explicit heartbeat metadata.
    Heartbeat(Metadata),
}

/// Validated stream terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelTerminal {
    /// Successful response.
    Completed(ModelResponse),
    /// External suspension.
    Deferred(ModelDeferral),
}

/// Completely validated stream result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledModelStream {
    /// Existing transient progress in source order.
    pub progress: Arc<[ModelProgress]>,
    /// Exactly one validated terminal.
    pub terminal: ModelTerminal,
}

#[derive(Default)]
struct PartialToolCall {
    name: Option<Arc<str>>,
    arguments: String,
    provider_call_id: Option<Arc<str>>,
}

/// Pure bounded stream validator and assembler.
#[derive(Debug, Clone, Copy)]
pub struct ModelStreamAssembler {
    limits: ModelStreamLimits,
}

impl ModelStreamAssembler {
    /// Construct with explicit aggregate limits.
    ///
    /// # Errors
    ///
    /// Rejects zero bounds.
    pub fn new(limits: ModelStreamLimits) -> Result<Self, ModelError> {
        if limits.max_items == 0 || limits.max_bytes == 0 || limits.max_tool_calls == 0 {
            return Err(ModelError::validation(
                MODEL_STREAM_LIMIT_EXCEEDED,
                "model stream limits must be non-zero",
            ));
        }
        Ok(Self { limits })
    }

    /// Consume through EOF and validate exactly one terminal item.
    ///
    /// # Errors
    ///
    /// Returns stable ordering, bound, usage, tool-call, and response mismatch errors.
    pub async fn assemble(
        &self,
        stream: ModelEventStream,
    ) -> Result<AssembledModelStream, ModelError> {
        let mut progress = Vec::new();
        let terminal = self
            .assemble_incremental(stream, |item| {
                progress.push(item);
                ready(Ok(()))
            })
            .await?;
        Ok(AssembledModelStream {
            progress: progress.into(),
            terminal,
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the stream state machine keeps all ordering and terminal transitions contiguous"
    )]
    pub(crate) async fn assemble_incremental<F, Fut>(
        &self,
        mut stream: ModelEventStream,
        mut emit_progress: F,
    ) -> Result<ModelTerminal, ModelError>
    where
        F: FnMut(ModelProgress) -> Fut,
        Fut: Future<Output = Result<(), ModelError>>,
    {
        let mut item_count = 0_usize;
        let mut byte_count = 0_usize;
        let mut text = String::new();
        let mut reasoning_bytes = 0_usize;
        let mut tools = BTreeMap::<u32, PartialToolCall>::new();
        let mut order = Vec::<u32>::new();
        let mut usage: Option<Usage> = None;
        let mut terminal: Option<ModelTerminal> = None;

        while let Some(item) = poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
            if terminal.is_some() {
                return match item {
                    Err(_) => Err(ModelError::validation(
                        MODEL_STREAM_ERROR_AFTER_COMPLETION,
                        "model stream returned an error after its terminal item",
                    )),
                    Ok(ModelStreamItem::Completed(_) | ModelStreamItem::Deferred(_)) => {
                        Err(ModelError::validation(
                            MODEL_STREAM_DUPLICATE_COMPLETION,
                            "model stream returned more than one terminal item",
                        ))
                    }
                    Ok(_) => Err(ModelError::validation(
                        MODEL_STREAM_ITEM_AFTER_COMPLETION,
                        "model stream returned an item after its terminal item",
                    )),
                };
            }
            let item = item?;
            item_count = item_count.checked_add(1).ok_or_else(stream_limit_error)?;
            if item_count > self.limits.max_items {
                return Err(stream_limit_error());
            }
            match item {
                ModelStreamItem::TextDelta(delta) => {
                    validate_delta(&delta.text)?;
                    add_bytes(&mut byte_count, delta.text.len(), self.limits.max_bytes)?;
                    text.push_str(&delta.text);
                    if text.len() > STREAM_TEXT_MAX_BYTES {
                        return Err(stream_limit_error());
                    }
                    emit_progress(ModelProgress::Text(delta.text)).await?;
                }
                ModelStreamItem::ReasoningDelta(delta) => {
                    validate_delta(&delta.text)?;
                    add_bytes(&mut byte_count, delta.text.len(), self.limits.max_bytes)?;
                    reasoning_bytes = reasoning_bytes
                        .checked_add(delta.text.len())
                        .ok_or_else(stream_limit_error)?;
                    if reasoning_bytes > STREAM_REASONING_MAX_BYTES {
                        return Err(stream_limit_error());
                    }
                    emit_progress(ModelProgress::Reasoning(delta.text)).await?;
                }
                ModelStreamItem::ToolCallDelta(delta) => {
                    let is_new = !tools.contains_key(&delta.index);
                    if is_new {
                        if tools.len() >= self.limits.max_tool_calls {
                            return Err(stream_limit_error());
                        }
                        order.push(delta.index);
                    }
                    let partial = tools.entry(delta.index).or_default();
                    if let Some(name) = delta.name {
                        validated_label(&name, "tool_call.name")?;
                        add_bytes(&mut byte_count, name.len(), self.limits.max_bytes)?;
                        if partial
                            .name
                            .as_ref()
                            .is_some_and(|current| current != &name)
                        {
                            return Err(ModelError::validation(
                                MODEL_TOOL_CALL_DELTA_INVALID,
                                "model tool-call name mutated across fragments",
                            ));
                        }
                        partial.name = Some(name);
                    }
                    add_bytes(
                        &mut byte_count,
                        delta.arguments_delta.len(),
                        self.limits.max_bytes,
                    )?;
                    partial.arguments.push_str(&delta.arguments_delta);
                    if let Some(provider_call_id) = delta.provider_call_id {
                        validated_label(&provider_call_id, "tool_call.provider_call_id")?;
                        if partial
                            .provider_call_id
                            .as_ref()
                            .is_some_and(|current| current != &provider_call_id)
                        {
                            return Err(ModelError::validation(
                                MODEL_TOOL_CALL_DELTA_INVALID,
                                "model tool-call provider id mutated across fragments",
                            ));
                        }
                        partial.provider_call_id = Some(provider_call_id);
                    }
                }
                ModelStreamItem::Usage(delta) => {
                    validate_usage(&delta.usage, usage.as_ref())?;
                    usage = Some(delta.usage);
                }
                ModelStreamItem::Heartbeat(metadata) => {
                    add_bytes(
                        &mut byte_count,
                        metadata.as_bytes().len(),
                        self.limits.max_bytes,
                    )?;
                    emit_progress(ModelProgress::Heartbeat(metadata)).await?;
                }
                ModelStreamItem::ProviderEvent(event) => {
                    validated_label(&event.namespace, "provider_event.namespace")?;
                    add_bytes(
                        &mut byte_count,
                        event.namespace.len(),
                        self.limits.max_bytes,
                    )?;
                    add_bytes(
                        &mut byte_count,
                        event.payload.as_bytes().len(),
                        self.limits.max_bytes,
                    )?;
                }
                ModelStreamItem::Completed(response) => {
                    let encoded_len = canonical_byte_len(&response).ok_or_else(|| {
                        ModelError::validation(
                            MODEL_RESPONSE_MISMATCH,
                            "model response is not serializable",
                        )
                    })?;
                    add_bytes(&mut byte_count, encoded_len, self.limits.max_bytes)?;
                    terminal = Some(ModelTerminal::Completed(response));
                }
                ModelStreamItem::Deferred(deferral) => {
                    let encoded_len = canonical_byte_len(&deferral).ok_or_else(|| {
                        ModelError::validation(
                            MODEL_RESPONSE_MISMATCH,
                            "model deferral is not serializable",
                        )
                    })?;
                    add_bytes(&mut byte_count, encoded_len, self.limits.max_bytes)?;
                    terminal = Some(ModelTerminal::Deferred(deferral));
                }
            }
        }

        let terminal = terminal.ok_or_else(|| {
            ModelError::validation(
                MODEL_STREAM_MISSING_COMPLETION,
                "model stream ended without a terminal item",
            )
        })?;
        match &terminal {
            ModelTerminal::Completed(response) => {
                validate_completed_response(response, &text, &tools, &order, usage.as_ref())?;
            }
            ModelTerminal::Deferred(deferral) => {
                if !tools.is_empty() {
                    return Err(ModelError::validation(
                        MODEL_TOOL_CALL_INCOMPLETE,
                        "deferred model stream contains an unsettled tool call",
                    ));
                }
                if deferral
                    .next_poll_at
                    .zip(deferral.expires_at)
                    .is_some_and(|(next, expires)| next > expires)
                {
                    return Err(ModelError::validation(
                        MODEL_RESPONSE_MISMATCH,
                        "model deferral poll time exceeds its expiry",
                    ));
                }
            }
        }
        Ok(terminal)
    }
}

fn validate_completed_response(
    response: &ModelResponse,
    streamed_text: &str,
    tools: &BTreeMap<u32, PartialToolCall>,
    order: &[u32],
    usage: Option<&Usage>,
) -> Result<(), ModelError> {
    validated_label(&response.completion_id, "completion_id")?;
    let mut final_text = String::new();
    for block in response.assistant_content.iter() {
        match block {
            ContentBlock::Text(value) => final_text.push_str(value.text()),
            ContentBlock::ToolCall(_) | ContentBlock::ToolResult(_) => {
                return Err(ModelError::validation(
                    MODEL_RESPONSE_MISMATCH,
                    "model response content contains a framework-owned tool block",
                ));
            }
            _ => {}
        }
    }
    if final_text != streamed_text {
        return Err(ModelError::validation(
            MODEL_RESPONSE_MISMATCH,
            "streamed text does not match the final response",
        ));
    }
    let mut assembled_calls = Vec::with_capacity(order.len());
    for index in order {
        let partial = &tools[index];
        let name = partial.name.clone().ok_or_else(|| {
            ModelError::validation(
                MODEL_TOOL_CALL_INCOMPLETE,
                "model tool call completed without a name",
            )
        })?;
        if partial.arguments.is_empty() {
            return Err(ModelError::validation(
                MODEL_TOOL_CALL_INCOMPLETE,
                "model tool call completed without arguments",
            ));
        }
        let arguments = RawJson::parse(partial.arguments.as_bytes()).map_err(|_| {
            ModelError::validation(
                MODEL_TOOL_CALL_ARGUMENTS_INVALID,
                "model tool-call arguments are not strict JSON",
            )
        })?;
        assembled_calls.push(ModelToolCall {
            name,
            arguments,
            provider_call_id: partial.provider_call_id.clone(),
        });
    }
    if assembled_calls.as_slice() != response.tool_calls.as_ref() {
        return Err(ModelError::validation(
            MODEL_RESPONSE_MISMATCH,
            "streamed tool calls do not match the final response",
        ));
    }
    validate_usage(&response.usage, None)?;
    if usage.is_some_and(|streamed| streamed != &response.usage) {
        return Err(ModelError::validation(
            MODEL_RESPONSE_MISMATCH,
            "streamed usage does not match the final response",
        ));
    }
    Ok(())
}

fn validate_usage(current: &Usage, previous: Option<&Usage>) -> Result<(), ModelError> {
    current
        .validate()
        .map_err(|_| ModelError::validation(MODEL_USAGE_INVALID, "model usage is invalid"))?;
    if let (Some(input), Some(output), Some(total)) = (
        current.input_tokens(),
        current.output_tokens(),
        current.total_tokens(),
    ) && input.checked_add(output) != Some(total)
    {
        return Err(ModelError::validation(
            MODEL_USAGE_INVALID,
            "model usage total is inconsistent",
        ));
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
        return Err(ModelError::validation(
            MODEL_USAGE_INVALID,
            "model usage regressed across cumulative snapshots",
        ));
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

fn validate_delta(value: &str) -> Result<(), ModelError> {
    if value.is_empty() || value.as_bytes().contains(&0) {
        return Err(ModelError::validation(
            MODEL_STREAM_LIMIT_EXCEEDED,
            "model stream delta must be non-empty UTF-8 without NUL",
        ));
    }
    Ok(())
}

fn add_bytes(total: &mut usize, add: usize, max: usize) -> Result<(), ModelError> {
    *total = total.checked_add(add).ok_or_else(stream_limit_error)?;
    if *total > max {
        return Err(stream_limit_error());
    }
    Ok(())
}

/// Counts canonical JSON bytes without materializing the encoded buffer.
fn canonical_byte_len<T: Serialize>(value: &T) -> Option<usize> {
    let mut writer = CountingWriter::default();
    serde_json_canonicalizer::to_writer(value, &mut writer).ok()?;
    Some(writer.len)
}

#[derive(Default)]
struct CountingWriter {
    len: usize,
}

impl std::io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.len = self.len.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn stream_limit_error() -> ModelError {
    ModelError::limit(
        MODEL_STREAM_LIMIT_EXCEEDED,
        "model stream exceeded a configured aggregate limit",
    )
}
