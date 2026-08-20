//! Human-in-the-loop elicitation implementation of the public `Toolset` port.
//!
//! Exposes tools that ask the user a question: a free-form `ask_user` tool
//! and typed per-workflow elicitation tools with response schemas fixed at
//! registration. Calls never complete inline — they park the run durably by
//! returning [`ELICITATION_INPUT_REQUIRED`] with the serialized
//! [`InteractionRequest`] in the error metadata; the user's resolution
//! becomes the tool result.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
    )
)]
#![doc(test(attr(allow(clippy::expect_used))))]

use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentRef, ContentBlock, EffectId, ErrorCategory, InteractionId,
    InteractionKind, InteractionRequest, Metadata, RawJson, RetrySafety, TextBlock,
    ToolExecutionMode, ToolId, ValidatedToolCall, Version,
};
use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, PortFuture, SideEffectClass, TOOL_INTERACTION_REQUIRED,
    ToolCallContext, ToolDeferralSupport, ToolError, ToolEventStream, ToolResult, ToolSpec,
    ToolStreamItem, Toolset, ToolsetDescriptor,
};
use futures_util::stream;
use serde::Deserialize;
use thiserror::Error;

const POLICY_COMPONENT: &str = "finstack.tools.elicitation";
const TOOL_ID_PREFIX: &str = "finstack.tools.elicitation";
const ASK_USER_NAME: &str = "ask_user";
const MAX_RESULT_BYTES: u64 = 65_536;

/// Stable park code. Aliased to [`TOOL_INTERACTION_REQUIRED`] so the runtime
/// journals the pending interaction and moves the run to `AwaitingInteraction`.
pub const ELICITATION_INPUT_REQUIRED: &str = TOOL_INTERACTION_REQUIRED;
/// Stable invalid-elicitation-argument code.
pub const ELICITATION_INVALID_ARGUMENTS: &str = "elicitation_invalid_arguments";

/// Elicitation toolset failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ElicitationError {
    /// A registered definition or checked-in constant is invalid.
    #[error("elicitation_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Interaction profile for one elicitation tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElicitationKind {
    /// Free-text answer.
    FreeText,
    /// Choice between fixed options.
    Choice,
    /// Structured answer against a JSON schema.
    Form,
}

impl From<ElicitationKind> for InteractionKind {
    fn from(kind: ElicitationKind) -> Self {
        match kind {
            ElicitationKind::FreeText => Self::FreeText,
            ElicitationKind::Choice => Self::Choice,
            ElicitationKind::Form => Self::Form,
        }
    }
}

/// Typed per-workflow elicitation tool definition.
///
/// The response schema is fixed at registration; the model can only supply
/// call-time context, never reshape the question contract.
#[derive(Debug, Clone)]
pub struct ElicitationToolDef {
    /// Tool name exposed to the model (also the tool id suffix).
    pub name: String,
    /// Human-readable title.
    pub title: String,
    /// Description shown to the model.
    pub description: String,
    /// Static question text shown to the user.
    pub prompt: String,
    /// Interaction profile.
    pub kind: ElicitationKind,
    /// JSON schema the user's answer must satisfy.
    pub response_schema: serde_json::Value,
}

/// Builder for [`ElicitationToolset`].
#[derive(Debug, Default)]
pub struct ElicitationToolsetBuilder {
    ask_user: bool,
    tools: Vec<ElicitationToolDef>,
}

impl ElicitationToolsetBuilder {
    /// Expose the free-form `ask_user` tool.
    #[must_use]
    pub fn with_ask_user(mut self) -> Self {
        self.ask_user = true;
        self
    }

    /// Register one typed elicitation tool.
    #[must_use]
    pub fn tool(mut self, def: ElicitationToolDef) -> Self {
        self.tools.push(def);
        self
    }

    /// Build the toolset and its cached public specifications.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when no tool is registered or a
    /// definition is invalid.
    pub fn build(self) -> Result<ElicitationToolset, ElicitationError> {
        if !self.ask_user && self.tools.is_empty() {
            return Err(ElicitationError::Configuration {
                reason: "no_tools_registered",
            });
        }
        let mut specs = Vec::with_capacity(self.tools.len() + usize::from(self.ask_user));
        let mut typed = Vec::with_capacity(self.tools.len());
        if self.ask_user {
            specs.push(ask_user_spec()?);
        }
        for def in &self.tools {
            let (spec, tool) = typed_tool(def)?;
            specs.push(spec);
            typed.push(tool);
        }
        let component = ComponentId::parse(POLICY_COMPONENT)
            .map(|id| ComponentRef::new(id, None))
            .map_err(|_| ElicitationError::Configuration {
                reason: "invalid_policy_component",
            })?;
        Ok(ElicitationToolset {
            component,
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-elicitation"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from(specs),
            typed: Arc::from(typed),
            ask_user: self.ask_user,
        })
    }
}

#[derive(Debug)]
struct TypedTool {
    name: Arc<str>,
    prompt: Arc<str>,
    kind: ElicitationKind,
    response_schema: RawJson,
}

/// Human-in-the-loop elicitation toolset.
#[derive(Debug, Clone)]
pub struct ElicitationToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    typed: Arc<[TypedTool]>,
    ask_user: bool,
    component: ComponentRef,
}

impl ElicitationToolset {
    /// Start building an elicitation toolset.
    #[must_use]
    pub fn builder() -> ElicitationToolsetBuilder {
        ElicitationToolsetBuilder::default()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AskUserKind {
    FreeText,
    Choice,
    Form,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskUserArguments {
    prompt: String,
    kind: Option<AskUserKind>,
    options: Option<Vec<String>>,
    response_schema: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct TypedArguments {
    context: Option<String>,
}

impl Toolset for ElicitationToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>> {
        let typed = Arc::clone(&self.typed);
        let ask_user = self.ask_user;
        let component = self.component.clone();
        Box::pin(async move {
            validate_call_context(&ctx, &call)?;
            let tool_name = call.call.tool_name().to_owned();
            // On resolution the runtime re-invokes the tool with the original
            // arguments merged with the response object. Both input schemas
            // forbid unknown properties, so a present `answer` key can only
            // come from the resolution overlay.
            if let Some(answer) = resumed_answer(call.call.arguments()) {
                return completed_answer(&answer);
            }
            let request = if ask_user && tool_name == ASK_USER_NAME {
                ask_user_request(ctx.run.effect_id, call.call.arguments(), component)?
            } else if let Some(tool) = typed.iter().find(|tool| tool.name.as_ref() == tool_name) {
                typed_request(ctx.run.effect_id, tool, call.call.arguments(), component)?
            } else {
                return Err(invalid_arguments("elicitation tool is not registered"));
            };
            Err(interaction_required_error(&request))
        })
    }
}

fn resumed_answer(arguments: &RawJson) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_slice(arguments.as_bytes()).ok()?;
    value.get("answer").cloned()
}

fn completed_answer(answer: &serde_json::Value) -> Result<ToolEventStream, ToolError> {
    let output = serde_json_canonicalizer::to_vec(&serde_json::json!({ "answer": answer }))
        .map_err(|_| invalid_arguments("elicitation answer is not json"))?;
    let result = ToolResult {
        output: RawJson::parse(output)
            .map_err(|_| invalid_arguments("elicitation answer normalization failed"))?,
        is_error: false,
    };
    Ok(Box::pin(stream::once(async move {
        Ok(ToolStreamItem::Completed(result))
    })) as ToolEventStream)
}

/// Wrap one answer-value schema under the reserved `answer` response key.
fn wrap_answer_schema(schema: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {"answer": schema},
        "required": ["answer"]
    })
}

/// Reconstruct the parked HITL request from [`ELICITATION_INPUT_REQUIRED`].
#[must_use]
pub fn interaction_request_from_tool_error(error: &ToolError) -> Option<InteractionRequest> {
    if error.code() != ELICITATION_INPUT_REQUIRED {
        return None;
    }
    serde_json::from_slice(error.metadata().as_bytes()).ok()
}

fn ask_user_request(
    effect_id: EffectId,
    arguments: &RawJson,
    component: ComponentRef,
) -> Result<InteractionRequest, ToolError> {
    let arguments: AskUserArguments = serde_json::from_slice(arguments.as_bytes())
        .map_err(|_| invalid_arguments("ask_user arguments are invalid"))?;
    let kind = arguments.kind.unwrap_or(AskUserKind::FreeText);
    let (interaction_kind, schema_value) = match kind {
        AskUserKind::FreeText => (
            InteractionKind::FreeText,
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"]
            }),
        ),
        AskUserKind::Choice => {
            let options = arguments
                .options
                .filter(|options| !options.is_empty())
                .ok_or_else(|| invalid_arguments("choice elicitation requires options"))?;
            (
                InteractionKind::Choice,
                serde_json::json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"answer": {"type": "string", "enum": options}},
                    "required": ["answer"]
                }),
            )
        }
        AskUserKind::Form => {
            let schema = arguments
                .response_schema
                .filter(serde_json::Value::is_object)
                .ok_or_else(|| invalid_arguments("form elicitation requires a response schema"))?;
            (InteractionKind::Form, wrap_answer_schema(&schema))
        }
    };
    build_request(
        effect_id,
        interaction_kind,
        prompt_blocks(&arguments.prompt, None)?,
        canonical_schema(&schema_value)?,
        component,
    )
}

fn typed_request(
    effect_id: EffectId,
    tool: &TypedTool,
    arguments: &RawJson,
    component: ComponentRef,
) -> Result<InteractionRequest, ToolError> {
    let arguments: TypedArguments = serde_json::from_slice(arguments.as_bytes())
        .map_err(|_| invalid_arguments("elicitation arguments are invalid"))?;
    build_request(
        effect_id,
        tool.kind.into(),
        prompt_blocks(tool.prompt.as_ref(), arguments.context.as_deref())?,
        // Wrapped and canonicalized once at registration; pass the bytes through.
        tool.response_schema.clone(),
        component,
    )
}

fn canonical_schema(schema_value: &serde_json::Value) -> Result<RawJson, ToolError> {
    RawJson::parse(
        serde_json_canonicalizer::to_vec(schema_value)
            .map_err(|_| invalid_arguments("response schema is not json"))?,
    )
    .map_err(|_| invalid_arguments("response schema is not raw json"))
}

fn prompt_blocks(prompt: &str, context: Option<&str>) -> Result<Vec<ContentBlock>, ToolError> {
    let mut blocks = vec![ContentBlock::Text(
        TextBlock::try_new(prompt).map_err(|_| invalid_arguments("prompt is invalid"))?,
    )];
    if let Some(context) = context.filter(|context| !context.is_empty()) {
        blocks.push(ContentBlock::Text(
            TextBlock::try_new(context).map_err(|_| invalid_arguments("context is invalid"))?,
        ));
    }
    Ok(blocks)
}

fn build_request(
    effect_id: EffectId,
    kind: InteractionKind,
    prompt: Vec<ContentBlock>,
    response_schema: RawJson,
    component: ComponentRef,
) -> Result<InteractionRequest, ToolError> {
    InteractionRequest::try_new(
        1,
        InteractionId::from_bytes(effect_id.to_bytes()),
        effect_id,
        kind,
        prompt,
        response_schema,
        component,
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        None,
        false,
        Metadata::empty(),
    )
    .map_err(|_| invalid_arguments("elicitation request is invalid"))
}

fn ask_user_spec() -> Result<ToolSpec, ElicitationError> {
    let input_schema = RawJson::parse(
        br#"{"additionalProperties":false,"properties":{"kind":{"description":"Interaction profile; defaults to free_text.","enum":["free_text","choice","form"],"type":"string"},"options":{"description":"Allowed answers; required when kind is choice.","items":{"type":"string"},"minItems":1,"type":"array"},"prompt":{"description":"Question shown to the user.","minLength":1,"type":"string"},"response_schema":{"description":"JSON schema the answer must satisfy; required when kind is form.","type":"object"}},"required":["prompt"],"type":"object"}"#,
    )
    .map_err(|_| ElicitationError::Configuration {
        reason: "invalid_ask_user_input_schema",
    })?;
    spec(
        ASK_USER_NAME,
        "Ask the user",
        "Ask the user a question and wait for their answer. Use when required \
         information is missing or a decision needs human input.",
        input_schema,
    )
}

fn typed_tool(def: &ElicitationToolDef) -> Result<(ToolSpec, TypedTool), ElicitationError> {
    if def.name == ASK_USER_NAME {
        return Err(ElicitationError::Configuration {
            reason: "typed_tool_name_reserved",
        });
    }
    let response_schema = if def.response_schema.is_object() {
        RawJson::parse(
            serde_json_canonicalizer::to_vec(&wrap_answer_schema(&def.response_schema)).map_err(
                |_| ElicitationError::Configuration {
                    reason: "invalid_response_schema",
                },
            )?,
        )
        .map_err(|_| ElicitationError::Configuration {
            reason: "invalid_response_schema",
        })?
    } else {
        return Err(ElicitationError::Configuration {
            reason: "response_schema_must_be_object",
        });
    };
    let input_schema = RawJson::parse(
        br#"{"additionalProperties":false,"properties":{"context":{"description":"Call-specific details shown to the user beneath the registered prompt.","type":"string"}},"type":"object"}"#,
    )
    .map_err(|_| ElicitationError::Configuration {
        reason: "invalid_typed_input_schema",
    })?;
    let spec = spec(&def.name, &def.title, &def.description, input_schema)?;
    let tool = TypedTool {
        name: Arc::from(def.name.as_str()),
        prompt: Arc::from(def.prompt.as_str()),
        kind: def.kind,
        response_schema,
    };
    Ok((spec, tool))
}

fn spec(
    name: &str,
    title: &str,
    description: &str,
    input_schema: RawJson,
) -> Result<ToolSpec, ElicitationError> {
    let id = ToolId::parse(format!("{TOOL_ID_PREFIX}.{name}")).map_err(|_| {
        ElicitationError::Configuration {
            reason: "invalid_tool_name",
        }
    })?;
    let spec = ToolSpec {
        id,
        model_name: Arc::from(name),
        title: Arc::from(title),
        description: Arc::from(description),
        input_schema,
        output_schema: None,
        execution: ToolExecutionMode::Sequential,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            // The interaction itself is the human touchpoint; gating it
            // behind a second approval interaction would double-prompt.
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: MAX_RESULT_BYTES,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate()
        .map_err(|_| ElicitationError::Configuration {
            reason: "invalid_tool_spec",
        })?;
    Ok(spec)
}

fn validate_call_context(ctx: &ToolCallContext, call: &ValidatedToolCall) -> Result<(), ToolError> {
    if !call
        .tool_id
        .as_str()
        .starts_with(&format!("{TOOL_ID_PREFIX}."))
    {
        return Err(invalid_arguments("elicitation call identity is invalid"));
    }
    let locator_scope = ctx.run.locator.tenant_scope.as_ref();
    if ctx
        .run
        .authorization
        .principal
        .tenant_scope()
        .is_some_and(|scope| scope != locator_scope)
    {
        return Err(invalid_arguments(
            "elicitation principal scope does not match the committed effect",
        ));
    }
    Ok(())
}

fn interaction_required_error(request: &InteractionRequest) -> ToolError {
    let metadata = serde_json::to_vec(request)
        .ok()
        .and_then(|bytes| Metadata::parse(bytes).ok())
        .unwrap_or_else(Metadata::empty);
    ToolError::try_new(
        ELICITATION_INPUT_REQUIRED,
        ErrorCategory::Tool,
        false,
        "elicitation tool requested user input",
        metadata,
    )
    .unwrap_or_else(Into::into)
}

fn invalid_arguments(message: &'static str) -> ToolError {
    ToolError::try_new(
        ELICITATION_INVALID_ARGUMENTS,
        ErrorCategory::Validation,
        false,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
