use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, EffectId, EffectInput, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectRequested, EventTag, Id, IdTag,
    InvocationRecovery, Message, MessageRole, MessageTag, Metadata, PipelinePosition, ProviderIds,
    RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody, RecordEnvelope, RecordTag, RetrySafety,
    Sensitivity, TextBlock, Timestamp, Version,
};

use crate::context::CONTEXT_RECOVERY_UNCERTAIN;
use crate::context::{
    CommittedContextCall, ContextAuthority, ContextBudget, ContextCallContext, ContextContribution,
    ContextError, ContextItem, ContextItemKind, ContextOverflowPolicy, ContextProvenance,
    ContextProvider, ContextProviderDescriptor, ContextReconcileResult, ContextRequest,
    InvocationResumeAction, map_context_reconcile_result,
};
use crate::{
    AuthorizationContext, CancellationSignal, PendingContextEffect, PortFuture, ReconcileContext,
    RunCallContext,
};

use super::commit::chain_digest;
use super::structural_protected;

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn message(ordinal: u64, role: MessageRole, text: &str) -> Message {
    Message::try_new(
        id::<MessageTag>(ordinal),
        role,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

#[test]
fn structural_protected_covers_system_developer_and_trailing_user() {
    let system = message(1, MessageRole::System, "sys");
    let developer = message(2, MessageRole::Developer, "dev");
    let history_user = message(3, MessageRole::User, "old");
    let assistant = message(4, MessageRole::Assistant, "a");
    let current = message(5, MessageRole::User, "now");
    assert!(structural_protected(&system, false));
    assert!(structural_protected(&developer, false));
    assert!(!structural_protected(&history_user, false));
    assert!(!structural_protected(&assistant, true));
    assert!(structural_protected(&current, true));
}

#[test]
fn empty_provider_chain_digest_is_deterministic() {
    let empty: Arc<[Arc<dyn crate::ContextProvider>]> = Arc::from([]);
    assert_eq!(chain_digest(&empty), chain_digest(&empty));
    assert_ne!(chain_digest(&empty), Digest::raw_json(b""));
}

struct ReconcileFixture {
    descriptor: ContextProviderDescriptor,
    collect_calls: Arc<AtomicUsize>,
    last_effect_id: Arc<std::sync::Mutex<Option<EffectId>>>,
    result: ContextReconcileResult,
    contribution: ContextContribution,
}

impl ContextProvider for ReconcileFixture {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        _request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        self.collect_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_effect_id.lock().expect("effect id") = Some(ctx.run.effect_id);
        let contribution = self.contribution.clone();
        Box::pin(async move { Ok(contribution) })
    }

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingContextEffect,
    ) -> PortFuture<Result<ContextReconcileResult, ContextError>> {
        let result = self.result.clone();
        Box::pin(async move { Ok(result) })
    }
}

fn contribution(text: &str) -> ContextContribution {
    let item = ContextItem::try_new(
        ContextItemKind::QuotedSource,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        ContextProvenance {
            source_id: Arc::from("fixture.reconcile"),
            source_ref: None,
            external: true,
        },
        ContextAuthority::Untrusted,
        1,
        1,
        Sensitivity::Internal,
        false,
    )
    .expect("item");
    ContextContribution::try_new(vec![item], None::<&str>).expect("contribution")
}

fn reconcile_fixture(
    result: ContextReconcileResult,
) -> (
    ReconcileFixture,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Option<EffectId>>>,
) {
    let collect_calls = Arc::new(AtomicUsize::new(0));
    let last_effect_id = Arc::new(std::sync::Mutex::new(None));
    let contribution = contribution("reconciled");
    let fixture = ReconcileFixture {
        descriptor: ContextProviderDescriptor {
            invocation: ComponentInvocation {
                component: ComponentId::parse("fixture.context.reconcile").expect("component"),
                version: Version {
                    major: 0,
                    minor: 0,
                    patch: 4,
                },
                configuration_digest: Digest::raw_json(b"{}"),
                recovery: InvocationRecovery::Reconcile,
            },
            trusted_application_instructions: false,
            metadata: Metadata::empty(),
        },
        collect_calls: Arc::clone(&collect_calls),
        last_effect_id: Arc::clone(&last_effect_id),
        result,
        contribution,
    };
    (fixture, collect_calls, last_effect_id)
}

fn context_request() -> ContextRequest {
    ContextRequest {
        session_id: id(1),
        lane_id: id(2),
        run_id: id(3),
        user_input: Arc::from([]),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 8,
            max_tokens: 1_024,
            max_bytes: 4_096,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    }
}

fn call_context(effect_id: EffectId) -> ContextCallContext {
    ContextCallContext {
        run: RunCallContext {
            locator: finstack_ai_kernel::OperationLocator::try_new("tenant-a", id(1), id(2), id(3))
                .expect("locator"),
            authorization: AuthorizationContext {
                principal: finstack_ai_kernel::PrincipalRef::try_new(
                    "issuer",
                    "subject",
                    Some("tenant-a"),
                )
                .expect("principal"),
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id,
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
            relation_depth: 0,
        },
        provider_index: 0,
        chain_digest: Digest::raw_json(b"context-chain"),
    }
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

fn resume_after_crash(
    provider: &ReconcileFixture,
    effect_id: EffectId,
) -> Result<ContextContribution, ContextError> {
    let request = context_request();
    let context = call_context(effect_id);
    let envelope = request_envelope(effect_id, &provider.descriptor(), &request);
    let call = CommittedContextCall::try_new(&envelope, context, request, &provider.descriptor())?;
    block_on(call.resume(provider))
}

fn request_envelope(
    effect_id: EffectId,
    descriptor: &ContextProviderDescriptor,
    request: &ContextRequest,
) -> RecordEnvelope {
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Context,
        None,
        Some(descriptor.invocation.clone()),
        Some(
            PipelinePosition::try_new(Digest::raw_json(b"context-chain"), "prepare_context", 0)
                .expect("pipeline"),
        ),
        EffectOutputContract {
            kind: EffectOutputKind::ContextContribution,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"context-contribution-v1"),
        },
        EffectInput::Context {
            cursor: finstack_ai_kernel::StageCursor {
                cycle: 0,
                stage: finstack_ai_kernel::Stage::PrepareContext,
            },
            request: request.to_raw_json().expect("request"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("effect");
    let body = RecordBody::EffectRequested(requested);
    let events = (0..body
        .derived_event_count(RECORD_KIND_VERSION)
        .expect("events"))
        .map(|offset| id::<EventTag>(100 + u64::try_from(offset).expect("offset")))
        .collect();
    RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(10),
        id(1),
        id(2),
        Some(id(3)),
        1,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        Digest::raw_json(b"context-request"),
        None,
        Digest::raw_json(b"context-request"),
        events,
        body,
    )
    .expect("envelope")
}

#[test]
fn context_reconcile_completed_resumes_without_a_second_collect() {
    let contribution = contribution("already-done");
    let (provider, calls, last_id) =
        reconcile_fixture(ContextReconcileResult::Completed(contribution.clone()));
    let effect_id = id::<finstack_ai_kernel::EffectTag>(10);
    let resumed = resume_after_crash(&provider, effect_id).expect("completed");
    assert_eq!(resumed, contribution);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(last_id.lock().expect("id").is_none());
    assert_eq!(
        map_context_reconcile_result(&ContextReconcileResult::Completed(contribution)),
        InvocationResumeAction::UseRecorded
    );
}

#[test]
fn context_reconcile_not_started_retries_collect_with_the_same_effect_id() {
    let (provider, calls, last_id) = reconcile_fixture(ContextReconcileResult::NotStarted);
    let effect_id = id::<finstack_ai_kernel::EffectTag>(10);
    resume_after_crash(&provider, effect_id).expect("retry");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(last_id.lock().expect("id").as_ref(), Some(&effect_id));
    assert_eq!(
        map_context_reconcile_result(&ContextReconcileResult::NotStarted),
        InvocationResumeAction::Recompute
    );
}

#[test]
fn context_reconcile_retry_safe_retries_collect_with_the_same_effect_id() {
    let (provider, calls, last_id) = reconcile_fixture(ContextReconcileResult::RetrySafe);
    let effect_id = id::<finstack_ai_kernel::EffectTag>(10);
    resume_after_crash(&provider, effect_id).expect("retry");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(last_id.lock().expect("id").as_ref(), Some(&effect_id));
    assert_eq!(
        map_context_reconcile_result(&ContextReconcileResult::RetrySafe),
        InvocationResumeAction::Recompute
    );
}

#[test]
fn context_reconcile_unknown_fails_closed() {
    let (provider, calls, _) = reconcile_fixture(ContextReconcileResult::Unknown);
    let effect_id = id::<finstack_ai_kernel::EffectTag>(10);
    let error = resume_after_crash(&provider, effect_id).expect_err("unknown");
    assert_eq!(error.code(), CONTEXT_RECOVERY_UNCERTAIN);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        map_context_reconcile_result(&ContextReconcileResult::Unknown),
        InvocationResumeAction::SuspendUncertain
    );
}

#[test]
fn context_reconcile_non_repeatable_does_not_retry() {
    let (provider, calls, _) = reconcile_fixture(ContextReconcileResult::NonRepeatable);
    let effect_id = id::<finstack_ai_kernel::EffectTag>(10);
    let error = resume_after_crash(&provider, effect_id).expect_err("non-repeatable");
    assert_eq!(error.code(), CONTEXT_RECOVERY_UNCERTAIN);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        map_context_reconcile_result(&ContextReconcileResult::NonRepeatable),
        InvocationResumeAction::SuspendUncertain
    );
}

#[test]
fn crash_after_context_request_recovers_contribution_once() {
    let contribution = contribution("already-done");
    let (provider, calls, last_id) =
        reconcile_fixture(ContextReconcileResult::Completed(contribution.clone()));
    let effect_id = id::<finstack_ai_kernel::EffectTag>(10);
    let request = context_request();
    let _requested = request_envelope(effect_id, &provider.descriptor(), &request);
    let resumed = resume_after_crash(&provider, effect_id).expect("recover");
    assert_eq!(resumed, contribution);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(last_id.lock().expect("id").is_none());
    let collected = local_context_conformance(&provider).expect("conformance");
    assert_eq!(collected, provider.contribution);
}

fn local_context_conformance(
    provider: &ReconcileFixture,
) -> Result<ContextContribution, ContextError> {
    let descriptor = provider.descriptor();
    let effect_id = id::<finstack_ai_kernel::EffectTag>(10);
    let collected = block_on(provider.collect(call_context(effect_id), context_request()))?;
    if provider.descriptor() != descriptor {
        return Err(ContextError::try_new(
            CONTEXT_RECOVERY_UNCERTAIN,
            finstack_ai_kernel::ErrorCategory::Validation,
            "descriptor mutated after collection",
            Metadata::empty(),
        )
        .expect("error"));
    }
    Ok(collected)
}
