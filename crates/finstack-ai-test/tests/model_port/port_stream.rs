#[tokio::test]
async fn one_ten_hundred_and_thousand_chunks_assemble_identically() {
    let text = "x".repeat(10_000);
    let mut terminals = Vec::new();
    for count in [1, 10, 100, 1_000] {
        let model = ScriptedModel::from_inputs(profile(), vec![chunks(count, &text)]);
        let stream = model
            .request(request(CancellationSignal::new()))
            .await
            .expect("stream");
        let assembled = ModelStreamAssembler::new(ModelStreamLimits::default())
            .expect("assembler")
            .assemble(stream)
            .await
            .expect("assembled");
        terminals.push(assembled.terminal);
    }
    assert!(terminals.windows(2).all(|pair| pair[0] == pair[1]));
}

#[tokio::test]
async fn one_warmed_scripted_handle_is_reused_for_multiple_requests() {
    let model =
        ScriptedModel::from_inputs(profile(), vec![chunks(1, "first"), chunks(1, "second")]);
    model
        .warmup(ModelWarmupContext {
            cancellation: CancellationSignal::new(),
            deadline: None,
            metadata: Metadata::empty(),
        })
        .await
        .expect("warmup");
    let assembler = ModelStreamAssembler::new(ModelStreamLimits::default()).expect("assembler");
    for expected in ["first", "second"] {
        let stream = model
            .request(request(CancellationSignal::new()))
            .await
            .expect("stream");
        let result = assembler.assemble(stream).await.expect("assembled");
        let ModelTerminal::Completed(response) = result.terminal else {
            panic!("expected completion");
        };
        assert_eq!(
            response.assistant_content.as_ref(),
            [ContentBlock::Text(
                TextBlock::try_new(expected).expect("text")
            )]
        );
    }
    assert_eq!(model.warmup_count(), 1);
    assert_eq!(model.request_count(), 2);
    assert_eq!(model.active_stream_count(), 0);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the legal transition proof keeps every normalized stream item and final aggregate visible"
)]
async fn legal_progress_tool_usage_completion_and_deferral_transitions_are_normalized() {
    let final_usage =
        Usage::try_new(Some(1), Some(1), Some(2), None, BTreeMap::new()).expect("final usage");
    let response = ModelResponse {
        assistant_content: Arc::from([ContentBlock::Text(TextBlock::try_new("ok").expect("text"))]),
        tool_calls: Arc::from([
            ModelToolCall {
                name: Arc::from("calc"),
                arguments: RawJson::parse(br#"{"a":1}"#).expect("arguments"),
                provider_call_id: None,
            },
            ModelToolCall {
                name: Arc::from("lookup"),
                arguments: RawJson::parse(br#"{"x":1}"#).expect("arguments"),
                provider_call_id: None,
            },
        ]),
        usage: final_usage.clone(),
        provider_ids: ProviderIds::try_new(None::<&str>, Some("response-legal"), None::<&str>)
            .expect("provider ids"),
        completion_id: Arc::from("completion-legal"),
        continuation_state: Some(RawJson::parse(br#"{"cursor":"next"}"#).expect("state")),
    };
    let plan = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from("ok"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ReasoningDelta(ReasoningDelta {
                text: Arc::from("private"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 2,
                name: Some(Arc::from("calc")),
                arguments_delta: Arc::from("{\"a\":"),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("lookup")),
                arguments_delta: Arc::from("{\"x\":"),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 2,
                name: None,
                arguments_delta: Arc::from("1}"),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: None,
                arguments_delta: Arc::from("1}"),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta {
                usage: Usage::try_new(Some(1), Some(0), Some(1), None, BTreeMap::new())
                    .expect("usage"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta {
                usage: final_usage,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Heartbeat(Metadata::empty()))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ProviderEvent(OpaqueProviderEvent {
                namespace: Arc::from("scripted.raw"),
                payload: RawJson::parse(br#"{"kind":"debug"}"#).expect("event"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(response.clone()))),
        ],
    };
    let model = ScriptedModel::from_plans(profile(), vec![plan]);
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    let assembled = ModelStreamAssembler::new(ModelStreamLimits::default())
        .expect("assembler")
        .assemble(stream)
        .await
        .expect("assembled");
    assert_eq!(
        assembled.progress.as_ref(),
        [
            ModelProgress::Text(Arc::from("ok")),
            ModelProgress::Reasoning(Arc::from("private")),
            ModelProgress::Heartbeat(Metadata::empty()),
        ]
    );
    assert_eq!(assembled.terminal, ModelTerminal::Completed(response));

    let provider = ComponentId::parse("finstack.model.scripted").expect("component");
    let deferral = ModelDeferral {
        handle: ExternalHandleRef::try_new(
            provider,
            "wait-1",
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::Poll,
        next_poll_at: Some(timestamp(2_500)),
        expires_at: Some(timestamp(5_000)),
    };
    assert_eq!(
        assemble_plan(ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Ok(ModelStreamItem::Deferred(
                deferral.clone(),
            )))],
        })
        .await
        .expect("deferred"),
        ModelTerminal::Deferred(deferral)
    );
}

#[tokio::test]
async fn malformed_terminal_transitions_have_exact_stable_codes() {
    let complete = ModelStreamItem::Completed(completed("a"));
    let cases = [
        (vec![], MODEL_STREAM_MISSING_COMPLETION),
        (
            vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(complete.clone())),
                ScriptedModelAction::Emit(Ok(complete.clone())),
            ],
            MODEL_STREAM_DUPLICATE_COMPLETION,
        ),
        (
            vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(complete.clone())),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Heartbeat(Metadata::empty()))),
            ],
            MODEL_STREAM_ITEM_AFTER_COMPLETION,
        ),
        (
            vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(complete.clone())),
                ScriptedModelAction::Emit(Err(ModelError::try_new(
                    "provider_error",
                    finstack_ai_kernel::ErrorCategory::Model,
                    false,
                    "late provider error",
                    Metadata::empty(),
                )
                .expect("error"))),
            ],
            MODEL_STREAM_ERROR_AFTER_COMPLETION,
        ),
    ];
    for (actions, code) in cases {
        let error = assemble_plan(ScriptedModelPlan { actions })
            .await
            .expect_err(code);
        assert_eq!(error.code(), code);
        assert_eq!(
            error.category(),
            finstack_ai_kernel::ErrorCategory::Validation
        );
        assert!(!error.retryable());
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "the malformed transition table keeps exact error classifications visible"
)]
async fn malformed_tool_usage_and_response_sequences_are_fail_closed() {
    let incomplete = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: None,
                arguments_delta: Arc::from("{"),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
        ],
    };
    assert_eq!(
        assemble_plan(incomplete)
            .await
            .expect_err("incomplete")
            .code(),
        MODEL_TOOL_CALL_INCOMPLETE
    );

    let invalid_arguments = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("lookup")),
                arguments_delta: Arc::from("{"),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
        ],
    };
    assert_eq!(
        assemble_plan(invalid_arguments)
            .await
            .expect_err("arguments")
            .code(),
        MODEL_TOOL_CALL_ARGUMENTS_INVALID
    );

    let mutated_name = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("first")),
                arguments_delta: Arc::from("{"),
                provider_call_id: None,
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(Arc::from("second")),
                arguments_delta: Arc::from("}"),
                provider_call_id: None,
            }))),
        ],
    };
    assert_eq!(
        assemble_plan(mutated_name)
            .await
            .expect_err("mutated tool name")
            .code(),
        MODEL_TOOL_CALL_DELTA_INVALID
    );

    let first = Usage::try_new(Some(2), Some(2), Some(4), None, BTreeMap::new()).expect("usage");
    let second = Usage::try_new(Some(1), Some(2), Some(3), None, BTreeMap::new()).expect("usage");
    let usage_regression = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta { usage: first }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Usage(UsageDelta { usage: second }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
        ],
    };
    assert_eq!(
        assemble_plan(usage_regression)
            .await
            .expect_err("usage")
            .code(),
        MODEL_USAGE_INVALID
    );

    let mismatch = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: Arc::from("stream"),
            }))),
            ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("final")))),
        ],
    };
    assert_eq!(
        assemble_plan(mismatch).await.expect_err("mismatch").code(),
        MODEL_RESPONSE_MISMATCH
    );

    let model = ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                    text: Arc::from("a"),
                }))),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("a")))),
            ],
        }],
    );
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    let error = ModelStreamAssembler::new(ModelStreamLimits {
        max_items: 1,
        max_bytes: 32,
        max_tool_calls: 1,
    })
    .expect("assembler")
    .assemble(stream)
    .await
    .expect_err("item limit");
    assert_eq!(
        error.code(),
        finstack_ai_runtime::MODEL_STREAM_LIMIT_EXCEEDED
    );
    assert_eq!(error.category(), finstack_ai_kernel::ErrorCategory::Limit);
    assert!(!error.retryable());
}

#[tokio::test]
async fn completed_and_deferred_byte_limits_match_canonical_encoded_len() {
    let response = completed("");
    let response_len = serde_json_canonicalizer::to_vec(&response)
        .expect("canonical response")
        .len();
    assert!(response_len > 1);
    assert_eq!(
        assemble_terminal_with_max_bytes(
            ModelStreamItem::Completed(response.clone()),
            response_len
        )
        .await
        .expect("exact completed limit"),
        ModelTerminal::Completed(response.clone())
    );
    assert_eq!(
        assemble_terminal_with_max_bytes(ModelStreamItem::Completed(response), response_len - 1)
            .await
            .expect_err("completed over limit")
            .code(),
        finstack_ai_runtime::MODEL_STREAM_LIMIT_EXCEEDED
    );

    let deferral = ModelDeferral {
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.model.scripted").expect("component"),
            "wait-1",
            RawJson::parse(b"{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::Poll,
        next_poll_at: None,
        expires_at: None,
    };
    let deferral_len = serde_json_canonicalizer::to_vec(&deferral)
        .expect("canonical deferral")
        .len();
    assert!(deferral_len > 1);
    assert_eq!(
        assemble_terminal_with_max_bytes(ModelStreamItem::Deferred(deferral.clone()), deferral_len)
            .await
            .expect("exact deferred limit"),
        ModelTerminal::Deferred(deferral.clone())
    );
    assert_eq!(
        assemble_terminal_with_max_bytes(ModelStreamItem::Deferred(deferral), deferral_len - 1)
            .await
            .expect_err("deferred over limit")
            .code(),
        finstack_ai_runtime::MODEL_STREAM_LIMIT_EXCEEDED
    );
}

async fn assemble_terminal_with_max_bytes(
    item: ModelStreamItem,
    max_bytes: usize,
) -> Result<ModelTerminal, ModelError> {
    let model = ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::Emit(Ok(item))],
        }],
    );
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    ModelStreamAssembler::new(ModelStreamLimits {
        max_items: 1,
        max_bytes,
        max_tool_calls: 1,
    })
    .expect("assembler")
    .assemble(stream)
    .await
    .map(|value| value.terminal)
}

#[tokio::test]
async fn named_block_acknowledges_exact_effect_cancellation_without_sleep() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![
                ScriptedModelAction::Block(Arc::from("slow")),
                ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(completed("")))),
            ],
        }],
    ));
    let control = model.control();
    let cancellation = CancellationSignal::new();
    let stream = model
        .request(request(cancellation.clone()))
        .await
        .expect("stream");
    let assembler = ModelStreamAssembler::new(ModelStreamLimits::default()).expect("assembler");
    let task = tokio::spawn(async move { assembler.assemble(stream).await });
    while control.entries("slow") == 0 {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();
    let error = task.await.expect("join").expect_err("cancelled");
    assert_eq!(error.code(), "model_cancelled");
    assert_eq!(model.cancellation_acknowledgement_count(), 1);
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
}

#[tokio::test]
async fn dropping_a_slow_stream_consumer_releases_the_stream() {
    let model = Arc::new(ScriptedModel::from_plans(
        profile(),
        vec![ScriptedModelPlan {
            actions: vec![ScriptedModelAction::BlockUninterruptibly(Arc::from(
                "consumer-drop",
            ))],
        }],
    ));
    let control = model.control();
    let stream = model
        .request(request(CancellationSignal::new()))
        .await
        .expect("stream");
    let assembler = ModelStreamAssembler::new(ModelStreamLimits::default()).expect("assembler");
    let task = tokio::spawn(async move { assembler.assemble(stream).await });
    while control.entries("consumer-drop") == 0 {
        tokio::task::yield_now().await;
    }
    task.abort();
    assert!(
        task.await
            .expect_err("consumer task aborted")
            .is_cancelled()
    );
    assert_eq!(model.active_stream_count(), 0);
    assert_eq!(model.dropped_stream_count(), 1);
}
