//! Nested MCP sampling under an open parent tool (ADR-046).

use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, EffectCompleted, EffectOutputKind, EffectPurpose, EffectRelation,
    EffectTag, EventTag, KernelInput, Message, MessageRole, MessageTag, Metadata, ModelRef,
    ModelSettled, ModelSettlement, NestedModelKind, OutputSpec, RecordTag, RequestCompactionModel,
    TransitionEnv,
};

use crate::coordinator::CommitCoordinator;
use crate::model::{
    ModelCallContext, ModelRequest, ModelRequestDraft, ModelRequestLimits, ModelSettings,
    ModelStreamAssembler, ModelStreamLimits, ModelTerminal, validate_model_request,
};
use crate::run_types::RunHandleError;
use crate::tool::AssembledToolTerminal;
use crate::{
    CancellationSignal, Clock, MCP_SAMPLING_REQUIRED, MCP_SAMPLING_UNAVAILABLE, NestedSample,
    RandomSource, RawJson, RunCallContext, ToolCallContext, ToolError, ToolStreamAssembler,
    ToolStreamLimits,
};

use super::{SettlementSources, ToolDriverResult, model_handle_error, tool::process_tool_result};

const NESTED_SAMPLE_UNAVAILABLE: &str = "mcp_sampling_unavailable";

pub(super) async fn fulfill_nested_sample<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    driver_result: ToolDriverResult,
    sources: &SettlementSources<C, R>,
) -> Result<super::ToolResultDisposition, RunHandleError> {
    let Some(ports) = sources.nested_sampling() else {
        return process_tool_result(
            coordinator,
            ToolDriverResult {
                seed: driver_result.seed,
                result: Err(ToolError::stable(
                    MCP_SAMPLING_UNAVAILABLE,
                    "nested sampling requires a locked model and budget",
                )),
            },
            sources,
        )
        .await;
    };
    let error = match &driver_result.result {
        Err(error) if error.code() == MCP_SAMPLING_REQUIRED => error.clone(),
        _ => {
            return Err(RunHandleError::ToolSettlement {
                code: NESTED_SAMPLE_UNAVAILABLE,
            });
        }
    };
    let params = error
        .sampling_params()
        .ok_or(RunHandleError::ToolSettlement {
            code: "mcp_sampling_params_invalid",
        })?;
    let draft = draft_from_sampling(&params, &ports.profile)?;
    validate_model_request(ports.model.as_ref(), &draft, &ports.profile)
        .map_err(|error| model_handle_error(&error))?;
    commit_nested_request(coordinator, sources, &draft, &driver_result).await?;
    let sample = execute_nested_model(
        coordinator,
        sources,
        ports.model.as_ref(),
        &draft,
        &ports.cancellation,
        &driver_result,
    )
    .await?;
    let result = complete_parent_tool(ports, &driver_result, sample).await;
    process_tool_result(
        coordinator,
        ToolDriverResult {
            seed: driver_result.seed,
            result,
        },
        sources,
    )
    .await
}

async fn commit_nested_request<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    draft: &ModelRequestDraft,
    driver_result: &ToolDriverResult,
) -> Result<(), RunHandleError> {
    let request_json = draft
        .canonical_bytes()
        .map_err(|error| model_handle_error(&error))?;
    let request_json =
        RawJson::parse(request_json).map_err(|_| RunHandleError::ToolSettlement {
            code: crate::MODEL_REQUEST_INVALID,
        })?;
    coordinator
        .submit(
            nested_request_env(sources)?,
            KernelInput::RequestCompactionModel(RequestCompactionModel {
                request: request_json,
                component: None,
                relation: EffectRelation {
                    parent_effect_id: driver_result.seed.requested.effect_id(),
                    purpose: EffectPurpose::NestedModel {
                        kind: NestedModelKind::McpSampling,
                    },
                },
                output_contract: finstack_ai_kernel::EffectOutputContract {
                    kind: EffectOutputKind::ModelResponse,
                    schema_version: 1,
                    schema_digest: finstack_ai_kernel::Digest::raw_json(b"model-response-v1"),
                },
                retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
                deadline: driver_result.seed.requested.deadline(),
            }),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    Ok(())
}

async fn complete_parent_tool(
    ports: &super::NestedSamplingPorts,
    driver_result: &ToolDriverResult,
    sample: NestedSample,
) -> Result<AssembledToolTerminal, ToolError> {
    let resolved = ports
        .catalog
        .by_id(&driver_result.seed.call.tool_id)
        .cloned()
        .ok_or_else(|| ToolError::stable(MCP_SAMPLING_UNAVAILABLE, "tool resolution missing"))?;
    let context = ToolCallContext {
        run: RunCallContext {
            locator: driver_result.seed.locator.clone(),
            authorization: driver_result.seed.authorization.clone(),
            effect_id: driver_result.seed.requested.effect_id(),
            attempt: driver_result.seed.attempt,
            deadline: driver_result.seed.requested.deadline(),
            budget_scope_id: driver_result.seed.budget_scope_id,
            cancellation: ports.cancellation.child(),
        },
        tool_batch_id: driver_result.seed.tool_batch_id,
        tool_call_id: driver_result.seed.tool_call_id,
    };
    match resolved
        .toolset
        .complete_nested_sample(context, driver_result.seed.call.clone(), sample)
        .await
    {
        Ok(stream) => ToolStreamAssembler::new(ToolStreamLimits::default())
            .assemble(
                stream,
                resolved.output_validator.as_deref(),
                resolved.spec.max_result_bytes,
                resolved.spec.deferral,
            )
            .await
            .map(|assembled| AssembledToolTerminal {
                usage: assembled.usage,
                terminal: assembled.terminal,
            }),
        Err(error) => Err(error),
    }
}

async fn execute_nested_model<C: Clock, R: RandomSource>(
    coordinator: &mut CommitCoordinator,
    sources: &SettlementSources<C, R>,
    model: &dyn crate::Model,
    draft: &ModelRequestDraft,
    cancellation: &CancellationSignal,
    driver_result: &ToolDriverResult,
) -> Result<NestedSample, RunHandleError> {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .ok_or(RunHandleError::ToolSettlement {
            code: NESTED_SAMPLE_UNAVAILABLE,
        })?
        .clone();
    if !pending.requested.is_nested_model() {
        return Err(RunHandleError::ToolSettlement {
            code: NESTED_SAMPLE_UNAVAILABLE,
        });
    }
    let run = RunCallContext {
        effect_id: pending.requested.effect_id(),
        locator: driver_result.seed.locator.clone(),
        authorization: driver_result.seed.authorization.clone(),
        attempt: driver_result.seed.attempt,
        deadline: pending.requested.deadline(),
        budget_scope_id: driver_result.seed.budget_scope_id,
        cancellation: cancellation.child(),
    };
    let stream = model
        .request(ModelRequest {
            call: ModelCallContext {
                run,
                request_id: pending.model_request_id,
            },
            draft: draft.clone(),
            continuation_state: None,
        })
        .await
        .map_err(|error| model_handle_error(&error))?;
    let assembled = ModelStreamAssembler::new(ModelStreamLimits::default())
        .map_err(|error| model_handle_error(&error))?
        .assemble(stream)
        .await
        .map_err(|error| model_handle_error(&error))?;
    let ModelTerminal::Completed(response) = assembled.terminal else {
        return Err(RunHandleError::ToolSettlement {
            code: NESTED_SAMPLE_UNAVAILABLE,
        });
    };
    let output_bytes = serde_json_canonicalizer::to_vec(&response).map_err(|_| {
        RunHandleError::ModelSettlement {
            code: "model_response_serialize_failed",
        }
    })?;
    let output = RawJson::parse(&output_bytes).map_err(|_| RunHandleError::ModelSettlement {
        code: "model_response_output_invalid",
    })?;
    let completion = EffectCompleted::try_new(
        pending.requested.effect_id(),
        pending.requested.output_contract().clone(),
        output.clone(),
        Some(response.usage.clone()),
        Vec::new(),
        response.provider_ids.clone(),
        Some(response.completion_id.as_ref()),
        None,
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_effect_completion_invalid",
    })?;
    let message_id = sources.generate::<MessageTag>()?;
    let now = sources.now()?;
    let model_ref =
        ModelRef::try_new("mcp", "sampling").map_err(|_| RunHandleError::ModelSettlement {
            code: "model_reference_invalid",
        })?;
    let assistant_message = Message::try_new(
        message_id,
        MessageRole::Assistant,
        response.assistant_content.to_vec(),
        now,
        Some(model_ref),
        response.provider_ids,
        Metadata::empty(),
    )
    .map_err(|_| RunHandleError::ModelSettlement {
        code: "model_assistant_message_invalid",
    })?;
    coordinator
        .submit(
            nested_settlement_env(sources)?,
            KernelInput::ModelSettled(ModelSettled {
                turn_id: pending.turn_id,
                model_request_id: pending.model_request_id,
                outcome: ModelSettlement::Completed {
                    completion,
                    assistant_message,
                },
            }),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    Ok(NestedSample { output })
}

fn draft_from_sampling(
    params: &RawJson,
    profile: &crate::LockedModelContextProfile,
) -> Result<ModelRequestDraft, RunHandleError> {
    if profile.profile.hard_input_bytes == 0 || profile.profile.reserved_output_tokens == 0 {
        return Err(RunHandleError::Tool {
            code: Arc::from(MCP_SAMPLING_UNAVAILABLE),
        });
    }
    let available = profile
        .profile
        .context_window_tokens
        .saturating_sub(profile.profile.reserved_output_tokens)
        .saturating_sub(profile.profile.provider_overhead_tokens)
        .max(1);
    Ok(ModelRequestDraft {
        model: profile.profile.model.clone(),
        messages: Arc::from([]),
        tools: Arc::from([]),
        output: OutputSpec::PlainText,
        settings: ModelSettings {
            values: params.clone(),
        },
        limits: ModelRequestLimits {
            max_input_bytes: profile.profile.hard_input_bytes,
            max_input_tokens: available,
            max_output_tokens: profile.profile.reserved_output_tokens,
        },
    })
}

fn nested_request_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<TransitionEnv, RunHandleError> {
    Ok(TransitionEnv {
        now: sources.now()?,
        ids: AllocatedIds::try_new(
            vec![sources.generate::<RecordTag>()?],
            vec![sources.generate::<EventTag>()?],
            vec![sources.generate::<EffectTag>()?],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sources.generate::<AppendBatchTag>()?],
            Vec::new(),
        )
        .map_err(|_| RunHandleError::ToolSettlement {
            code: NESTED_SAMPLE_UNAVAILABLE,
        })?,
    })
}

fn nested_settlement_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
) -> Result<TransitionEnv, RunHandleError> {
    Ok(TransitionEnv {
        now: sources.now()?,
        ids: AllocatedIds::try_new(
            vec![sources.generate::<RecordTag>()?],
            vec![sources.generate::<EventTag>()?],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sources.generate::<AppendBatchTag>()?],
            Vec::new(),
        )
        .map_err(|_| RunHandleError::ToolSettlement {
            code: NESTED_SAMPLE_UNAVAILABLE,
        })?,
    })
}
