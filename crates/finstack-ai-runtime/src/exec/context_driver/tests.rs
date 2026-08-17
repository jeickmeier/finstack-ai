use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, EffectId, Id, IdTag,
    InvocationRecovery, Message, MessageRole, MessageTag, Metadata, PipelinePosition, ProviderIds,
    Sensitivity, TextBlock, Timestamp, Version,
};

use crate::context::{
    ContextAuthority, ContextBudget, ContextCallContext, ContextContribution, ContextError,
    ContextItem, ContextItemKind, ContextOverflowPolicy, ContextProvenance, ContextProvider,
    ContextProviderDescriptor, ContextReconcileResult, ContextRequest, InvocationResumeAction,
    PendingContextEffect,
};
use crate::{
    AuthorizationContext, CONTEXT_RECOVERY_UNCERTAIN, CancellationSignal, PortFuture,
    ReconcileContext, RunCallContext,
};

use super::commit::{chain_digest, derived_context_effect_id};
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
fn derived_context_effect_id_is_stable_for_the_same_cursor() {
    let locator = finstack_ai_kernel::OperationLocator::try_new("tenant-a", id(1), id(2), id(3))
        .expect("locator");
    let first = derived_context_effect_id(&locator, 0, 0);
    let second = derived_context_effect_id(&locator, 0, 0);
    let other = derived_context_effect_id(&locator, 0, 1);
    assert_eq!(first, second);
    assert_ne!(first, other);
    assert_ne!(first, EffectId::from_bytes([0; 16]));
}

/// Map one provider reconcile result onto the existing context resume taxonomy.
///
/// Mirrors `map_model_reconcile_result`: `Completed` reuses the contribution,
/// `NotStarted` / `RetrySafe` retry the same effect identity, and `Unknown` /
/// `NonRepeatable` fail closed.
fn map_context_reconcile_result(result: &ContextReconcileResult) -> InvocationResumeAction {
    match result {
        ContextReconcileResult::Completed(_) => InvocationResumeAction::UseRecorded,
        ContextReconcileResult::NotStarted | ContextReconcileResult::RetrySafe => {
            InvocationResumeAction::Recompute
        }
        ContextReconcileResult::Unknown | ContextReconcileResult::NonRepeatable => {
            InvocationResumeAction::SuspendUncertain
        }
    }
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

fn pending_effect(
    request: ContextRequest,
    descriptor: &ContextProviderDescriptor,
) -> PendingContextEffect {
    PendingContextEffect {
        request,
        invocation: descriptor.invocation.clone(),
        pipeline: PipelinePosition::try_new(
            Digest::raw_json(b"context-chain"),
            "prepare_context",
            0,
        )
        .expect("pipeline"),
    }
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
    let pending = pending_effect(request.clone(), &provider.descriptor());
    let reconcile_ctx = ReconcileContext {
        run: call_context(effect_id).run,
        original_input_digest: Digest::raw_json(b"context-request"),
    };
    let result = block_on(provider.reconcile(reconcile_ctx, pending))?;
    match map_context_reconcile_result(&result) {
        InvocationResumeAction::UseRecorded => match result {
            ContextReconcileResult::Completed(contribution) => Ok(contribution),
            _ => Err(ContextError::try_new(
                CONTEXT_RECOVERY_UNCERTAIN,
                finstack_ai_kernel::ErrorCategory::Validation,
                "completed resume missing contribution",
                Metadata::empty(),
            )
            .expect("error")),
        },
        InvocationResumeAction::Recompute => {
            block_on(provider.collect(call_context(effect_id), request))
        }
        InvocationResumeAction::Reconcile | InvocationResumeAction::SuspendUncertain => {
            Err(ContextError::try_new(
                CONTEXT_RECOVERY_UNCERTAIN,
                finstack_ai_kernel::ErrorCategory::Validation,
                "context resume is non-repeatable or unknown",
                Metadata::empty(),
            )
            .expect("error"))
        }
    }
}

#[test]
fn context_reconcile_completed_resumes_without_a_second_collect() {
    let contribution = contribution("already-done");
    let (provider, calls, last_id) =
        reconcile_fixture(ContextReconcileResult::Completed(contribution.clone()));
    let locator = finstack_ai_kernel::OperationLocator::try_new("tenant-a", id(1), id(2), id(3))
        .expect("locator");
    let effect_id = derived_context_effect_id(&locator, 0, 0);
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
    let locator = finstack_ai_kernel::OperationLocator::try_new("tenant-a", id(1), id(2), id(3))
        .expect("locator");
    let effect_id = derived_context_effect_id(&locator, 0, 0);
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
    let locator = finstack_ai_kernel::OperationLocator::try_new("tenant-a", id(1), id(2), id(3))
        .expect("locator");
    let effect_id = derived_context_effect_id(&locator, 0, 0);
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
    let locator = finstack_ai_kernel::OperationLocator::try_new("tenant-a", id(1), id(2), id(3))
        .expect("locator");
    let effect_id = derived_context_effect_id(&locator, 0, 0);
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
    let locator = finstack_ai_kernel::OperationLocator::try_new("tenant-a", id(1), id(2), id(3))
        .expect("locator");
    let effect_id = derived_context_effect_id(&locator, 0, 0);
    let error = resume_after_crash(&provider, effect_id).expect_err("non-repeatable");
    assert_eq!(error.code(), CONTEXT_RECOVERY_UNCERTAIN);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        map_context_reconcile_result(&ContextReconcileResult::NonRepeatable),
        InvocationResumeAction::SuspendUncertain
    );
}
