// ---- BeforeModel fold -------------------------------------------------

fn tool_id(name: &str) -> ToolId {
    ToolId::parse(format!("fixture.{name}")).expect("tool id")
}

fn tool_spec(name: &str) -> crate::model::ToolSpec {
    crate::model::ToolSpec {
        id: tool_id(name),
        model_name: Arc::from(name),
        title: Arc::from(name),
        description: Arc::from("fixture tool"),
        input_schema: RawJson::parse(br#"{"type":"object"}"#).expect("input schema"),
        output_schema: None,
        execution: finstack_ai_kernel::ToolExecutionMode::Sequential,
        side_effect: crate::model::SideEffectClass::ReadOnly,
        retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
        approval: crate::model::ApprovalMetadata {
            requirement: crate::model::ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes: 4_096,
        metadata: Metadata::empty(),
        deferral: crate::model::ToolDeferralSupport::Never,
    }
}

/// `context_window_tokens - (reserved_output_tokens + provider_overhead_tokens)`
/// = `10_000 - (1_000 + 500)`.
const FIXTURE_HARD_INPUT_TOKENS: u64 = 8_500;

fn test_profile() -> crate::ports::model::LockedModelContextProfile {
    crate::model::resolve_model_context_profile(
        crate::model::ModelContextProfile {
            provider: Arc::from("fixture-provider"),
            model: crate::model::ModelName::try_new("fixture-model").expect("model"),
            hard_input_bytes: 1_000_000,
            context_window_tokens: 10_000,
            max_output_tokens: 1_000,
            reserved_output_tokens: 1_000,
            provider_overhead_tokens: 500,
            estimator: crate::model::TokenEstimatorRef {
                id: Arc::from("fixture-estimator"),
                version: Arc::from("1"),
                source: crate::model::TokenEstimatorSource::ProjectExact,
            },
        },
        None,
        None,
        false,
    )
    .expect("locked profile")
}

fn request_draft(
    messages: Vec<Message>,
    tools: Vec<crate::model::ToolSpec>,
) -> crate::ports::model::ModelRequestDraft {
    crate::ports::model::ModelRequestDraft {
        model: crate::model::ModelName::try_new("fixture-model").expect("model"),
        messages: messages.into(),
        tools: tools.into(),
        output: finstack_ai_kernel::OutputSpec::PlainText,
        settings: crate::model::ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: crate::model::ModelRequestLimits {
            max_input_bytes: 1_000_000,
            max_input_tokens: FIXTURE_HARD_INPUT_TOKENS,
            max_output_tokens: 1_000,
        },
    }
}

fn model_output_contract() -> finstack_ai_kernel::EffectOutputContract {
    finstack_ai_kernel::EffectOutputContract {
        kind: finstack_ai_kernel::EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

fn model_request_settled(draft: &crate::ports::model::ModelRequestDraft) -> StageSettled {
    StageSettled {
        cursor: StageCursor {
            cycle: 0,
            stage: Stage::BeforeModel,
        },
        outcome: ReducerStageOutcome::ModelRequestPrepared {
            request: RawJson::parse(draft.canonical_bytes().expect("canonical bytes"))
                .expect("request"),
            component: None,
            output_contract: model_output_contract(),
            retry_safety: finstack_ai_kernel::RetrySafety::SafeToRetry,
            deadline: None,
        },
    }
}

/// Drive an accepted coordinator to the `BeforeModel` cursor.
fn drive_to_before_model(coordinator: &mut CommitCoordinator) {
    drive_to_prepare_context(coordinator);
    block_on(coordinator.submit(
        env(1_200, &[3, 4], &[], &[], &[101], &[], &[], 103),
        KernelInput::StageSettled(StageSettled {
            cursor: StageCursor {
                cycle: 0,
                stage: Stage::PrepareContext,
            },
            outcome: ReducerStageOutcome::ContextPrepared {
                messages: Arc::from([user_message(4, "hi")]),
            },
        }),
    ))
    .expect("context");
}

/// The facade's own id bag for a `ModelRequestPrepared` settlement:
/// `(records 2, events 1, effects 1, turns 0, model_requests 1, messages 0)`.
fn before_model_env() -> TransitionEnv {
    env(1_300, &[5, 6], &[2], &[103], &[], &[102], &[], 104)
}

/// The model draft the kernel actually committed for the pending effect.
fn committed_model_request(coordinator: &CommitCoordinator) -> crate::ports::model::ModelRequestDraft {
    let pending = coordinator
        .state()
        .pending_model_effect
        .as_ref()
        .expect("pending model effect");
    let finstack_ai_kernel::EffectInput::Model { request } = pending.requested.input() else {
        panic!("the pending model effect must carry a model input");
    };
    serde_json::from_slice(request.as_bytes()).expect("committed draft")
}

#[test]
fn before_model_filter_tools_narrows_the_committed_model_request() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let driver = driver_for(
        "fixture.filter",
        Stage::BeforeModel,
        StageOutcome::FilterTools(Arc::from([tool_id("keep")])),
    );
    let draft = request_draft(
        vec![user_message(4, "hi")],
        vec![tool_spec("keep"), tool_spec("drop")],
    );

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect("settles");

    assert_eq!(
        committed_model_request(&coordinator)
            .tools
            .iter()
            .map(|tool| tool.id.clone())
            .collect::<Vec<_>>(),
        vec![tool_id("keep")],
        "FilterTools did not narrow the committed model draft"
    );
}

#[test]
fn before_model_add_context_appends_to_the_committed_model_request() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let driver = driver_for(
        "fixture.context",
        Stage::BeforeModel,
        StageOutcome::AddContext(Arc::from([item("injected-before-model")])),
    );
    let draft = request_draft(vec![user_message(4, "hi")], Vec::new());

    block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        model_request_settled(&draft),
    ))
    .expect("settles");

    let committed = committed_model_request(&coordinator);
    assert_eq!(
        committed
            .messages
            .iter()
            .map(message_text)
            .collect::<Vec<_>>(),
        vec!["hi".to_owned(), "injected-before-model".to_owned()],
        "AddContext must append after the facade's own draft messages"
    );
    assert_eq!(committed.messages[1].role(), MessageRole::User);
}
// ---- BeforeModel: draft applier --------------------------------------

#[test]
fn a_before_model_replace_substitutes_the_whole_draft_not_just_its_messages() {
    let sources = test_sources();
    let base = request_draft(vec![user_message(4, "base")], vec![tool_spec("base-tool")]);
    let substitute = request_draft(
        vec![user_message(11, "replaced")],
        vec![tool_spec("substitute-tool")],
    );
    let fold = StageFold {
        replacement: Some(canonical_draft(&substitute).expect("replacement")),
        ..StageFold::default()
    };

    let applied = apply_model_draft(&fold, base, &sources).expect("applied");

    assert_eq!(
        applied, substitute,
        "at BeforeModel the stage payload is the whole request, so Replace re-bases all of it"
    );
}

#[test]
fn a_before_model_replace_still_admits_additive_and_narrowing_contributions() {
    let sources = test_sources();
    let base = request_draft(vec![user_message(4, "base")], Vec::new());
    let substitute = request_draft(
        vec![user_message(11, "replaced")],
        vec![tool_spec("keep"), tool_spec("drop")],
    );
    let fold = StageFold {
        replacement: Some(canonical_draft(&substitute).expect("replacement")),
        instructions: vec![item("system-add")],
        context: vec![item("context-add")],
        retained_tools: Some(BTreeSet::from([tool_id("keep")])),
        ..StageFold::default()
    };

    let applied = apply_model_draft(&fold, base, &sources).expect("applied");

    assert_eq!(
        applied
            .messages
            .iter()
            .map(message_text)
            .collect::<Vec<_>>(),
        vec![
            "replaced".to_owned(),
            "system-add".to_owned(),
            "context-add".to_owned()
        ],
        "Replace rebases; additive contributions still land, after it"
    );
    assert_eq!(applied.messages[1].role(), MessageRole::System);
    assert_eq!(applied.messages[2].role(), MessageRole::User);
    assert_eq!(
        applied
            .tools
            .iter()
            .map(|tool| tool.id.clone())
            .collect::<Vec<_>>(),
        vec![tool_id("keep")],
        "narrowing applies after Replace, so a Replace cannot resurrect a filtered tool"
    );
}

#[test]
fn an_oversized_before_model_fold_is_a_stable_bounds_error() {
    let sources = test_sources();
    let fold = StageFold {
        context: (0..crate::ports::model::ModelRequestDraft::MAX_MESSAGES)
            .map(|_| item("x"))
            .collect(),
        ..StageFold::default()
    };

    let error = apply_model_draft(
        &fold,
        request_draft(vec![user_message(4, "base")], Vec::new()),
        &sources,
    )
    .expect_err("base + additions exceed the model request message bound");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == MIDDLEWARE_STAGE_BOUNDS_EXCEEDED),
        "expected a stable bounds error, got {error:?}"
    );
}

/// The `Replace` payload is the one route to an oversized message array
/// that `StageFold::accumulate`'s own bounds check cannot see, because it
/// never inspects the replacement. It must still report the *bounds* code,
/// not the payload one — the condition is identical to the case above.
#[test]
fn an_oversized_before_model_replace_is_the_same_stable_bounds_error() {
    let sources = test_sources();
    let oversized = request_draft(
        (0..=crate::ports::model::ModelRequestDraft::MAX_MESSAGES)
            .map(|ordinal| user_message(u64::try_from(ordinal).expect("ordinal") + 1_000, "x"))
            .collect(),
        Vec::new(),
    );
    // Deliberately NOT `canonical_draft`: that goes through
    // `ModelRequestDraft::canonical_bytes`, which validates first and so
    // cannot express this payload at all. A component's `Replace` is raw
    // bytes that never passed through the draft's own constructor, which
    // is exactly why this check cannot be left to `validate`.
    let fold = StageFold {
        replacement: Some(
            RawJson::parse(serde_json_canonicalizer::to_vec(&oversized).expect("raw replacement"))
                .expect("replacement"),
        ),
        ..StageFold::default()
    };

    let error = apply_model_draft(
        &fold,
        request_draft(vec![user_message(4, "base")], Vec::new()),
        &sources,
    )
    .expect_err("a replacement draft can exceed the message bound on its own");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == MIDDLEWARE_STAGE_BOUNDS_EXCEEDED),
        "a Replace overflow must report the bounds code, not the payload one: {error:?}"
    );
}

#[test]
fn a_before_model_replace_that_is_not_a_model_draft_is_a_stable_payload_error() {
    let sources = test_sources();
    let fold = StageFold {
        replacement: Some(RawJson::parse(b"[]").expect("not a draft")),
        ..StageFold::default()
    };

    let error = apply_model_draft(
        &fold,
        request_draft(vec![user_message(4, "base")], Vec::new()),
        &sources,
    )
    .expect_err("a message array is not a model request draft");

    assert!(matches!(&error, RunHandleError::Middleware { code }
        if code.as_ref() == "middleware_stage_payload_invalid"));
}

#[test]
fn apply_model_draft_rejects_replacement_plus_compaction() {
    let sources = test_sources();
    let base = request_draft(vec![user_message(4, "base")], Vec::new());
    let substitute = request_draft(vec![user_message(11, "replaced")], Vec::new());
    let fold = StageFold {
        replacement: Some(canonical_draft(&substitute).expect("replacement")),
        compaction: Some(Box::new(crate::middleware::CompactionResult {
            evidence: crate::middleware::CompactionEvidence {
                strategy_id: Arc::from("fixture.strategy"),
                strategy_version: 1,
                configuration_digest: Digest::raw_json(b"{}"),
                model_context_profile_digest: Digest::raw_json(b"profile"),
                source_digest: Digest::raw_json(b"source"),
                protected_item_set_digest: Digest::raw_json(b"protected"),
                covered_entry_ids: Arc::from([]),
                retained_entry_ids: Arc::from([]),
                projection_digest: Digest::raw_json(b"projection"),
                estimated_tokens_before: 10,
                estimated_tokens_after: 5,
                summary_digest: None,
                cache_impact: crate::middleware::PromptCacheImpact::CacheInvalidated,
            },
            replacement_messages: Arc::from([user_message(4, "compacted")]),
            derived_summaries: Arc::from([]),
            checkpoint: None,
        })),
        ..StageFold::default()
    };

    let error = apply_model_draft(&fold, base, &sources)
        .expect_err("compaction must not silently overwrite a replacement");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == MIDDLEWARE_STAGE_UNLANDABLE),
        "expected middleware_stage_unlandable, got {error:?}"
    );
}

// ---- folded-allocation rejection is a run failure, not a worker fault --

/// `stage_allocation` speaks [`RunHandleError::ToolSettlement`], which
/// `result_fault_code` treats as a worker fault: intake torn down,
/// `RunStatus::Faulted`. A middleware fold the kernel will not admit must
/// fail only the run.
#[test]
fn folded_allocation_rejection_is_reclassified_as_a_middleware_failure() {
    let error = folded_allocation_error(RunHandleError::ToolSettlement {
        code: "stage_allocation_model_request_contract_mismatch",
    });
    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == "stage_allocation_model_request_contract_mismatch"),
        "the diagnosis must survive verbatim while the classification changes, got {error:?}"
    );
    // Anything that is already correctly classified passes through.
    assert!(matches!(
        folded_allocation_error(RunHandleError::InvalidConfiguration),
        RunHandleError::InvalidConfiguration
    ));
}

/// End to end: the *only* `stage_allocation` guard a fold can now trip.
/// `apply_model_draft` re-emits `ModelRequestPrepared` carrying the facade's
/// own `output_contract`, so a base outcome with a non-`ModelResponse`
/// contract makes `stage_allocation_model_request_contract_mismatch`
/// reachable for the first time. Before `BeforeModel` folded, `apply_fold`
/// could only produce `Fail`, `Retry`, and `ContextPrepared`, none of which
/// hits a guarded arm.
#[test]
fn a_fold_the_kernel_cannot_allocate_for_fails_the_run_not_the_worker() {
    let mut coordinator = accepted_coordinator(RunLimits::empty());
    drive_to_before_model(&mut coordinator);
    let sources = test_sources();
    let driver = driver_for(
        "fixture.filter",
        Stage::BeforeModel,
        StageOutcome::FilterTools(Arc::from([tool_id("keep")])),
    );
    let draft = request_draft(
        vec![user_message(4, "hi")],
        vec![tool_spec("keep"), tool_spec("drop")],
    );
    let mut settled = model_request_settled(&draft);
    let ReducerStageOutcome::ModelRequestPrepared {
        ref mut output_contract,
        ..
    } = settled.outcome
    else {
        panic!("fixture must be a prepared model request");
    };
    output_contract.kind = finstack_ai_kernel::EffectOutputKind::ToolResult;

    let error = block_on(settle_facade_stage(
        &mut coordinator,
        Some(&driver),
        &sources,
        &test_profile(),
        before_model_env(),
        settled,
    ))
    .expect_err("a non-ModelResponse contract has no stage allocation");

    assert!(
        matches!(&error, RunHandleError::Middleware { code }
            if code.as_ref() == "stage_allocation_model_request_contract_mismatch"),
        "a fold the kernel cannot allocate for must be a run failure, not a worker fault: {error:?}"
    );
}
