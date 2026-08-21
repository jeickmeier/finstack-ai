//! Private Gemini `generateContent` request translation.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine;
use finstack_ai_kernel::{
    ContentBlock, ErrorCategory, MediaRef, Message, MessageRole, OutputSpec, RawJson,
    SUBMIT_FINAL_OUTPUT_TOOL, ToolCallId,
};
use finstack_ai_provider_wire::GEMINI_CONTINUATION_PROVIDER;
use finstack_ai_runtime::{
    ModelError, ModelRequestDraft, ResolvedMedia, ToolSpec, thinking_level_budget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::GeminiModelConfig;
use crate::error::{GEMINI_REQUEST_INVALID, error};

const CONTINUATION_VERSION: u64 = 1;
const CANDIDATE_COUNT: u32 = 1;
const JSON_MIME_TYPE: &str = "application/json";
const RESERVED_SETTINGS: &[&str] = &[
    "model",
    "contents",
    "systemInstruction",
    "system_instruction",
    "tools",
    "toolConfig",
    "tool_config",
    "stream",
    "generationConfig",
    "generation_config",
    "cachedContent",
    "cached_content",
    "responseSchema",
    "responseJsonSchema",
    "responseMimeType",
];

pub(crate) fn request_error(message: &'static str) -> ModelError {
    error(
        GEMINI_REQUEST_INVALID,
        ErrorCategory::Validation,
        false,
        message,
    )
}

/// One `generateContent` request body.
///
/// `contents` stays as raw JSON values so continuation `replay_contents`
/// (which carry opaque `thoughtSignature` material) round-trip verbatim.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateContentRequest {
    contents: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<WireContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_content: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

/// Deliberately opaque: the body carries replayed `thoughtSignature`
/// material and user content that must never reach logs (SEC-INV-005).
impl core::fmt::Debug for GenerateContentRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("GenerateContentRequest { [REDACTED] }")
    }
}

#[derive(Serialize)]
struct WireContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    parts: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    max_output_tokens: u64,
    candidate_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_json_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    thinking_budget: u64,
    include_thoughts: bool,
}

#[derive(Deserialize)]
struct ContinuationEnvelope {
    provider: String,
    version: u64,
    replay_contents: Vec<Value>,
}

/// Name and provider call id captured from an earlier `ToolCall` block.
struct CallIdentity {
    name: String,
    provider_call_id: Option<String>,
}

impl GenerateContentRequest {
    pub(crate) fn try_from_draft(
        draft: &ModelRequestDraft,
        model: &GeminiModelConfig,
        continuation: Option<&RawJson>,
        resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    ) -> Result<Self, ModelError> {
        draft.validate()?;
        if draft.model != *model.name() {
            return Err(request_error("requested model is not configured"));
        }
        let max_output_tokens = draft
            .limits
            .max_output_tokens
            .min(model.max_output_tokens());
        let (system_instruction, contents) =
            map_messages(&draft.messages, continuation, resolved, model)?;

        let mut settings = parse_settings(&draft.settings.values)?;
        for reserved in RESERVED_SETTINGS {
            if settings.remove(*reserved).is_some() {
                return Err(request_error("provider settings contain a reserved field"));
            }
        }
        let thinking_config = take_thinking(&mut settings, model, max_output_tokens)?;
        let cached_content = take_cached_content(&mut settings)?;
        let native_tools = take_native_tools(&mut settings)?;

        let (declarations, response_json_schema) = map_tools(&draft.tools, &draft.output)?;
        let mut tools = Vec::with_capacity(native_tools.len().saturating_add(1));
        if !declarations.is_empty() {
            tools.push(json!({ "functionDeclarations": declarations }));
        }
        tools.extend(native_tools);

        Ok(Self {
            contents,
            system_instruction,
            tools: (!tools.is_empty()).then_some(tools),
            generation_config: Some(GenerationConfig {
                max_output_tokens,
                candidate_count: CANDIDATE_COUNT,
                response_mime_type: response_json_schema.is_some().then_some(JSON_MIME_TYPE),
                response_json_schema,
                thinking_config,
            }),
            cached_content,
            extra: settings,
        })
    }

    pub(crate) fn serialize(&self) -> Result<Vec<u8>, ModelError> {
        serde_json::to_vec(self).map_err(|_| request_error("provider request could not be encoded"))
    }
}

fn parse_settings(settings: &RawJson) -> Result<BTreeMap<String, Value>, ModelError> {
    let Value::Object(values) = raw_value(settings)? else {
        return Err(request_error("provider settings must be a JSON object"));
    };
    Ok(values.into_iter().collect())
}

fn raw_value(value: &RawJson) -> Result<Value, ModelError> {
    serde_json::from_slice(value.as_bytes())
        .map_err(|_| request_error("canonical provider JSON could not be decoded"))
}

/// `gemini.thinking` > `thinking_level` > model default, with the budget
/// always strictly below the effective `maxOutputTokens`.
fn take_thinking(
    settings: &mut BTreeMap<String, Value>,
    model: &GeminiModelConfig,
    max_output_tokens: u64,
) -> Result<Option<ThinkingConfig>, ModelError> {
    let explicit = settings.remove("gemini.thinking");
    let level = settings.remove("thinking_level");
    // Explicit settings are honored regardless of the model-config flag (the
    // linked-surface convention shared with the Anthropic leaf); the flag only
    // drives the default-on path and capability advertising.
    let budget = if let Some(value) = explicit {
        thinking_budget(&value)?
    } else if let Some(level) = level {
        level
            .as_str()
            .and_then(thinking_level_budget)
            .ok_or_else(|| request_error("thinking_level is not an allowlisted value"))?
    } else if model.thinking() {
        model.thinking_budget_tokens()
    } else {
        return Ok(None);
    };
    if budget >= max_output_tokens {
        return Err(request_error("thinking budget exceeds maxOutputTokens"));
    }
    Ok(Some(ThinkingConfig {
        thinking_budget: budget,
        include_thoughts: true,
    }))
}

fn thinking_budget(value: &Value) -> Result<u64, ModelError> {
    let budget = match value {
        Value::Object(config) => config.get("thinkingBudget").and_then(Value::as_u64),
        other => other.as_u64(),
    };
    budget.ok_or_else(|| request_error("gemini.thinking must carry a thinkingBudget"))
}

fn take_cached_content(
    settings: &mut BTreeMap<String, Value>,
) -> Result<Option<String>, ModelError> {
    let Some(value) = settings.remove("gemini.cached_content") else {
        return Ok(None);
    };
    let Value::String(name) = value else {
        return Err(request_error("gemini.cached_content must be a string"));
    };
    Ok(Some(name))
}

// Explicit native-tool settings are honored without model-flag gating; the
// flags exist to advertise capabilities, not to veto host settings.
fn take_native_tools(settings: &mut BTreeMap<String, Value>) -> Result<Vec<Value>, ModelError> {
    let mut tools = Vec::new();
    if let Some(config) = take_native_tool(settings, "gemini.google_search")? {
        tools.push(json!({ "googleSearch": config }));
    }
    if let Some(config) = take_native_tool(settings, "gemini.code_execution")? {
        tools.push(json!({ "codeExecution": config }));
    }
    Ok(tools)
}

fn take_native_tool(
    settings: &mut BTreeMap<String, Value>,
    key: &str,
) -> Result<Option<Value>, ModelError> {
    match settings.remove(key) {
        None | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(json!({}))),
        Some(Value::Object(config)) => Ok(Some(Value::Object(config))),
        Some(_) => Err(request_error(
            "gemini native tool setting must be a boolean or an object",
        )),
    }
}

/// Function declarations plus the structured-output schema, if any.
fn map_tools(
    tools: &[ToolSpec],
    output: &OutputSpec,
) -> Result<(Vec<Value>, Option<Value>), ModelError> {
    let mut declarations = Vec::with_capacity(tools.len());
    let mut response_json_schema = None;
    for tool in tools {
        if tool.model_name.as_ref() == SUBMIT_FINAL_OUTPUT_TOOL {
            let OutputSpec::JsonSchema { schema } = output else {
                return Err(request_error(
                    "native structured output does not accept the prompted-mode tool",
                ));
            };
            if tool.input_schema.digest() != schema.schema_digest {
                return Err(request_error(
                    "structured-output schema does not match the committed schema reference",
                ));
            }
            response_json_schema = Some(raw_value(&tool.input_schema)?);
            continue;
        }
        declarations.push(json!({
            "name": tool.model_name.as_ref(),
            "description": tool.description.as_ref(),
            "parameters": raw_value(&tool.input_schema)?
        }));
    }
    if matches!(output, OutputSpec::JsonSchema { .. }) && response_json_schema.is_none() {
        return Err(request_error(
            "structured output requires the matching framework schema tool",
        ));
    }
    Ok((declarations, response_json_schema))
}

fn map_messages(
    messages: &[Message],
    continuation: Option<&RawJson>,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &GeminiModelConfig,
) -> Result<(Option<WireContent>, Vec<Value>), ModelError> {
    let prefix_len = messages
        .iter()
        .take_while(|message| {
            matches!(message.role(), MessageRole::System | MessageRole::Developer)
        })
        .count();
    if messages[prefix_len..]
        .iter()
        .any(|message| matches!(message.role(), MessageRole::System | MessageRole::Developer))
    {
        return Err(request_error(
            "system instructions must remain a stable leading prefix",
        ));
    }
    let mut instruction_parts = Vec::with_capacity(prefix_len);
    for message in &messages[..prefix_len] {
        let text = render_text(message.content())?;
        if !text.is_empty() {
            instruction_parts.push(json!({ "text": text }));
        }
    }
    let system_instruction = if instruction_parts.is_empty() {
        None
    } else {
        Some(WireContent {
            role: None,
            parts: instruction_parts,
        })
    };

    let identities = call_identities(messages);
    let mut contents = Vec::new();
    // On continuation, the envelope splices over ONLY the assistant turn it
    // replays (the last one). Everything before and after it still maps from
    // the kernel transcript: `generateContent` is stateless, so dropping the
    // earlier user turns would erase the request the model is answering.
    let replaced = match continuation {
        Some(_) => messages
            .iter()
            .rposition(|message| message.role() == MessageRole::Assistant),
        None => None,
    };
    for (index, message) in messages.iter().enumerate().skip(prefix_len) {
        if replaced == Some(index) {
            if let Some(state) = continuation {
                contents.extend(parse_continuation(state)?.replay_contents);
            }
            continue;
        }
        contents.extend(map_conversation_message(
            message,
            &identities,
            resolved,
            model,
        )?);
    }
    if let Some(state) = continuation
        && replaced.is_none()
    {
        // No assistant turn to replace (deferred/recovery edge): replay first,
        // then the mapped conversation.
        let mut spliced = parse_continuation(state)?.replay_contents;
        spliced.append(&mut contents);
        contents = spliced;
    }
    Ok((system_instruction, contents))
}

fn parse_continuation(state: &RawJson) -> Result<ContinuationEnvelope, ModelError> {
    let envelope: ContinuationEnvelope = serde_json::from_slice(state.as_bytes())
        .map_err(|_| request_error("continuation state is invalid"))?;
    if envelope.provider != GEMINI_CONTINUATION_PROVIDER || envelope.version != CONTINUATION_VERSION
    {
        return Err(request_error("continuation state is invalid"));
    }
    Ok(envelope)
}

fn call_identities(messages: &[Message]) -> BTreeMap<ToolCallId, CallIdentity> {
    let mut identities = BTreeMap::new();
    for message in messages {
        for block in message.content() {
            if let ContentBlock::ToolCall(call) = block {
                identities.insert(
                    *call.tool_call_id(),
                    CallIdentity {
                        name: call.tool_name().to_owned(),
                        provider_call_id: call.provider_call_id().map(str::to_owned),
                    },
                );
            }
        }
    }
    identities
}

fn map_conversation_message(
    message: &Message,
    identities: &BTreeMap<ToolCallId, CallIdentity>,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &GeminiModelConfig,
) -> Result<Vec<Value>, ModelError> {
    let (role, parts) = match message.role() {
        MessageRole::User => ("user", map_user_parts(message.content(), resolved, model)?),
        MessageRole::Assistant => ("model", map_assistant_parts(message.content())?),
        MessageRole::Tool => (
            "user",
            map_function_responses(message.content(), identities)?,
        ),
        MessageRole::System | MessageRole::Developer => {
            return Err(request_error(
                "system instructions must remain a stable leading prefix",
            ));
        }
    };
    if parts.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![json!({ "role": role, "parts": parts })])
}

fn map_user_parts(
    content: &[ContentBlock],
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
    model: &GeminiModelConfig,
) -> Result<Vec<Value>, ModelError> {
    let mut parts = Vec::new();
    let mut text = String::new();
    for block in content {
        match block {
            ContentBlock::Text(value) => text.push_str(value.text()),
            ContentBlock::Json(value) => text.push_str(value.value().as_str()),
            ContentBlock::Image(media) => {
                if !model.input_images() {
                    return Err(request_error("model is not configured for image input"));
                }
                flush_text(&mut parts, &mut text);
                parts.push(media_part(media, resolved)?);
            }
            ContentBlock::Audio(media) => {
                if !model.input_audio() {
                    return Err(request_error("model is not configured for audio input"));
                }
                flush_text(&mut parts, &mut text);
                parts.push(media_part(media, resolved)?);
            }
            ContentBlock::File(media) => {
                if !model.input_files() {
                    return Err(request_error("model is not configured for file input"));
                }
                flush_text(&mut parts, &mut text);
                parts.push(media_part(media, resolved)?);
            }
            ContentBlock::Opaque(_) => {}
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    flush_text(&mut parts, &mut text);
    Ok(parts)
}

fn flush_text(parts: &mut Vec<Value>, text: &mut String) {
    if text.is_empty() {
        return;
    }
    parts.push(json!({ "text": text.as_str() }));
    text.clear();
}

fn media_part(
    media: &MediaRef,
    resolved: &BTreeMap<Arc<str>, ResolvedMedia>,
) -> Result<Value, ModelError> {
    match resolved.get(media.blob().id()) {
        Some(ResolvedMedia::Url(uri)) => Ok(json!({
            "fileData": {
                "fileUri": uri.as_ref(),
                "mimeType": media.blob().media_type()
            }
        })),
        Some(ResolvedMedia::Bytes { media_type, bytes }) => Ok(json!({
            "inlineData": {
                "mimeType": media_type.as_ref(),
                "data": base64::engine::general_purpose::STANDARD.encode(bytes)
            }
        })),
        None => Err(request_error(
            "media content requires a configured media resolver",
        )),
    }
}

fn map_assistant_parts(content: &[ContentBlock]) -> Result<Vec<Value>, ModelError> {
    let mut parts = Vec::new();
    let mut text = String::new();
    for block in content {
        match block {
            ContentBlock::Text(value) => text.push_str(value.text()),
            ContentBlock::Json(value) => text.push_str(value.value().as_str()),
            ContentBlock::ToolCall(call) => {
                flush_text(&mut parts, &mut text);
                let mut function_call = serde_json::Map::new();
                if let Some(id) = call.provider_call_id() {
                    function_call.insert("id".to_owned(), json!(id));
                }
                function_call.insert("name".to_owned(), json!(call.tool_name()));
                function_call.insert("args".to_owned(), raw_value(call.arguments())?);
                parts.push(json!({ "functionCall": Value::Object(function_call) }));
            }
            ContentBlock::Opaque(_) => {}
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    flush_text(&mut parts, &mut text);
    Ok(parts)
}

fn map_function_responses(
    content: &[ContentBlock],
    identities: &BTreeMap<ToolCallId, CallIdentity>,
) -> Result<Vec<Value>, ModelError> {
    content
        .iter()
        .map(|block| {
            let ContentBlock::ToolResult(result) = block else {
                return Err(request_error("tool messages contain unsupported content"));
            };
            let identity = identities
                .get(result.tool_call_id())
                .ok_or_else(|| request_error("tool result does not match an earlier tool call"))?;
            let payload = render_text(result.content())?;
            let response = if result.is_error() {
                json!({ "error": payload })
            } else {
                json!({ "output": payload })
            };
            let mut function_response = serde_json::Map::new();
            if let Some(id) = identity.provider_call_id.as_deref() {
                function_response.insert("id".to_owned(), json!(id));
            }
            function_response.insert("name".to_owned(), json!(identity.name.as_str()));
            function_response.insert("response".to_owned(), response);
            Ok(json!({ "functionResponse": Value::Object(function_response) }))
        })
        .collect()
}

fn render_text(content: &[ContentBlock]) -> Result<String, ModelError> {
    let mut rendered = String::new();
    for block in content {
        match block {
            ContentBlock::Text(value) => rendered.push_str(value.text()),
            ContentBlock::Json(value) => rendered.push_str(value.value().as_str()),
            _ => {
                return Err(request_error(
                    "message contains unsupported provider content",
                ));
            }
        }
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use base64::Engine;
    use finstack_ai_kernel::{
        BlobRef, ContentBlock, JsonSchemaDraft, MediaRef, Message, MessageId, MessageRole,
        Metadata, OutputSpec, ProviderIds, RawJson, RetrySafety, SUBMIT_FINAL_OUTPUT_TOOL,
        SchemaRef, TextBlock, Timestamp, ToolCallBlock, ToolCallId, ToolExecutionMode, ToolId,
        ToolResultBlock,
    };
    use finstack_ai_runtime::{
        ApprovalMetadata, ApprovalRequirement, ModelName, ModelRequestLimits, ModelSettings,
        SideEffectClass, ToolSpec,
    };
    use serde_json::Value;

    const SIGNATURE: &str = "c2ln-fixture-001";
    const CALL_ID: &str = "01234567-89ab-7cde-89ab-0123456789ac";

    fn body(request: &GenerateContentRequest) -> Value {
        serde_json::from_slice(&request.serialize().expect("serialize")).expect("body")
    }

    #[test]
    fn reserved_settings_are_rejected() {
        for reserved in [
            br#"{"model":"shadow"}"#.as_slice(),
            br#"{"contents":[]}"#.as_slice(),
            br#"{"systemInstruction":{}}"#.as_slice(),
            br#"{"system_instruction":{}}"#.as_slice(),
            br#"{"tools":[]}"#.as_slice(),
            br#"{"toolConfig":{}}"#.as_slice(),
            br#"{"tool_config":{}}"#.as_slice(),
            br#"{"stream":true}"#.as_slice(),
            br#"{"generationConfig":{}}"#.as_slice(),
            br#"{"generation_config":{}}"#.as_slice(),
            br#"{"cachedContent":"x"}"#.as_slice(),
            br#"{"cached_content":"x"}"#.as_slice(),
            br#"{"responseSchema":{}}"#.as_slice(),
            br#"{"responseJsonSchema":{}}"#.as_slice(),
            br#"{"responseMimeType":"text/plain"}"#.as_slice(),
        ] {
            let error =
                GenerateContentRequest::try_from_draft(&draft(reserved), &model(), None, &media())
                    .expect_err("reserved setting must fail");
            assert_eq!(error.code(), GEMINI_REQUEST_INVALID);
        }

        let request = GenerateContentRequest::try_from_draft(
            &draft(br#"{"safetySettings":[{"category":"HARM_CATEGORY_HATE_SPEECH"}]}"#),
            &model(),
            None,
            &media(),
        )
        .expect("passthrough settings");
        let value = body(&request);
        assert_eq!(
            value["safetySettings"][0]["category"],
            "HARM_CATEGORY_HATE_SPEECH"
        );
    }

    #[test]
    fn system_prefix_becomes_system_instruction_and_late_system_errors() {
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([
            text_message(MessageRole::System, "Be brief."),
            text_message(MessageRole::Developer, " Stay factual."),
            text_message(MessageRole::User, "hello"),
        ]);
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("request");
        let value = body(&request);
        assert_eq!(value["systemInstruction"]["parts"][0]["text"], "Be brief.");
        assert_eq!(
            value["systemInstruction"]["parts"][1]["text"],
            " Stay factual."
        );
        assert!(value["systemInstruction"].get("role").is_none());
        assert_eq!(value["contents"][0]["role"], "user");
        assert_eq!(value["contents"][0]["parts"][0]["text"], "hello");

        draft.messages = Arc::from([
            text_message(MessageRole::User, "hello"),
            text_message(MessageRole::System, "late"),
        ]);
        let error = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect_err("late system message must fail");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);
    }

    #[test]
    fn tool_result_maps_to_function_response_with_name_lookup() {
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([
            text_message(MessageRole::User, "hello"),
            assistant_tool_call(),
            tool_result_message(),
        ]);
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("request");
        let value = body(&request);
        assert_eq!(value["contents"][1]["role"], "model");
        assert_eq!(
            value["contents"][1]["parts"][0]["functionCall"]["name"],
            "lookup"
        );
        assert_eq!(
            value["contents"][1]["parts"][0]["functionCall"]["id"],
            "fc-1"
        );
        let response = &value["contents"][2];
        assert_eq!(response["role"], "user");
        assert_eq!(response["parts"][0]["functionResponse"]["name"], "lookup");
        assert_eq!(response["parts"][0]["functionResponse"]["id"], "fc-1");
        assert_eq!(
            response["parts"][0]["functionResponse"]["response"]["output"],
            "ok"
        );

        let mut orphan = draft.clone();
        orphan.messages = Arc::from([
            text_message(MessageRole::User, "hello"),
            tool_result_message(),
        ]);
        let error = GenerateContentRequest::try_from_draft(&orphan, &model(), None, &media())
            .expect_err("orphan tool result must fail");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);
    }

    #[test]
    fn google_search_setting_appends_native_tool() {
        let mut draft = draft(br#"{"gemini.google_search":true}"#);
        draft.tools = Arc::from([tool("lookup")]);
        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_google_search(true),
            None,
            &media(),
        )
        .expect("request");
        let value = body(&request);
        let tools = value["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["functionDeclarations"][0]["name"], "lookup");
        assert!(tools[0]["functionDeclarations"][0]["parameters"].is_object());
        assert_eq!(tools[1]["googleSearch"], serde_json::json!({}));
        assert!(value.get("gemini.google_search").is_none());

        let mut object_form = draft.clone();
        object_form.settings.values =
            RawJson::parse(br#"{"gemini.google_search":{"dynamicThreshold":0.5}}"#)
                .expect("settings");
        let request = GenerateContentRequest::try_from_draft(
            &object_form,
            &model().with_google_search(true),
            None,
            &media(),
        )
        .expect("request");
        let value = body(&request);
        assert_eq!(value["tools"][1]["googleSearch"]["dynamicThreshold"], 0.5);
    }

    #[test]
    fn google_search_setting_is_honored_without_the_model_flag() {
        // Explicit settings win over the capability-advertising flag, matching
        // the Anthropic leaf's settings convention.
        let draft = draft(br#"{"gemini.google_search":true}"#);
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("explicit setting must be honored");
        let value = body(&request);
        assert_eq!(value["tools"][0]["googleSearch"], serde_json::json!({}));
    }

    #[test]
    fn code_execution_setting_appends_native_tool() {
        let draft = draft(br#"{"gemini.code_execution":true}"#);
        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_code_execution(true),
            None,
            &media(),
        )
        .expect("request");
        let value = body(&request);
        assert_eq!(value["tools"][0]["codeExecution"], serde_json::json!({}));

        // The flag is capability advertising, not a veto on explicit settings.
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("explicit setting must be honored without the flag");
        let value = body(&request);
        assert_eq!(value["tools"][0]["codeExecution"], serde_json::json!({}));
    }

    #[test]
    fn thinking_level_maps_to_budget_and_include_thoughts() {
        let mut draft = draft(br#"{"thinking_level":"medium"}"#);
        draft.limits.max_output_tokens = 8_192;
        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_thinking(true, 1_024),
            None,
            &media(),
        )
        .expect("request");
        let value = body(&request);
        assert_eq!(
            value["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            4_096
        );
        assert_eq!(
            value["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
        assert!(value.get("thinking_level").is_none());

        // Explicit override wins over the level and the model default.
        let mut override_draft = draft.clone();
        override_draft.settings.values = RawJson::parse(
            br#"{"gemini.thinking":{"thinkingBudget":2048},"thinking_level":"high"}"#,
        )
        .expect("settings");
        let request = GenerateContentRequest::try_from_draft(
            &override_draft,
            &model().with_thinking(true, 1_024),
            None,
            &media(),
        )
        .expect("request");
        let value = body(&request);
        assert_eq!(
            value["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            2_048
        );

        // Model default applies with no settings at all.
        let mut default_draft = draft.clone();
        default_draft.settings.values = RawJson::parse(b"{}").expect("settings");
        let request = GenerateContentRequest::try_from_draft(
            &default_draft,
            &model().with_thinking(true, 1_024),
            None,
            &media(),
        )
        .expect("request");
        let value = body(&request);
        assert_eq!(
            value["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            1_024
        );

        // Thinking off on the model still honors an explicit setting.
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("explicit thinking_level must be honored without the flag");
        let value = body(&request);
        assert_eq!(
            value["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            4_096
        );

        // A budget at or above maxOutputTokens is an error.
        let mut oversized = draft.clone();
        oversized.limits.max_output_tokens = 1_024;
        let error = GenerateContentRequest::try_from_draft(
            &oversized,
            &model().with_thinking(true, 1_024),
            None,
            &media(),
        )
        .expect_err("thinking budget must stay under maxOutputTokens");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);

        // No thinking configured at all leaves the knob off.
        let mut plain = draft.clone();
        plain.settings.values = RawJson::parse(b"{}").expect("settings");
        let request = GenerateContentRequest::try_from_draft(&plain, &model(), None, &media())
            .expect("request");
        let value = body(&request);
        assert!(value["generationConfig"].get("thinkingConfig").is_none());
    }

    #[test]
    fn structured_output_uses_response_json_schema() {
        let schema =
            RawJson::parse(br#"{"properties":{"answer":{"type":"integer"}},"type":"object"}"#)
                .expect("schema");
        let mut draft = draft(b"{}");
        draft.output = OutputSpec::JsonSchema {
            schema: SchemaRef {
                draft: JsonSchemaDraft::Draft202012,
                schema_version: 1,
                schema_digest: schema.digest(),
            },
        };
        draft.tools = Arc::from([schema_tool(schema.as_bytes())]);
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("request");
        let value = body(&request);
        assert_eq!(
            value["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            value["generationConfig"]["responseJsonSchema"]["properties"]["answer"]["type"],
            "integer"
        );
        assert!(
            value.get("tools").is_none(),
            "the framework schema tool must not reach functionDeclarations"
        );

        // The prompted-mode tool must not ride along on a plain-text request.
        let mut prompted = draft.clone();
        prompted.output = OutputSpec::PlainText;
        let error = GenerateContentRequest::try_from_draft(&prompted, &model(), None, &media())
            .expect_err("submit_final_output must be rejected for native structured output");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);

        // Structured output without the framework tool has no schema to send.
        let mut missing = draft.clone();
        missing.tools = Arc::from([]);
        let error = GenerateContentRequest::try_from_draft(&missing, &model(), None, &media())
            .expect_err("structured output requires the framework schema tool");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);
    }

    #[test]
    fn cached_content_setting_maps_to_body_field() {
        let draft = draft(br#"{"gemini.cached_content":"cachedContents/abc"}"#);
        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_cached_content(true),
            None,
            &media(),
        )
        .expect("request");
        let value = body(&request);
        assert_eq!(value["cachedContent"], "cachedContents/abc");
        assert!(value.get("gemini.cached_content").is_none());

        // The flag is capability advertising, not a veto on explicit settings.
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("explicit setting must be honored without the flag");
        let value = body(&request);
        assert_eq!(value["cachedContent"], "cachedContents/abc");
    }

    #[test]
    fn continuation_replays_contents_verbatim_and_appends_tail() {
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([
            text_message(MessageRole::System, "Be brief."),
            text_message(MessageRole::User, "hello"),
            assistant_tool_call(),
            tool_result_message(),
        ]);
        let continuation = RawJson::parse(
            format!(
                r#"{{"provider":"gemini.generate-content","replay_contents":[{{"parts":[{{"functionCall":{{"args":{{"q":1}},"id":"fc-1","name":"lookup"}},"thoughtSignature":"{SIGNATURE}"}}],"role":"model"}}],"version":1}}"#
            )
            .as_bytes(),
        )
        .expect("continuation");
        let request =
            GenerateContentRequest::try_from_draft(&draft, &model(), Some(&continuation), &media())
                .expect("request");
        let bytes = request.serialize().expect("serialize");
        let rendered = String::from_utf8(bytes.clone()).expect("utf-8");
        assert!(
            rendered.contains(SIGNATURE),
            "the thought signature must replay verbatim"
        );
        let value: Value = serde_json::from_slice(&bytes).expect("body");
        let contents = value["contents"].as_array().expect("contents");
        assert_eq!(
            contents.len(),
            3,
            "the replay splices over only the assistant turn it replaces"
        );
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(
            contents[0]["parts"][0]["text"], "hello",
            "earlier user turns must still reach the stateless wire"
        );
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["thoughtSignature"], SIGNATURE);
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"],
            "lookup"
        );
        assert_eq!(value["systemInstruction"]["parts"][0]["text"], "Be brief.");
    }

    #[test]
    fn continuation_provider_mismatch_is_rejected() {
        let draft = draft(b"{}");
        for state in [
            br#"{"provider":"openai.responses","replay_contents":[],"version":1}"#.as_slice(),
            br#"{"provider":"gemini.generate-content","replay_contents":[],"version":2}"#
                .as_slice(),
            br#"{"provider":"gemini.generate-content"}"#.as_slice(),
        ] {
            let continuation = RawJson::parse(state).expect("continuation");
            let error = GenerateContentRequest::try_from_draft(
                &draft,
                &model(),
                Some(&continuation),
                &media(),
            )
            .expect_err("continuation mismatch must fail");
            assert_eq!(error.code(), GEMINI_REQUEST_INVALID);
        }
    }

    #[test]
    fn image_url_maps_to_file_data_and_bytes_to_inline_data() {
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([media_message(ContentBlock::Image(media_ref(
            "blob-1",
            "image/png",
        )))]);

        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.png")),
        );
        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &resolved,
        )
        .expect("request");
        let value = body(&request);
        let parts = value["contents"][0]["parts"].as_array().expect("parts");
        assert_eq!(parts.len(), 1, "media-only content must omit empty text");
        assert_eq!(parts[0]["fileData"]["fileUri"], "https://cdn.example/a.png");
        assert_eq!(parts[0]["fileData"]["mimeType"], "image/png");

        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-1"),
            ResolvedMedia::Bytes {
                media_type: Arc::from("image/png"),
                bytes: Arc::from(&b"pngdata"[..]),
            },
        );
        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &resolved,
        )
        .expect("request");
        let value = body(&request);
        let part = &value["contents"][0]["parts"][0];
        assert_eq!(part["inlineData"]["mimeType"], "image/png");
        assert_eq!(
            part["inlineData"]["data"],
            base64::engine::general_purpose::STANDARD.encode(b"pngdata")
        );

        // The image flag gates image blocks.
        let error = GenerateContentRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("image input must require the model flag");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);

        // Unresolved media fails closed.
        let error = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_input_images(true),
            None,
            &media(),
        )
        .expect_err("unresolved media must fail");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);
    }

    #[test]
    fn video_media_type_rides_on_input_files_flag() {
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([media_message(ContentBlock::File(media_ref(
            "blob-v",
            "video/mp4",
        )))]);
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-v"),
            ResolvedMedia::Url(Arc::from("https://cdn.example/a.mp4")),
        );
        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_input_files(true),
            None,
            &resolved,
        )
        .expect("request");
        let value = body(&request);
        let part = &value["contents"][0]["parts"][0];
        assert_eq!(part["fileData"]["fileUri"], "https://cdn.example/a.mp4");
        assert_eq!(part["fileData"]["mimeType"], "video/mp4");

        let error = GenerateContentRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("video must require the input_files flag");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);
    }

    #[test]
    fn audio_rejected_when_flag_off() {
        let mut draft = draft(b"{}");
        draft.messages = Arc::from([media_message(ContentBlock::Audio(media_ref(
            "blob-a",
            "audio/mpeg",
        )))]);
        let mut resolved = BTreeMap::new();
        resolved.insert(
            Arc::from("blob-a"),
            ResolvedMedia::Bytes {
                media_type: Arc::from("audio/mpeg"),
                bytes: Arc::from(&b"mp3data"[..]),
            },
        );
        let error = GenerateContentRequest::try_from_draft(&draft, &model(), None, &resolved)
            .expect_err("audio input must require the model flag");
        assert_eq!(error.code(), GEMINI_REQUEST_INVALID);

        let request = GenerateContentRequest::try_from_draft(
            &draft,
            &model().with_input_audio(true),
            None,
            &resolved,
        )
        .expect("request");
        let value = body(&request);
        assert_eq!(
            value["contents"][0]["parts"][0]["inlineData"]["mimeType"],
            "audio/mpeg"
        );
    }

    #[test]
    fn candidate_count_is_pinned_to_one() {
        let request =
            GenerateContentRequest::try_from_draft(&draft(b"{}"), &model(), None, &media())
                .expect("request");
        let value = body(&request);
        assert_eq!(value["generationConfig"]["candidateCount"], 1);
    }

    #[test]
    fn max_output_tokens_is_the_draft_model_minimum_and_opaque_blocks_drop() {
        let mut draft = draft(b"{}");
        draft.limits.max_output_tokens = 2_048;
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("request");
        assert_eq!(body(&request)["generationConfig"]["maxOutputTokens"], 2_048);

        draft.limits.max_output_tokens = 32_768;
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("request");
        assert_eq!(body(&request)["generationConfig"]["maxOutputTokens"], 8_192);

        let opaque = finstack_ai_kernel::OpaqueBlock::try_new(
            "application/vnd.finstack.gemini.grounding-metadata",
            finstack_ai_kernel::OpaquePayload::json(
                RawJson::parse(br#"{"webSearchQueries":["rust"]}"#).expect("payload"),
            ),
        )
        .expect("opaque");
        draft.limits.max_output_tokens = 2_048;
        draft.messages = Arc::from([
            text_message(MessageRole::User, "hello"),
            assistant_message(vec![
                ContentBlock::Text(TextBlock::try_new("answer").expect("text")),
                ContentBlock::Opaque(opaque),
            ]),
        ]);
        let request = GenerateContentRequest::try_from_draft(&draft, &model(), None, &media())
            .expect("request");
        let value = body(&request);
        let parts = value["contents"][1]["parts"].as_array().expect("parts");
        assert_eq!(parts.len(), 1, "outbound opaque blocks are dropped");
        assert_eq!(parts[0]["text"], "answer");
    }

    fn media() -> BTreeMap<Arc<str>, ResolvedMedia> {
        BTreeMap::new()
    }

    fn model() -> GeminiModelConfig {
        GeminiModelConfig::try_new("fixture-model", 1_000_000, 128_000, 8_192).expect("model")
    }

    fn media_ref(id: &str, media_type: &str) -> MediaRef {
        MediaRef::new(BlobRef::try_new(id, media_type, 4, None, None::<&str>).expect("blob"))
    }

    fn message(role: MessageRole, content: Vec<ContentBlock>) -> Message {
        Message::try_new(
            MessageId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("message id"),
            role,
            content,
            Timestamp::from_unix_ms(0).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    fn text_message(role: MessageRole, text: &str) -> Message {
        message(
            role,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        )
    }

    fn media_message(block: ContentBlock) -> Message {
        message(MessageRole::User, vec![block])
    }

    fn assistant_message(content: Vec<ContentBlock>) -> Message {
        message(MessageRole::Assistant, content)
    }

    fn assistant_tool_call() -> Message {
        assistant_message(vec![ContentBlock::ToolCall(
            ToolCallBlock::try_new_with_provider_call_id(
                ToolCallId::parse(CALL_ID).expect("tool call id"),
                "lookup",
                RawJson::parse(br#"{"q":1}"#).expect("args"),
                Some("fc-1"),
            )
            .expect("call"),
        )])
    }

    fn tool_result_message() -> Message {
        message(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult(
                ToolResultBlock::try_new(
                    ToolCallId::parse(CALL_ID).expect("tool call id"),
                    vec![ContentBlock::Text(TextBlock::try_new("ok").expect("text"))],
                    false,
                )
                .expect("result"),
            )],
        )
    }

    fn tool(name: &str) -> ToolSpec {
        schema_tool_named(name, br#"{"properties":{},"type":"object"}"#)
    }

    fn schema_tool(schema: &[u8]) -> ToolSpec {
        schema_tool_named(SUBMIT_FINAL_OUTPUT_TOOL, schema)
    }

    fn schema_tool_named(name: &str, schema: &[u8]) -> ToolSpec {
        ToolSpec {
            id: ToolId::parse("finstack.tools.fixture").expect("tool id"),
            model_name: Arc::from(name),
            title: Arc::from("Fixture tool"),
            description: Arc::from("A deterministic fixture tool."),
            input_schema: RawJson::parse(schema).expect("schema"),
            output_schema: None,
            execution: ToolExecutionMode::Sequential,
            side_effect: SideEffectClass::ReadOnly,
            retry_safety: RetrySafety::SafeToRetry,
            approval: ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
            max_result_bytes: 1_024,
            metadata: Metadata::empty(),
            deferral: finstack_ai_runtime::ToolDeferralSupport::Never,
        }
    }

    fn draft(settings: &[u8]) -> ModelRequestDraft {
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages: Arc::from([text_message(MessageRole::User, "hello")]),
            tools: Arc::from([]),
            output: OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(settings).expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 100_000,
                max_output_tokens: 2_048,
            },
        }
    }
}
