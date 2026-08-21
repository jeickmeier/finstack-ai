use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentRef, ContentBlock, EffectTag, ErrorCategory, ErrorDescriptor, Id, IdTag,
    InteractionKind, InteractionRequest, InteractionTag, LaneTag, OperationLocator, RawJson,
    RetryClassification, RetryDirective, RunTag, Sensitivity, SessionTag, Stage, StageCursor,
    TextBlock, ToolId, Version,
};

use super::{
    MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE, MiddlewareStageContext,
    StageFold, StageTerminal, derived_stage_effect_id,
};
use crate::RunCallContext;
use crate::context::{ContextAuthority, ContextItem, ContextItemKind, ContextProvenance};
use crate::middleware::{
    CompactionEvidence, CompactionModelRequest, CompactionResult, MiddlewareDescriptor,
    MiddlewareOrder, MiddlewareRegistration, MiddlewareRole, OrderTier, PromptCacheImpact,
    ResolvedMiddlewareChain, StageMask, StageOutcome,
};
use crate::model::{
    CancellationSignal, ModelName, ModelRequestDraft, ModelRequestLimits, ModelSettings,
};

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn item(text: &str) -> ContextItem {
    ContextItem::try_new(
        ContextItemKind::Instruction,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        ContextProvenance {
            source_id: Arc::from("fixture.source"),
            source_ref: None,
            external: false,
        },
        ContextAuthority::TrustedApplication,
        0,
        4,
        Sensitivity::Internal,
        false,
    )
    .expect("item")
}

fn item_text(value: &ContextItem) -> &str {
    match value.content.first() {
        Some(ContentBlock::Text(text)) => text.text(),
        other => panic!("expected a single text content block, got {other:?}"),
    }
}

fn tool_id(name: &str) -> ToolId {
    ToolId::parse(format!("fixture.{name}")).expect("tool id")
}

fn error(code: &str) -> Box<ErrorDescriptor> {
    Box::new(
        ErrorDescriptor::new(code, "fixture failure", ErrorCategory::Middleware, false)
            .expect("error descriptor"),
    )
}

fn retry_directive() -> RetryDirective {
    RetryDirective::try_new(
        RetryClassification::Framework,
        finstack_ai_kernel::Duration::from_millis(10),
        "fixture-policy-v1",
    )
    .expect("retry directive")
}

fn model_draft(messages: Arc<[finstack_ai_kernel::Message]>) -> ModelRequestDraft {
    ModelRequestDraft {
        model: ModelName::try_new("fixture-model").expect("model"),
        messages,
        tools: Arc::from([]),
        output: finstack_ai_kernel::OutputSpec::PlainText,
        settings: ModelSettings {
            values: RawJson::parse(b"{}").expect("settings"),
        },
        limits: ModelRequestLimits {
            max_input_bytes: 1_000_000,
            max_input_tokens: 10_000,
            max_output_tokens: 1_000,
        },
    }
}

/// A structurally valid `CompactionResult` whose semantic correctness
/// (protected-content preservation, tool-pair atomicity, digest
/// integrity...) is irrelevant here: the pure fold treats `CompactContext`
/// opaquely, trusting that `validate_stage_outcome` already checked it.
fn compaction_result(replacement_message_count: usize) -> CompactionResult {
    let replacement_messages: Arc<[finstack_ai_kernel::Message]> = (0..replacement_message_count)
        .map(|ordinal| {
            finstack_ai_kernel::Message::try_new(
                id(ordinal as u64),
                finstack_ai_kernel::MessageRole::User,
                vec![ContentBlock::Text(TextBlock::try_new("m").expect("text"))],
                finstack_ai_kernel::Timestamp::from_unix_ms(1).expect("timestamp"),
                None,
                finstack_ai_kernel::ProviderIds::empty(),
                finstack_ai_kernel::Metadata::empty(),
            )
            .expect("message")
        })
        .collect::<Vec<_>>()
        .into();
    CompactionResult {
        replacement_messages,
        derived_summaries: Arc::from([]),
        evidence: CompactionEvidence {
            strategy_id: Arc::from("fixture.strategy"),
            strategy_version: 1,
            configuration_digest: finstack_ai_kernel::Digest::raw_json(b"cfg"),
            model_context_profile_digest: finstack_ai_kernel::Digest::raw_json(b"profile"),
            source_digest: finstack_ai_kernel::Digest::raw_json(b"source"),
            protected_item_set_digest: finstack_ai_kernel::Digest::raw_json(b"protected"),
            covered_entry_ids: Arc::from([]),
            retained_entry_ids: Arc::from([]),
            projection_digest: finstack_ai_kernel::Digest::raw_json(b"projection"),
            estimated_tokens_before: 10,
            estimated_tokens_after: 5,
            summary_digest: None,
            cache_impact: PromptCacheImpact::CacheInvalidated,
        },
        checkpoint: None,
    }
}

fn interaction_request() -> Box<InteractionRequest> {
    let policy_component = ComponentId::parse("fixture.policy").expect("component");
    let version = Version {
        major: 1,
        minor: 0,
        patch: 0,
    };
    Box::new(
        InteractionRequest::try_new(
            1,
            id::<InteractionTag>(1),
            id::<EffectTag>(2),
            InteractionKind::Approval,
            vec![ContentBlock::Text(
                TextBlock::try_new("approve?").expect("prompt"),
            )],
            RawJson::parse(
                br#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
            )
            .expect("schema"),
            ComponentRef::new(policy_component, Some(version)),
            version,
            None,
            None,
            false,
            finstack_ai_kernel::Metadata::empty(),
        )
        .expect("interaction request"),
    )
}

fn compaction_model_request() -> Box<CompactionModelRequest> {
    Box::new(CompactionModelRequest {
        model: ComponentRef::new(
            ComponentId::parse("fixture.child-model").expect("component"),
            None,
        ),
        request: model_draft(Arc::from([])),
        budget_scope_id: id(9),
        source_sensitivity: Sensitivity::Internal,
        residency_policy_digest: finstack_ai_kernel::Digest::raw_json(b"residency"),
        resume_state: RawJson::parse(b"{}").expect("resume"),
    })
}

const ALL_STAGES: [Stage; 7] = [
    Stage::BeforeRun,
    Stage::PrepareContext,
    Stage::BeforeModel,
    Stage::AfterModel,
    Stage::BeforeToolBatch,
    Stage::AfterToolBatch,
    Stage::BeforeFinalize,
];

#[test]
fn stage_names_round_trip() {
    for stage in ALL_STAGES {
        let name = crate::middleware::stage_name(stage);
        assert_eq!(crate::middleware::parse_stage(name), Some(stage));
    }
}

#[test]
fn empty_outcomes_fold_to_identity() {
    let fold = StageFold::accumulate(Stage::PrepareContext, &[]).expect("fold");
    assert!(
        fold.is_identity(),
        "an empty chain must not perturb the base outcome"
    );
}

#[test]
fn add_instructions_and_add_context_each_accumulate_in_chain_order_and_stay_separate() {
    let fold = StageFold::accumulate(
        Stage::PrepareContext,
        &[
            StageOutcome::AddInstructions(Arc::from([item("system-first")])),
            StageOutcome::AddContext(Arc::from([item("context-first")])),
            StageOutcome::AddInstructions(Arc::from([item("system-second")])),
            StageOutcome::AddContext(Arc::from([item("context-second")])),
        ],
    )
    .expect("fold");
    assert_eq!(
        fold.instructions.iter().map(item_text).collect::<Vec<_>>(),
        vec!["system-first", "system-second"],
        "later components append after earlier, and instructions never mix into context"
    );
    assert_eq!(
        fold.context.iter().map(item_text).collect::<Vec<_>>(),
        vec!["context-first", "context-second"],
        "later components append after earlier, and context never mixes into instructions"
    );
    assert!(!fold.is_identity());
}

#[test]
fn filter_tools_intersects_so_order_cannot_matter() {
    let ab = StageFold::accumulate(
        Stage::BeforeModel,
        &[
            StageOutcome::FilterTools(Arc::from([tool_id("a"), tool_id("b")])),
            StageOutcome::FilterTools(Arc::from([tool_id("b"), tool_id("c")])),
        ],
    )
    .expect("fold");
    let ba = StageFold::accumulate(
        Stage::BeforeModel,
        &[
            StageOutcome::FilterTools(Arc::from([tool_id("b"), tool_id("c")])),
            StageOutcome::FilterTools(Arc::from([tool_id("a"), tool_id("b")])),
        ],
    )
    .expect("fold");
    assert_eq!(
        ab.retained_tools, ba.retained_tools,
        "intersection must be order-independent"
    );
    assert_eq!(
        ab.retained_tools.expect("narrowed").len(),
        1,
        "only b survives"
    );
}

#[test]
fn filter_tools_is_landable_at_before_tool_batch_too() {
    let fold = StageFold::accumulate(
        Stage::BeforeToolBatch,
        &[StageOutcome::FilterTools(Arc::from([tool_id("a")]))],
    )
    .expect("fold");
    assert_eq!(fold.retained_tools.expect("narrowed").len(), 1);
}

#[test]
fn first_terminal_short_circuits_the_rest_of_the_chain() {
    let fold = StageFold::accumulate(
        Stage::AfterModel,
        &[
            StageOutcome::Fail(error("first_failure")),
            StageOutcome::Fail(error("second_failure")),
        ],
    )
    .expect("fold");
    match fold.terminal {
        Some(StageTerminal::Fail(descriptor)) => {
            assert_eq!(descriptor.code.as_str(), "first_failure");
        }
        other => panic!("expected first Fail to win, got {other:?}"),
    }
}

#[test]
fn fail_is_landable_at_every_stage() {
    for stage in ALL_STAGES {
        let fold = StageFold::accumulate(stage, &[StageOutcome::Fail(error("boom"))])
            .unwrap_or_else(|error| panic!("Fail must land at {stage:?}: {error}"));
        assert!(matches!(fold.terminal, Some(StageTerminal::Fail(_))));
    }
}

#[test]
fn retry_lands_only_at_before_finalize() {
    let fold = StageFold::accumulate(
        Stage::BeforeFinalize,
        &[StageOutcome::Retry(retry_directive())],
    )
    .expect("Retry lands at BeforeFinalize");
    assert!(matches!(fold.terminal, Some(StageTerminal::Retry(_))));

    for stage in [Stage::AfterModel, Stage::AfterToolBatch] {
        let error = StageFold::accumulate(stage, &[StageOutcome::Retry(retry_directive())])
            .expect_err("the kernel admits Retry only at BeforeFinalize");
        assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
    }
}

#[test]
fn terminal_short_circuits_before_a_bounds_violation_is_reached() {
    let oversized: Arc<[ContextItem]> = (0..=finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
        .map(|_| item("x"))
        .collect::<Vec<_>>()
        .into();
    let fold = StageFold::accumulate(
        Stage::PrepareContext,
        &[
            StageOutcome::Fail(error("stop_here")),
            StageOutcome::AddContext(oversized),
        ],
    )
    .expect("the terminal short-circuits before the oversized AddContext is ever folded");
    assert!(matches!(fold.terminal, Some(StageTerminal::Fail(_))));
    assert!(fold.context.is_empty());
}

#[test]
fn replace_last_writer_wins() {
    let fold = StageFold::accumulate(
        Stage::PrepareContext,
        &[
            StageOutcome::Replace(RawJson::parse(b"{\"v\":1}").expect("a")),
            StageOutcome::Replace(RawJson::parse(b"{\"v\":2}").expect("b")),
        ],
    )
    .expect("fold");
    assert_eq!(
        fold.replacement,
        Some(RawJson::parse(b"{\"v\":2}").expect("b")),
        "Replace substitutes; the later component wins"
    );
}

#[test]
fn replace_lands_at_prepare_context_and_before_model_only() {
    for stage in [Stage::PrepareContext, Stage::BeforeModel] {
        StageFold::accumulate(
            stage,
            &[StageOutcome::Replace(
                RawJson::parse(b"{}").expect("replacement"),
            )],
        )
        .unwrap_or_else(|error| panic!("Replace must land at {stage:?}: {error}"));
    }
    for stage in [
        Stage::BeforeRun,
        Stage::AfterModel,
        Stage::BeforeToolBatch,
        Stage::AfterToolBatch,
    ] {
        let error = StageFold::accumulate(
            stage,
            &[StageOutcome::Replace(
                RawJson::parse(b"{}").expect("replacement"),
            )],
        )
        .expect_err("no ReducerStageOutcome at this stage can carry a replaced raw value");
        assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
    }
}

#[test]
fn compact_context_lands_only_at_before_model() {
    let fold = StageFold::accumulate(
        Stage::BeforeModel,
        &[StageOutcome::CompactContext(Box::new(compaction_result(2)))],
    )
    .expect("CompactContext lands at BeforeModel");
    assert!(fold.compaction.is_some());

    for stage in [Stage::PrepareContext, Stage::AfterModel] {
        let error = StageFold::accumulate(
            stage,
            &[StageOutcome::CompactContext(Box::new(compaction_result(0)))],
        )
        .expect_err("CompactContext has no landing shape outside BeforeModel");
        assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
    }
}

#[test]
fn before_model_replace_plus_compact_context_is_unlandable() {
    let error = StageFold::accumulate(
        Stage::BeforeModel,
        &[
            StageOutcome::Replace(RawJson::parse(b"{}").expect("replacement")),
            StageOutcome::CompactContext(Box::new(compaction_result(1))),
        ],
    )
    .expect_err("Replace and CompactContext cannot both claim the projection");
    assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);

    let error = StageFold::accumulate(
        Stage::BeforeModel,
        &[
            StageOutcome::CompactContext(Box::new(compaction_result(1))),
            StageOutcome::Replace(RawJson::parse(b"{}").expect("replacement")),
        ],
    )
    .expect_err("order must not matter for the conflict");
    assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
}

#[test]
fn before_model_replace_or_compact_context_alone_still_lands() {
    let replaced = StageFold::accumulate(
        Stage::BeforeModel,
        &[StageOutcome::Replace(
            RawJson::parse(b"{}").expect("replacement"),
        )],
    )
    .expect("Replace alone remains landable");
    assert!(replaced.replacement.is_some());
    assert!(replaced.compaction.is_none());

    let compacted = StageFold::accumulate(
        Stage::BeforeModel,
        &[StageOutcome::CompactContext(Box::new(compaction_result(1)))],
    )
    .expect("CompactContext alone remains landable");
    assert!(compacted.compaction.is_some());
    assert!(compacted.replacement.is_none());
}

#[test]
fn suspend_complete_request_interaction_and_request_compaction_model_are_always_unlandable() {
    let cases: Vec<(Stage, StageOutcome)> = vec![
        (
            Stage::AfterToolBatch,
            StageOutcome::Suspend(error("suspend_me")),
        ),
        (
            Stage::BeforeRun,
            StageOutcome::Complete(RawJson::parse(b"{}").expect("complete")),
        ),
        (
            Stage::BeforeFinalize,
            StageOutcome::Complete(RawJson::parse(b"{}").expect("complete")),
        ),
        (
            Stage::BeforeFinalize,
            StageOutcome::RequestInteraction(interaction_request()),
        ),
        (
            Stage::BeforeModel,
            StageOutcome::RequestCompactionModel(compaction_model_request()),
        ),
    ];
    for (stage, outcome) in cases {
        let error = StageFold::accumulate(stage, std::slice::from_ref(&outcome))
            .expect_err("has no kernel landing at any stage");
        assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
    }
}

#[test]
fn unlandable_outcome_is_a_stable_error_not_a_silent_drop() {
    let error = StageFold::accumulate(
        Stage::BeforeRun,
        &[StageOutcome::Complete(RawJson::parse(b"{}").unwrap())],
    )
    .expect_err("Complete has no kernel landing at BeforeRun");
    assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
}

#[test]
fn prepare_context_bounds_exceeded_is_a_stable_error() {
    let items: Arc<[ContextItem]> = (0..=finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
        .map(|_| item("x"))
        .collect::<Vec<_>>()
        .into();
    let error = StageFold::accumulate(Stage::PrepareContext, &[StageOutcome::AddContext(items)])
        .expect_err("one component alone exceeding the bound must not silently pass");
    assert_eq!(error.code(), MIDDLEWARE_STAGE_BOUNDS_EXCEEDED);
}

#[test]
fn prepare_context_at_exactly_the_bound_is_not_exceeded() {
    let items: Arc<[ContextItem]> = (0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
        .map(|_| item("x"))
        .collect::<Vec<_>>()
        .into();
    StageFold::accumulate(Stage::PrepareContext, &[StageOutcome::AddContext(items)])
        .expect("exactly the bound must still be admissible");
}

#[test]
fn before_model_bounds_account_for_compaction_replacement_messages_too() {
    let half = ModelRequestDraft::MAX_MESSAGES / 2;
    let items: Arc<[ContextItem]> = (0..=half).map(|_| item("x")).collect::<Vec<_>>().into();
    let error = StageFold::accumulate(
        Stage::BeforeModel,
        &[
            StageOutcome::CompactContext(Box::new(compaction_result(half))),
            StageOutcome::AddContext(items),
        ],
    )
    .expect_err("compaction replacement messages plus additions must both count");
    assert_eq!(error.code(), MIDDLEWARE_STAGE_BOUNDS_EXCEEDED);
}

fn standard_descriptor(component: &str, stage: Stage) -> MiddlewareDescriptor {
    MiddlewareDescriptor {
        invocation: finstack_ai_kernel::ComponentInvocation {
            component: ComponentId::parse(component).expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: finstack_ai_kernel::Digest::raw_json(b"{}"),
            recovery: finstack_ai_kernel::InvocationRecovery::RecomputeSafe,
        },
        stages: StageMask::from_stages([stage]),
        order: MiddlewareOrder {
            tier: OrderTier::Standard,
            priority: 0,
            before: Arc::from([]),
            after: Arc::from([]),
        },
        role: MiddlewareRole::Standard,
        metadata: finstack_ai_kernel::Metadata::empty(),
    }
}

struct Stub {
    descriptor: MiddlewareDescriptor,
}

impl crate::middleware::Middleware for Stub {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: crate::middleware::MiddlewareContext,
        _input: crate::middleware::StageInput,
    ) -> crate::PortFuture<Result<StageOutcome, crate::middleware::MiddlewareError>> {
        Box::pin(async { Ok(StageOutcome::Continue) })
    }
}

fn one_component_chain() -> Arc<ResolvedMiddlewareChain> {
    let stub: Arc<dyn crate::middleware::Middleware> = Arc::new(Stub {
        descriptor: standard_descriptor("fixture.only", Stage::BeforeRun),
    });
    Arc::new(
        ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware: stub }])
            .expect("chain"),
    )
}

#[test]
fn stage_driver_is_active_reflects_registered_stages() {
    let driver = super::StageDriver::new(one_component_chain(), CancellationSignal::new());
    assert!(driver.is_active(Stage::BeforeRun));
    assert!(
        !driver.is_active(Stage::PrepareContext),
        "no component is registered for PrepareContext"
    );
}

#[test]
fn stage_driver_exposes_the_locked_chain() {
    let chain = one_component_chain();
    let digest = chain.digest();
    let driver = super::StageDriver::new(Arc::clone(&chain), CancellationSignal::new());
    assert_eq!(driver.chain().digest(), digest);
}

#[test]
fn stage_driver_cancellation_shares_the_underlying_signal() {
    let signal = CancellationSignal::new();
    let driver = super::StageDriver::new(one_component_chain(), signal.clone());
    assert!(!driver.cancellation().is_cancelled());
    signal.cancel();
    assert!(
        driver.cancellation().is_cancelled(),
        "CancellationSignal is a shared handle, not a snapshot"
    );
}

fn test_locator() -> OperationLocator {
    OperationLocator::try_new(
        "tenant-a",
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
    )
    .expect("locator")
}

#[test]
fn derived_stage_effect_id_is_deterministic_across_calls() {
    let locator = test_locator();
    let a = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
    let b = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
    assert_eq!(a, b, "derived id must be stable so replay reproduces it");
}

#[test]
fn derived_stage_effect_id_separates_stage_and_cycle() {
    let locator = test_locator();
    let base = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
    assert_ne!(
        base,
        derived_stage_effect_id(&locator, 4, Stage::BeforeModel),
        "distinct cycles must not collide"
    );
    assert_ne!(
        base,
        derived_stage_effect_id(&locator, 3, Stage::AfterModel),
        "distinct stages must not collide"
    );
}

fn test_run_call_context() -> RunCallContext {
    let locator = test_locator();
    RunCallContext {
        effect_id: derived_stage_effect_id(&locator, 0, Stage::BeforeModel),
        locator,
        authorization: crate::model::AuthorizationContext {
            principal: finstack_ai_kernel::PrincipalRef::try_new(
                "issuer",
                "subject",
                Some("tenant-a"),
            )
            .expect("principal"),
            authentication_method: Arc::from("fixture"),
            assurance_level: Arc::from("high"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([Arc::from("tenant-a")]),
            safe_claims: finstack_ai_kernel::Metadata::empty(),
            policy_version: Arc::from("v1"),
            decision_id: Arc::from("decision-1"),
        },
        attempt: 1,
        deadline: None,
        budget_scope_id: None,
        cancellation: CancellationSignal::new(),
        relation_depth: 0,
    }
}

/// A component that counts its invocations, so a test can distinguish
/// "ran and returned Continue" from "never ran".
struct Counting {
    descriptor: MiddlewareDescriptor,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl crate::middleware::Middleware for Counting {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: crate::middleware::MiddlewareContext,
        _input: crate::middleware::StageInput,
    ) -> crate::PortFuture<Result<StageOutcome, crate::middleware::MiddlewareError>> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Box::pin(async { Ok(StageOutcome::Continue) })
    }
}

fn counting_chain(calls: &Arc<std::sync::atomic::AtomicUsize>) -> Arc<ResolvedMiddlewareChain> {
    let middleware: Arc<dyn crate::middleware::Middleware> = Arc::new(Counting {
        descriptor: standard_descriptor("fixture.counting", Stage::BeforeRun),
        calls: Arc::clone(calls),
    });
    Arc::new(
        ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
            .expect("chain"),
    )
}

fn before_run_input() -> crate::middleware::StageInput {
    crate::middleware::StageInput::BeforeRun {
        value: RawJson::parse(b"[]").expect("value"),
    }
}

fn before_run_context() -> MiddlewareStageContext {
    MiddlewareStageContext::new(
        test_run_call_context(),
        finstack_ai_kernel::Digest::raw_json(b"{}"),
        StageCursor {
            cycle: 0,
            stage: Stage::BeforeRun,
        },
    )
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => return value,
            std::task::Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[test]
fn run_stage_invokes_every_component_registered_for_the_stage() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let driver = super::StageDriver::new(counting_chain(&calls), CancellationSignal::new());

    let outcomes =
        block_on(driver.run_stage_masked(&before_run_context(), before_run_input(), |_| true))
            .expect("chain runs");

    assert_eq!(outcomes, vec![StageOutcome::Continue]);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the registered component must actually have been invoked"
    );
}

#[test]
fn run_stage_on_a_cancelled_run_invokes_no_component_and_folds_to_identity() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let signal = CancellationSignal::new();
    signal.cancel();
    let driver = super::StageDriver::new(counting_chain(&calls), signal);

    let outcomes =
        block_on(driver.run_stage_masked(&before_run_context(), before_run_input(), |_| true))
            .expect("cancellation is not an error");

    assert!(outcomes.is_empty());
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "a cancelled run must not invoke any component"
    );
    assert!(
        StageFold::accumulate(Stage::BeforeRun, &outcomes)
            .expect("fold")
            .is_identity(),
        "the skipped chain must leave the base outcome untouched"
    );
}

#[test]
fn middleware_stage_context_stage_reflects_its_cursor() {
    let cursor = StageCursor {
        cycle: 2,
        stage: Stage::BeforeModel,
    };
    let ctx = MiddlewareStageContext::new(
        test_run_call_context(),
        finstack_ai_kernel::Digest::raw_json(b"{}"),
        cursor,
    );
    assert_eq!(ctx.stage(), Stage::BeforeModel);
    assert_eq!(ctx.cursor, cursor);
}
