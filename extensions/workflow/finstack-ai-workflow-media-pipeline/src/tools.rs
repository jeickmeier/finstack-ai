//! The media pipeline exposed as three tools: `render_movie` (submit and tick
//! once), `advance_render` (tick a submitted render), and `get_render_status`
//! (read-only progress).
//!
//! Every tool call is answered from the driver's own [`RenderState`]: none of
//! these tools keep independent state, so the pipeline's persisted progress
//! is the single source of truth for what an agent (or a resumed process)
//! sees.

use std::sync::Arc;

use finstack_ai_kernel::{
    ErrorCategory, Metadata, RawJson, RetrySafety, ToolExecutionMode, ToolId, ValidatedToolCall,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::{
    ApprovalMetadata, ApprovalRequirement, SideEffectClass, ToolDeferralSupport, ToolSpec,
};
use finstack_ai_runtime::ports::tool::{
    ToolCallContext, ToolError, ToolEventStream, ToolResult, ToolStreamItem, Toolset,
    ToolsetDescriptor, verify_authority,
};
use futures_util::stream;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::driver::{MEDIA_PIPELINE_INVALID_ARGUMENTS, MediaPipelineDriver, PipelineError};
use crate::state::{RenderState, SceneState};

const RENDER_MOVIE_NAME: &str = "render_movie";
const RENDER_MOVIE_ID: &str = "finstack.tools.render_movie";
const ADVANCE_RENDER_NAME: &str = "advance_render";
const ADVANCE_RENDER_ID: &str = "finstack.tools.advance_render";
const GET_RENDER_STATUS_NAME: &str = "get_render_status";
const GET_RENDER_STATUS_ID: &str = "finstack.tools.get_render_status";

/// Shared result schema for all three pipeline tools.
const RESULT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"final_artifact":{"type":"object"},"per_scene":{"items":{"additionalProperties":false,"properties":{"clip_artifact":{"type":"object"},"failure":{"type":"string"},"id":{"type":"string"},"job_id":{"type":"string"},"stage":{"type":"string"}},"required":["id","stage"],"type":"object"},"type":"array"},"render_id":{"type":"string"},"status":{"type":"string"},"transcript_srt_artifact":{"type":"object"},"transcript_vtt_artifact":{"type":"object"}},"required":["render_id","status","per_scene"],"type":"object"}"#;

/// `{"render_id": string}`, required.
const RENDER_ID_SCHEMA: &[u8] =
    br#"{"additionalProperties":false,"properties":{"render_id":{"type":"string"}},"required":["render_id"],"type":"object"}"#;

/// Exposes [`MediaPipelineDriver`] as `render_movie`, `advance_render`, and
/// `get_render_status`.
pub struct MediaPipelineToolset {
    descriptor: ToolsetDescriptor,
    tools: Arc<[ToolSpec]>,
    driver: Arc<MediaPipelineDriver>,
}

impl std::fmt::Debug for MediaPipelineToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaPipelineToolset")
            .field(
                "tools",
                &self.tools.iter().map(|spec| &spec.id).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl MediaPipelineToolset {
    /// Construct the toolset over an already-configured driver.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError::ConfigInvalid`] when a tool identity or one
    /// of the built-in [`ToolSpec`]s fails its own validation.
    pub fn try_new(driver: Arc<MediaPipelineDriver>) -> Result<Self, PipelineError> {
        let result_schema =
            RawJson::parse(RESULT_SCHEMA).map_err(|_| PipelineError::ConfigInvalid {
                reason: "invalid_output_schema",
            })?;
        let tools = Arc::from([
            tool_spec(
                RENDER_MOVIE_ID,
                RENDER_MOVIE_NAME,
                "Render movie",
                "Validate and start one MoviePlan render, then run one pipeline tick.",
                br#"{"additionalProperties":false,"properties":{"plan":{"type":"object"}},"required":["plan"],"type":"object"}"#,
                result_schema.clone(),
                false,
            )?,
            tool_spec(
                ADVANCE_RENDER_ID,
                ADVANCE_RENDER_NAME,
                "Advance render",
                "Run one bounded tick of a submitted render (generate frames, submit and poll \
                 jobs, download clips, compose when done).",
                RENDER_ID_SCHEMA,
                result_schema.clone(),
                false,
            )?,
            tool_spec(
                GET_RENDER_STATUS_ID,
                GET_RENDER_STATUS_NAME,
                "Get render status",
                "Read one render's current progress.",
                RENDER_ID_SCHEMA,
                result_schema,
                true,
            )?,
        ]);
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-media-pipeline"),
                metadata: Metadata::empty(),
            },
            tools,
            driver,
        })
    }
}

/// One validated pipeline tool spec. `read_only` selects the status tool's
/// side-effect and approval classes; the two mutating tools share the paid
/// generation policy.
fn tool_spec(
    id: &str,
    name: &str,
    title: &str,
    description: &str,
    input_schema: &[u8],
    output_schema: RawJson,
    read_only: bool,
) -> Result<ToolSpec, PipelineError> {
    let (side_effect, approval) = if read_only {
        (
            SideEffectClass::ReadOnly,
            ApprovalMetadata {
                requirement: ApprovalRequirement::NotRequired,
                reason: None,
                attributes: Metadata::empty(),
            },
        )
    } else {
        (
            SideEffectClass::NonIdempotentWrite,
            ApprovalMetadata {
                requirement: ApprovalRequirement::Policy,
                reason: Some(Arc::from("paid multi-scene media generation")),
                attributes: Metadata::empty(),
            },
        )
    };
    let spec = ToolSpec {
        id: ToolId::parse(id).map_err(|_| PipelineError::ConfigInvalid {
            reason: "invalid_tool_id",
        })?,
        model_name: Arc::from(name),
        title: Arc::from(title),
        description: Arc::from(description),
        input_schema: RawJson::parse(input_schema).map_err(|_| PipelineError::ConfigInvalid {
            reason: "invalid_input_schema",
        })?,
        output_schema: Some(output_schema),
        execution: ToolExecutionMode::Sequential,
        side_effect,
        retry_safety: RetrySafety::AtMostOnce,
        approval,
        max_result_bytes: 262_144,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate().map_err(|_| PipelineError::ConfigInvalid {
        reason: "invalid_tool_spec",
    })?;
    Ok(spec)
}

impl Toolset for MediaPipelineToolset {
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
        let tools = Arc::clone(&self.tools);
        let driver = Arc::clone(&self.driver);
        Box::pin(async move {
            verify_authority(&ctx)?;
            let name = call.call.tool_name();
            let known = tools
                .iter()
                .any(|spec| spec.id == call.tool_id && spec.model_name.as_ref() == name);
            if !known {
                return Err(tool_error(
                    MEDIA_PIPELINE_INVALID_ARGUMENTS,
                    ErrorCategory::Validation,
                    "media pipeline call identity is invalid",
                ));
            }
            let state = match name {
                RENDER_MOVIE_NAME => {
                    let args: RenderMovieArguments = parse_args(&call, name)?;
                    let plan_bytes = serde_json::to_vec(&args.plan).map_err(|_| {
                        tool_error(
                            MEDIA_PIPELINE_INVALID_ARGUMENTS,
                            ErrorCategory::Validation,
                            "render_movie plan could not be re-serialized",
                        )
                    })?;
                    let submitted = driver.submit_plan(&ctx, &plan_bytes).await?;
                    driver.advance(&ctx, submitted.render_id.as_ref()).await?
                }
                ADVANCE_RENDER_NAME => {
                    let args: RenderIdArguments = parse_args(&call, name)?;
                    driver.advance(&ctx, &args.render_id).await?
                }
                _ => {
                    let args: RenderIdArguments = parse_args(&call, name)?;
                    driver.status(ctx.run.locator.tenant_scope.as_ref(), &args.render_id)?
                }
            };
            completed_stream(&render_state_json(&state))
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderMovieArguments {
    plan: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderIdArguments {
    render_id: String,
}

fn parse_args<T: for<'de> Deserialize<'de>>(
    call: &ValidatedToolCall,
    tool: &str,
) -> Result<T, ToolError> {
    serde_json::from_slice(call.call.arguments().as_bytes()).map_err(|_| {
        tool_error(
            MEDIA_PIPELINE_INVALID_ARGUMENTS,
            ErrorCategory::Validation,
            format!("{tool} arguments are invalid"),
        )
    })
}

/// Shared result JSON for all three tools: stage/status names come from
/// their own serde snake_case serialization, and `None` fields are omitted.
fn render_state_json(state: &RenderState) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("render_id".to_owned(), json!(state.render_id.as_ref()));
    fields.insert("status".to_owned(), json!(state.status));
    let per_scene: Vec<Value> = state.scenes.iter().map(scene_json).collect();
    fields.insert("per_scene".to_owned(), Value::Array(per_scene));
    if let Some(artifact) = &state.final_artifact {
        fields.insert("final_artifact".to_owned(), json!(artifact));
    }
    if let Some(artifact) = &state.transcript_srt_artifact {
        fields.insert("transcript_srt_artifact".to_owned(), json!(artifact));
    }
    if let Some(artifact) = &state.transcript_vtt_artifact {
        fields.insert("transcript_vtt_artifact".to_owned(), json!(artifact));
    }
    Value::Object(fields)
}

fn scene_json(scene: &SceneState) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("id".to_owned(), json!(scene.scene_id));
    fields.insert("stage".to_owned(), json!(scene.stage));
    if let Some(job_id) = &scene.job_id {
        fields.insert("job_id".to_owned(), json!(job_id));
    }
    if let Some(artifact) = &scene.clip_artifact {
        fields.insert("clip_artifact".to_owned(), json!(artifact));
    }
    if let Some(failure) = &scene.failure {
        fields.insert("failure".to_owned(), json!(failure));
    }
    Value::Object(fields)
}

fn completed_stream(value: &Value) -> Result<ToolEventStream, ToolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| {
        tool_error(
            MEDIA_PIPELINE_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "result serialization failed",
        )
    })?;
    let output = RawJson::parse(bytes).map_err(|_| {
        tool_error(
            MEDIA_PIPELINE_INVALID_ARGUMENTS,
            ErrorCategory::Internal,
            "result normalization failed",
        )
    })?;
    let result = ToolResult {
        output,
        is_error: false,
    };
    Ok(Box::pin(stream::once(async move {
        Ok(ToolStreamItem::Completed(result))
    })) as ToolEventStream)
}

fn tool_error(code: &'static str, category: ErrorCategory, message: impl AsRef<str>) -> ToolError {
    ToolError::try_new(code, category, false, message, Metadata::empty()).unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::{
        Digest, EffectOutputContract, EffectOutputKind, ToolCallBlock, ToolCallId,
        ToolFailurePolicy, ValidatedToolCall,
    };
    use finstack_ai_runtime::ports::model::ToolSpec;
    use finstack_ai_runtime::ports::tool::{ToolStreamItem, Toolset};
    use futures_util::StreamExt;
    use serde_json::json;

    use super::MediaPipelineToolset;
    use crate::driver::test_support::{
        fixture_plan, harness, limits, prime_happy_path, stage, tool_context, url_only_plan,
    };

    fn spec_for(name: &str, tools: &[ToolSpec]) -> ToolSpec {
        tools
            .iter()
            .find(|spec| spec.model_name.as_ref() == name)
            .cloned()
            .expect("tool spec")
    }

    fn validated_call(spec: &ToolSpec, args: &serde_json::Value) -> ValidatedToolCall {
        let bytes = serde_json::to_vec(args).expect("json");
        ValidatedToolCall {
            call: ToolCallBlock::try_new(
                ToolCallId::from_bytes([6; 16]),
                spec.model_name.as_ref(),
                finstack_ai_kernel::RawJson::parse(bytes).expect("raw"),
            )
            .expect("call"),
            tool_id: spec.id.clone(),
            component: None,
            output_contract: EffectOutputContract {
                kind: EffectOutputKind::ToolResult,
                schema_version: 1,
                schema_digest: Digest::raw_json(b"{}"),
            },
            retry_safety: spec.retry_safety,
            deadline: None,
            execution: spec.execution,
            failure_policy: ToolFailurePolicy::ReturnToModel,
        }
    }

    async fn call_tool(
        toolset: &MediaPipelineToolset,
        name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let spec = spec_for(name, &toolset.tools());
        let call = validated_call(&spec, &args);
        let mut stream = toolset
            .call(tool_context(), call)
            .await
            .expect("call started");
        let item = stream.next().await.expect("item").expect("ok");
        let ToolStreamItem::Completed(result) = item else {
            panic!("expected completion");
        };
        assert!(!result.is_error, "tool returned an error result");
        serde_json::from_slice(result.output.as_bytes()).expect("json")
    }

    async fn call_tool_err(
        toolset: &MediaPipelineToolset,
        name: &str,
        args: serde_json::Value,
    ) -> finstack_ai_runtime::ports::tool::ToolError {
        let spec = spec_for(name, &toolset.tools());
        let call = validated_call(&spec, &args);
        match toolset.call(tool_context(), call).await {
            Ok(_) => panic!("call must fail"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn render_movie_submits_and_ticks_once() {
        let harness = harness(limits(), true);
        // `url_only_plan` pins every frame to a URL, so the one tick this
        // test drives goes straight to video submission (no image tool call).
        harness.media.push(
            "openrouter_generate_video",
            json!({ "id": "job-a", "status": "queued" }),
        );

        let toolset = MediaPipelineToolset::try_new(Arc::clone(&harness.driver)).expect("toolset");
        let plan: serde_json::Value = serde_json::from_slice(&url_only_plan(1)).expect("plan json");
        let result = call_tool(&toolset, "render_movie", json!({ "plan": plan })).await;

        assert_eq!(result["status"], "running");
        let per_scene = result["per_scene"].as_array().expect("per_scene");
        assert_eq!(per_scene.len(), 1);
        assert_eq!(per_scene[0]["stage"], "polling");
        assert_eq!(per_scene[0]["job_id"], "job-a");
    }

    #[tokio::test]
    async fn advance_render_progresses_to_completion() {
        let harness = harness(limits(), true);
        let store = harness.store.clone().expect("store");
        let image = stage(&store, &tool_context(), "frame-bytes").await;
        let clip_one = stage(&store, &tool_context(), "clip-one-bytes").await;
        let clip_two = stage(&store, &tool_context(), "clip-two-bytes").await;
        let final_clip = stage(&store, &tool_context(), "final-movie-bytes").await;
        prime_happy_path(&harness, &image, [&clip_one, &clip_two], &final_clip);

        let plan: serde_json::Value = serde_json::from_slice(&fixture_plan()).expect("plan json");
        let submitted = call_tool(
            &MediaPipelineToolset::try_new(Arc::clone(&harness.driver)).expect("toolset"),
            "render_movie",
            json!({ "plan": plan }),
        )
        .await;
        let render_id = submitted["render_id"]
            .as_str()
            .expect("render_id")
            .to_owned();

        let toolset = MediaPipelineToolset::try_new(Arc::clone(&harness.driver)).expect("toolset");
        let mut result = submitted;
        for _ in 0..12 {
            if result["status"] == "completed" {
                break;
            }
            result = call_tool(
                &toolset,
                "advance_render",
                json!({ "render_id": render_id }),
            )
            .await;
        }
        assert_eq!(result["status"], "completed", "{result:?}");
        assert!(result["final_artifact"].is_object());
        assert!(result["transcript_srt_artifact"].is_object());
        assert!(result["transcript_vtt_artifact"].is_object());
    }

    #[tokio::test]
    async fn get_render_status_is_read_only() {
        let harness = harness(limits(), true);
        let store = harness.store.clone().expect("store");
        let image = stage(&store, &tool_context(), "frame-bytes").await;
        let clip_one = stage(&store, &tool_context(), "clip-one-bytes").await;
        let clip_two = stage(&store, &tool_context(), "clip-two-bytes").await;
        let final_clip = stage(&store, &tool_context(), "final-movie-bytes").await;
        prime_happy_path(&harness, &image, [&clip_one, &clip_two], &final_clip);

        let toolset = MediaPipelineToolset::try_new(Arc::clone(&harness.driver)).expect("toolset");
        let plan: serde_json::Value = serde_json::from_slice(&fixture_plan()).expect("plan json");
        let mut result = call_tool(&toolset, "render_movie", json!({ "plan": plan })).await;
        let render_id = result["render_id"].as_str().expect("render_id").to_owned();
        for _ in 0..12 {
            if result["status"] == "completed" {
                break;
            }
            result = call_tool(
                &toolset,
                "advance_render",
                json!({ "render_id": render_id.clone() }),
            )
            .await;
        }
        assert_eq!(result["status"], "completed", "{result:?}");

        let queued_before = harness.media.queued() + harness.compose.queued();
        let first = call_tool(
            &toolset,
            "get_render_status",
            json!({ "render_id": render_id.clone() }),
        )
        .await;
        let second = call_tool(
            &toolset,
            "get_render_status",
            json!({ "render_id": render_id }),
        )
        .await;
        let queued_after = harness.media.queued() + harness.compose.queued();

        assert_eq!(first, second, "read-only status must be idempotent");
        assert_eq!(
            queued_before, queued_after,
            "reading status must not consume any queued tool result"
        );
    }

    #[tokio::test]
    async fn unknown_render_id_maps_to_not_found() {
        let harness = harness(limits(), true);
        let toolset = MediaPipelineToolset::try_new(Arc::clone(&harness.driver)).expect("toolset");
        let error = call_tool_err(
            &toolset,
            "get_render_status",
            json!({ "render_id": "render-does-not-exist" }),
        )
        .await;
        assert_eq!(error.code(), crate::driver::MEDIA_PIPELINE_NOT_FOUND);
    }
}
