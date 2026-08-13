use super::*;

use finstack_ai_kernel::{
    ActiveCapability, CapabilitiesActivated, CapabilityActivationSource, Duration,
    FinalResultRecorded, JsonBlock, JsonSchemaDraft, OutputConfiguration, OutputEndStrategy,
    OutputSpec, OutputValidated, RetryClassification, RetryDirective, SchemaRef,
    StructuredResultSource, ToolCallBlock, ValidationIssue, ValidationOutcome,
};

const TIMER: u64 = 700;
const APPLICATION_CALL: u64 = 710;
const INTERNAL_CALL: u64 = 711;

fn assert_structured_golden(file: &str, harness: &Harness) {
    let source = match file {
        "valid--pr012-success.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/structured-output/valid--pr012-success.json"
        ),
        "valid--pr012-validation-retry.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/structured-output/valid--pr012-validation-retry.json"
        ),
        "valid--pr012-early.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/structured-output/valid--pr012-early.json"
        ),
        "valid--pr012-exhaustive.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/structured-output/valid--pr012-exhaustive.json"
        ),
        "valid--pr012-capabilities.json" => include_str!(
            "../../../../fixtures/compatibility/golden-trace/v1/structured-output/valid--pr012-capabilities.json"
        ),
        _ => panic!("unknown structured golden: {file}"),
    };
    let expected: Value = serde_json::from_str(source).expect("parse structured golden");
    let state = harness.kernel.state();
    let mut replay = Kernel::default();
    let mut transient = 0_u64;
    for batch in &harness.batches {
        transient += u64::try_from(
            replay
                .apply(batch, transient)
                .expect("structured golden replay")
                .len(),
        )
        .expect("event count");
    }
    let actual = json!({
        "format_version": 1,
        "phase": state.phase,
        "cycle": state.cycle,
        "state_version": state.state_version,
        "records": harness.batches.iter().flat_map(|batch| batch.records.iter())
            .map(|record| record.body().kind_name()).collect::<Vec<_>>(),
        "final_result_digest": state.final_result.as_ref().map(|result| result.value_digest.to_string()),
        "validation_error": state.validation_failure.as_ref().map(|failure| failure.error.code.as_str()),
        "skipped_tool_call_ids": state.final_result.as_ref().map_or_else(Vec::new, |result| {
            result.skipped_tool_call_ids.iter().map(ToString::to_string).collect::<Vec<_>>()
        }),
        "active_capabilities": state.active_capabilities.iter()
            .map(|item| item.capability_id.as_str()).collect::<Vec<_>>(),
        "active_tool_batch": state.active_tool_batch.is_some(),
        "retry_attempts": state.retry.attempts,
        "terminal": state.terminal.as_ref().map(|terminal| match terminal {
            TerminalState::Completed(_) => "completed",
            TerminalState::Failed(_) => "failed",
            TerminalState::Cancelled(_) => "cancelled",
        }),
        "replay_hash_equal": replay.state().state_hash().expect("replay hash")
            == state.state_hash().expect("live hash"),
    });
    assert_eq!(actual, expected, "structured golden {file}");
}

fn schema_ref() -> SchemaRef {
    SchemaRef {
        draft: JsonSchemaDraft::Draft202012,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"object","required":["answer"]}"#),
    }
}

fn configuration(end_strategy: OutputEndStrategy) -> OutputConfiguration {
    OutputConfiguration {
        output: OutputSpec::JsonSchema {
            schema: schema_ref(),
        },
        end_strategy,
    }
}

fn begin_structured_run(end_strategy: OutputEndStrategy, max_retries: Option<u32>) -> Harness {
    let mut limits = RunLimits::empty();
    limits.max_retries = max_retries;
    let mut harness = Harness::default();
    harness.apply_input(
        transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
        KernelInput::AcceptRun(AcceptRun {
            session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
            lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
            accepted: root_acceptance_with(limits, None),
        }),
    );
    harness.apply_input(
        transition_env(1_050, &[2], &[], &[], &[], &[], &[]),
        KernelInput::ConfigureOutput(configuration(end_strategy)),
    );
    harness.apply_input(
        transition_env(1_100, &[3], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
    );
    harness.apply_input(
        transition_env(1_200, &[4, 5], &[], &[], &[TURN_ONE], &[], &[]),
        stage_input(
            0,
            Stage::PrepareContext,
            ReducerStageOutcome::ContextPrepared {
                messages: Arc::from(context_messages()),
            },
        ),
    );
    harness.apply_input(
        transition_env(
            1_300,
            &[6, 7],
            &[2],
            &[EFFECT_ONE],
            &[],
            &[MODEL_REQUEST_ONE],
            &[],
        ),
        stage_input(
            0,
            Stage::BeforeModel,
            ReducerStageOutcome::ModelRequestPrepared {
                request: RawJson::parse(r#"{"messages":[]}"#).expect("model request"),
                component: None,
                output_contract: output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: None,
            },
        ),
    );
    harness
}

fn complete_with_content(harness: &mut Harness, content: Vec<ContentBlock>, call_ids: &[u64]) {
    let message = Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
        MessageRole::Assistant,
        content,
        timestamp(1_400),
        None,
        provider_ids(),
        Metadata::empty(),
    )
    .expect("structured assistant message");
    harness.apply_input(
        super::tool_batches::tool_env(
            1_400,
            &[8, 9],
            &[3, 4],
            &[],
            &[FINAL_MESSAGE_ONE],
            &[],
            call_ids,
        ),
        KernelInput::ModelSettled(ModelSettled {
            turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
            model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
            outcome: ModelSettlement::Completed {
                completion: completed_effect(EFFECT_ONE, "structured-completion", "structured"),
                assistant_message: message,
            },
        }),
    );
}

fn validate_output(
    harness: &mut Harness,
    candidate: RawJson,
    source: StructuredResultSource,
    outcome: ValidationOutcome,
) -> Decision {
    harness.apply_input(
        transition_env(1_450, &[10], &[], &[], &[], &[], &[]),
        KernelInput::OutputValidated(OutputValidated {
            message_id: id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
            schema: schema_ref(),
            candidate,
            source,
            outcome,
        }),
    )
}

fn settle_structured_after_model(harness: &mut Harness, record: u64) {
    harness.apply_input(
        transition_env(1_500, &[record], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::AfterModel, ReducerStageOutcome::Continue),
    );
}

#[test]
fn valid_json_result_is_durable_and_finalizes_with_candidate_digest() {
    let candidate = RawJson::parse(r#"{"answer":42}"#).expect("candidate");
    let mut harness = begin_structured_run(OutputEndStrategy::Early, None);
    complete_with_content(
        &mut harness,
        vec![ContentBlock::Json(JsonBlock::new(candidate.clone()))],
        &[],
    );

    let decision = validate_output(
        &mut harness,
        candidate.clone(),
        StructuredResultSource::JsonBlock { content_index: 0 },
        ValidationOutcome::Valid,
    );
    assert!(matches!(
        decision.records[0].body(),
        RecordBody::FinalResultRecorded(FinalResultRecorded { value, .. }) if value == &candidate
    ));
    assert_eq!(harness.kernel.state().state_version, 4);
    assert_eq!(
        harness
            .kernel
            .state()
            .final_result
            .as_ref()
            .map(|result| result.value_digest),
        Some(candidate.digest())
    );
    let encoded = serde_json::to_value(harness.kernel.state()).expect("v4 state JSON");
    let mut tampered = encoded.clone();
    tampered["final_result"]["value_digest"] =
        Value::String("0000000000000000000000000000000000000000000000000000000000000000".into());
    assert!(serde_json::from_value::<finstack_ai_kernel::KernelState>(tampered).is_err());
    let decoded: finstack_ai_kernel::KernelState =
        serde_json::from_value(encoded).expect("strict v4 state round trip");
    assert_eq!(
        decoded.state_hash().expect("decoded v4 hash"),
        harness.kernel.state().state_hash().expect("live v4 hash")
    );

    settle_structured_after_model(&mut harness, 11);
    assert_eq!(harness.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    harness.apply_input(
        transition_env(1_600, &[12, 13], &[5], &[], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    );
    assert!(matches!(
        harness.kernel.state().terminal,
        Some(TerminalState::Completed(ref completed)) if completed.result_digest == candidate.digest()
    ));
    assert_structured_golden("valid--pr012-success.json", &harness);
}

fn invalid_outcome() -> ValidationOutcome {
    ValidationOutcome::Invalid {
        issues: Arc::from([ValidationIssue {
            instance_path: Arc::from("/answer"),
            schema_path: Arc::from("/required"),
            keyword: Some(Arc::from("required")),
            message: Arc::from("answer is required"),
        }]),
        feedback: Arc::from("Return an object with an answer field."),
    }
}

#[test]
fn validation_failure_retries_once_and_exhaustion_is_nonretryable() {
    let candidate = RawJson::parse(r"{}").expect("candidate");
    let mut retrying = begin_structured_run(OutputEndStrategy::Early, Some(1));
    complete_with_content(
        &mut retrying,
        vec![ContentBlock::Json(JsonBlock::new(candidate.clone()))],
        &[],
    );
    validate_output(
        &mut retrying,
        candidate.clone(),
        StructuredResultSource::JsonBlock { content_index: 0 },
        invalid_outcome(),
    );
    assert!(
        retrying
            .kernel
            .state()
            .validation_failure
            .as_ref()
            .is_some_and(|failure| failure.error.retryable)
    );
    settle_structured_after_model(&mut retrying, 11);
    retrying.apply_input(
        transition_env(1_600, &[12, 13, 14], &[5], &[TIMER], &[], &[], &[]),
        stage_input(
            0,
            Stage::BeforeFinalize,
            ReducerStageOutcome::Retry(
                RetryDirective::try_new(
                    RetryClassification::Validation,
                    Duration::from_millis(10),
                    "validation-v1",
                )
                .expect("retry directive"),
            ),
        ),
    );
    retrying.apply_input(
        transition_env(1_610, &[15], &[], &[], &[], &[], &[]),
        KernelInput::TimerFired(finstack_ai_kernel::TimerFiredInput {
            effect_id: id::<finstack_ai_kernel::EffectTag>(TIMER),
            due_at: timestamp(1_610),
            fired_at: timestamp(1_610),
        }),
    );
    assert_eq!(
        retrying.kernel.state().phase,
        Some(RunPhase::PreparingContext)
    );
    assert!(retrying.kernel.state().validation_failure.is_none());
    assert_structured_golden("valid--pr012-validation-retry.json", &retrying);

    let mut exhausted = begin_structured_run(OutputEndStrategy::Early, Some(0));
    complete_with_content(
        &mut exhausted,
        vec![ContentBlock::Json(JsonBlock::new(candidate.clone()))],
        &[],
    );
    validate_output(
        &mut exhausted,
        candidate,
        StructuredResultSource::JsonBlock { content_index: 0 },
        invalid_outcome(),
    );
    let failure = exhausted
        .kernel
        .state()
        .validation_failure
        .as_ref()
        .expect("exhausted validation failure");
    assert!(!failure.error.retryable);
    assert_eq!(
        failure.error.code.as_str(),
        "structured_output_retries_exhausted"
    );
}

#[test]
fn early_skips_application_calls_while_exhaustive_plans_only_application_calls() {
    let candidate = RawJson::parse(r#"{"answer":7}"#).expect("candidate");
    let application = ToolCallBlock::try_new(
        id::<finstack_ai_kernel::ToolCallTag>(APPLICATION_CALL),
        "lookup",
        RawJson::parse(r#"{"key":"x"}"#).expect("application args"),
    )
    .expect("application call");
    let internal = ToolCallBlock::try_new(
        id::<finstack_ai_kernel::ToolCallTag>(INTERNAL_CALL),
        finstack_ai_kernel::SUBMIT_FINAL_OUTPUT_TOOL,
        candidate.clone(),
    )
    .expect("internal final call");

    let mut early = begin_structured_run(OutputEndStrategy::Early, None);
    complete_with_content(
        &mut early,
        vec![
            ContentBlock::ToolCall(internal.clone()),
            ContentBlock::ToolCall(application.clone()),
        ],
        &[INTERNAL_CALL, APPLICATION_CALL],
    );
    validate_output(
        &mut early,
        candidate.clone(),
        StructuredResultSource::InternalTool {
            tool_call_id: id::<finstack_ai_kernel::ToolCallTag>(INTERNAL_CALL),
        },
        ValidationOutcome::Valid,
    );
    assert_eq!(
        early
            .kernel
            .state()
            .final_result
            .as_ref()
            .expect("early result")
            .skipped_tool_call_ids
            .as_ref(),
        &[id::<finstack_ai_kernel::ToolCallTag>(APPLICATION_CALL)]
    );
    settle_structured_after_model(&mut early, 11);
    assert_eq!(early.kernel.state().phase, Some(RunPhase::BeforeFinalize));
    assert_structured_golden("valid--pr012-early.json", &early);

    let mut exhaustive = begin_structured_run(OutputEndStrategy::Exhaustive, None);
    complete_with_content(
        &mut exhaustive,
        vec![
            ContentBlock::ToolCall(internal),
            ContentBlock::ToolCall(application.clone()),
        ],
        &[INTERNAL_CALL, APPLICATION_CALL],
    );
    validate_output(
        &mut exhaustive,
        candidate,
        StructuredResultSource::InternalTool {
            tool_call_id: id::<finstack_ai_kernel::ToolCallTag>(INTERNAL_CALL),
        },
        ValidationOutcome::Valid,
    );
    settle_structured_after_model(&mut exhaustive, 11);
    assert_eq!(
        exhaustive.kernel.state().phase,
        Some(RunPhase::BeforeToolBatch)
    );
    let plan = super::tool_batches::execute(
        &application,
        finstack_ai_kernel::ToolExecutionMode::Sequential,
        finstack_ai_kernel::ToolFailurePolicy::ReturnToModel,
    );
    exhaustive.apply_input(
        super::tool_batches::tool_env(1_600, &[12, 13, 14], &[5], &[720], &[], &[721], &[]),
        stage_input(
            0,
            Stage::BeforeToolBatch,
            ReducerStageOutcome::ToolBatchPrepared {
                calls: Arc::from([plan]),
                continuation: finstack_ai_kernel::ToolBatchContinuation::Finalize,
            },
        ),
    );
    assert_eq!(
        exhaustive
            .kernel
            .state()
            .active_tool_batch
            .as_ref()
            .map(|batch| batch.calls.len()),
        Some(1)
    );
    assert_structured_golden("valid--pr012-exhaustive.json", &exhaustive);
}

#[test]
fn unowned_internal_tool_names_are_rejected_before_dispatch() {
    let harness = begin_structured_run(OutputEndStrategy::Exhaustive, None);
    for tool_name in [
        finstack_ai_kernel::LOAD_CAPABILITY_TOOL,
        "finstack.internal.unknown",
    ] {
        let call = ToolCallBlock::try_new(
            id::<finstack_ai_kernel::ToolCallTag>(INTERNAL_CALL),
            tool_name,
            RawJson::parse(r#"{"capability":"x"}"#).expect("internal args"),
        )
        .expect("reserved internal call");
        let message = Message::try_new(
            id::<finstack_ai_kernel::MessageTag>(FINAL_MESSAGE_ONE),
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(call)],
            timestamp(1_400),
            None,
            provider_ids(),
            Metadata::empty(),
        )
        .expect("assistant message");
        assert_error_code(
            harness.kernel.decide(
                &super::tool_batches::tool_env(
                    1_400,
                    &[8, 9],
                    &[3, 4],
                    &[],
                    &[FINAL_MESSAGE_ONE],
                    &[],
                    &[INTERNAL_CALL],
                ),
                KernelInput::ModelSettled(ModelSettled {
                    turn_id: id::<finstack_ai_kernel::TurnTag>(TURN_ONE),
                    model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(MODEL_REQUEST_ONE),
                    outcome: ModelSettlement::Completed {
                        completion: completed_effect(EFFECT_ONE, "reserved-internal", "structured"),
                        assistant_message: message,
                    },
                }),
            ),
            "assistant_message_mismatch",
        );
    }
}

#[test]
fn capability_activation_is_sorted_replay_complete_and_supports_all_sources() {
    let mut harness = Harness::default();
    accept(&mut harness);
    let activation = CapabilitiesActivated {
        prior_plan_digest: None,
        resolved_plan_digest: Digest::raw_json(br#"{"plan":1}"#),
        active: Arc::from([
            ActiveCapability {
                capability_id: finstack_ai_kernel::CapabilityId::parse("finstack.capability.alpha")
                    .expect("capability id"),
                source: CapabilityActivationSource::Always,
            },
            ActiveCapability {
                capability_id: finstack_ai_kernel::CapabilityId::parse("finstack.capability.beta")
                    .expect("capability id"),
                source: CapabilityActivationSource::Application,
            },
        ]),
    };
    harness.apply_input(
        transition_env(1_050, &[2], &[], &[], &[], &[], &[]),
        KernelInput::CapabilitiesActivated(activation.clone()),
    );
    assert_eq!(harness.kernel.state().state_version, 4);
    assert_eq!(
        harness.kernel.state().active_capabilities,
        activation.active
    );
    let duplicate = harness
        .kernel
        .decide(
            &empty_env(1_060),
            KernelInput::CapabilitiesActivated(activation),
        )
        .expect("equal activation duplicate");
    assert!(duplicate.records.is_empty());
    assert_structured_golden("valid--pr012-capabilities.json", &harness);

    let model_selected = CapabilitiesActivated {
        prior_plan_digest: harness.kernel.state().resolved_plan_digest,
        resolved_plan_digest: Digest::raw_json(br#"{"plan":2}"#),
        active: Arc::from([ActiveCapability {
            capability_id: finstack_ai_kernel::CapabilityId::parse("finstack.capability.model")
                .expect("capability id"),
            source: CapabilityActivationSource::Model,
        }]),
    };
    harness.apply_input(
        transition_env(1_070, &[3], &[], &[], &[], &[], &[]),
        KernelInput::CapabilitiesActivated(model_selected.clone()),
    );
    assert_eq!(
        harness.kernel.state().active_capabilities,
        model_selected.active
    );

    let duplicate_id = finstack_ai_kernel::CapabilityId::parse("finstack.capability.alpha")
        .expect("capability id");
    let duplicate = CapabilitiesActivated {
        prior_plan_digest: harness.kernel.state().resolved_plan_digest,
        resolved_plan_digest: Digest::raw_json(br#"{"plan":3}"#),
        active: Arc::from([
            ActiveCapability {
                capability_id: duplicate_id.clone(),
                source: CapabilityActivationSource::Always,
            },
            ActiveCapability {
                capability_id: duplicate_id,
                source: CapabilityActivationSource::Application,
            },
        ]),
    };
    assert_error_code(
        harness.kernel.decide(
            &transition_env(1_080, &[3], &[], &[], &[], &[], &[]),
            KernelInput::CapabilitiesActivated(duplicate),
        ),
        "invalid_input_payload",
    );
}
