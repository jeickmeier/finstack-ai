#[tokio::test]
async fn stream_normalization_rejects_invalid_output_duplicates_and_all_oversized_results() {
    let spec = tool_spec("stream");
    let tools: Arc<[finstack_ai_runtime::ToolSpec]> = Arc::from([spec]);
    let toolset = Arc::new(ScriptedToolset::new(Arc::clone(&tools), Vec::new()));
    let resolved = catalog(toolset, 1)
        .by_name("stream")
        .expect("resolved")
        .clone();
    let invalid = ToolResult {
        output: RawJson::parse(br#"{"ok":"wrong","value":1}"#).expect("output"),
        is_error: false,
    };
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Ok(ToolStreamItem::Completed(invalid))]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect_err("invalid output");
    assert_eq!(error.code(), "tool_output_invalid");

    let valid = ToolResult {
        output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("output"),
        is_error: false,
    };
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![
                Ok(ToolStreamItem::Completed(valid.clone())),
                Ok(ToolStreamItem::Completed(valid)),
            ]),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect_err("duplicate completion");
    assert_eq!(error.code(), "tool_stream_invalid");

    let oversized_error = ToolResult {
        output: RawJson::parse(br#"{"error":"application"}"#).expect("output"),
        is_error: true,
    };
    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![Ok(ToolStreamItem::Completed(oversized_error))]),
            resolved.output_validator.as_deref(),
            4,
        )
        .await
        .expect_err("error result is also bounded");
    assert_eq!(error.code(), "tool_result_limit_exceeded");

    let error = ToolStreamAssembler::default()
        .assemble(
            stream(Vec::new()),
            resolved.output_validator.as_deref(),
            resolved.spec.max_result_bytes,
        )
        .await
        .expect_err("completion is required");
    assert_eq!(error.code(), "tool_stream_invalid");
}

#[tokio::test]
async fn progress_usage_and_stream_limits_are_normalized_before_settlement() {
    let first =
        Usage::try_new(Some(1), Some(0), Some(1), None, BTreeMap::new()).expect("first usage");
    let second =
        Usage::try_new(Some(1), Some(1), Some(2), None, BTreeMap::new()).expect("second usage");
    let completed = ToolResult {
        output: RawJson::parse(br#"{"ok":true,"value":1}"#).expect("output"),
        is_error: false,
    };
    let assembled = ToolStreamAssembler::default()
        .assemble(
            stream(vec![
                Ok(ToolStreamItem::Progress(
                    ToolProgress::try_new("halfway", Some(50)).expect("progress"),
                )),
                Ok(ToolStreamItem::Usage(UsageDelta {
                    usage: first.clone(),
                })),
                Ok(ToolStreamItem::Usage(UsageDelta {
                    usage: second.clone(),
                })),
                Ok(ToolStreamItem::Completed(completed.clone())),
            ]),
            None,
            4_096,
        )
        .await
        .expect("normalized stream");
    assert_eq!(assembled.progress.len(), 1);
    assert_eq!(assembled.usage, Some(second.clone()));

    let error = ToolStreamAssembler::default()
        .assemble(
            stream(vec![
                Ok(ToolStreamItem::Usage(UsageDelta { usage: second })),
                Ok(ToolStreamItem::Usage(UsageDelta { usage: first })),
                Ok(ToolStreamItem::Completed(completed.clone())),
            ]),
            None,
            4_096,
        )
        .await
        .expect_err("usage must not regress");
    assert_eq!(error.code(), "tool_stream_invalid");

    let error = ToolStreamAssembler::new(ToolStreamLimits {
        max_items: 1,
        max_stream_bytes: 1,
    })
    .assemble(
        stream(vec![
            Ok(ToolStreamItem::Progress(
                ToolProgress::try_new("too large", None).expect("progress"),
            )),
            Ok(ToolStreamItem::Completed(completed)),
        ]),
        None,
        4_096,
    )
    .await
    .expect_err("stream limits must apply");
    assert_eq!(error.code(), "tool_stream_limit_exceeded");
}
