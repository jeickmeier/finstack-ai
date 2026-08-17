use core::future::poll_fn;
use core::task::Waker;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use finstack_ai_kernel::{
    BudgetScopeId, Digest, EffectId, KernelState, Metadata, ModelRequestId, OperationLocator,
    PrincipalRef, RawJson, ReconciliationPolicy, RetrySafety, RunPhase, Timestamp,
};

use super::profile::ModelCapabilities;
use super::request::{ModelDeferral, ModelRequestDraft, ModelResponse};

/// Full authorization projection passed to a runtime port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationContext {
    /// Authenticated durable principal projection.
    pub principal: PrincipalRef,
    /// Authentication method.
    pub authentication_method: Arc<str>,
    /// Assurance level.
    pub assurance_level: Arc<str>,
    /// Roles granted by the exact decision.
    pub roles: Arc<[Arc<str>]>,
    /// Permitted resource scopes.
    pub permitted_scopes: Arc<[Arc<str>]>,
    /// Bounded safe claims only.
    pub safe_claims: Metadata,
    /// Authorization policy version.
    pub policy_version: Arc<str>,
    /// Authorization decision identity.
    pub decision_id: Arc<str>,
}

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    waiters: Mutex<Vec<Waker>>,
    children: Mutex<Vec<Weak<CancellationState>>>,
}

/// Cloneable, target-portable, effect-local cancellation signal.
#[derive(Clone, Debug)]
pub struct CancellationSignal(Arc<CancellationState>);

impl Default for CancellationSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationSignal {
    /// Construct an active signal.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(CancellationState {
            cancelled: AtomicBool::new(false),
            waiters: Mutex::new(Vec::new()),
            children: Mutex::new(Vec::new()),
        }))
    }

    /// Construct a descendant that is cancelled when this signal is cancelled.
    ///
    /// Cancelling the child never changes its parent or siblings. Registration
    /// is race-safe with concurrent parent cancellation: the new child is
    /// either registered before propagation or observes the cancelled parent
    /// and starts cancelled.
    #[must_use]
    pub fn child(&self) -> Self {
        let child = Self::new();
        let Ok(mut children) = self.0.children.lock() else {
            // A poisoned hierarchy cannot safely promise propagation.
            child.cancel();
            return child;
        };
        if self.is_cancelled() {
            drop(children);
            child.cancel();
        } else {
            children.push(Arc::downgrade(&child.0));
        }
        child
    }

    /// Mark the signal cancelled and wake registered observers.
    pub fn cancel(&self) {
        if self.0.cancelled.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut waiters) = self.0.waiters.lock() {
            for waiter in waiters.drain(..) {
                waiter.wake();
            }
        }
        let children = self
            .0
            .children
            .lock()
            .map(|mut children| children.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        for child in children.into_iter().filter_map(|child| child.upgrade()) {
            Self(child).cancel();
        }
    }

    /// Observe cancellation without blocking.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    /// Wait until cancellation is observed.
    pub async fn cancelled(&self) {
        poll_fn(|cx| {
            if self.is_cancelled() {
                return core::task::Poll::Ready(());
            }
            if let Ok(mut waiters) = self.0.waiters.lock()
                && !waiters.iter().any(|waiter| waiter.will_wake(cx.waker()))
            {
                waiters.push(cx.waker().clone());
            }
            if self.is_cancelled() {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
        .await;
    }
}

/// Identity, security, deadline, budget, and cancellation for one port call.
#[derive(Debug, Clone)]
pub struct RunCallContext {
    /// Complete durable operation locator.
    pub locator: OperationLocator,
    /// Authorized principal projection.
    pub authorization: AuthorizationContext,
    /// Committed effect identity — **except at a middleware stage boundary**.
    ///
    /// For every port that runs under a committed effect (Model, Tool, Context,
    /// Artifact, Budget) this is that effect's journaled `EffectId` and may be
    /// used as a journal key.
    ///
    /// A middleware stage boundary has no committed effect: no `KernelInput`
    /// commits an `EffectKind::Middleware` `EffectRequested`, and stage
    /// settlement emits only `StageOutcomeRecorded`. The chain driver therefore
    /// fills this field with
    /// [`middleware_driver::derived_stage_effect_id`](crate::middleware_driver::derived_stage_effect_id),
    /// a deterministic, domain-separated **correlation id** that names nothing
    /// in the journal. The value is projected verbatim to WIT guests by
    /// `sanitize_call_context` (`plugins/finstack-ai-wit/src/mapping.rs`), so a
    /// host or plugin that looks it up as a committed effect is wrong. See the
    /// [`middleware_driver`](crate::middleware_driver) module contract.
    pub effect_id: EffectId,
    /// One-based execution attempt.
    pub attempt: u32,
    /// Semantic deadline.
    pub deadline: Option<Timestamp>,
    /// Optional shared budget scope.
    pub budget_scope_id: Option<BudgetScopeId>,
    /// Effect-local cancellation signal.
    pub cancellation: CancellationSignal,
}

/// Model-specific call context.
#[derive(Debug, Clone)]
pub struct ModelCallContext {
    /// Shared run-call context.
    pub run: RunCallContext,
    /// Committed logical model-request identity.
    pub request_id: ModelRequestId,
}

/// Post-commit model request passed to the provider port.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Committed identity and authorization context.
    pub call: ModelCallContext,
    /// Frozen committed draft.
    pub draft: ModelRequestDraft,
    /// Optional bounded opaque continuation state.
    pub continuation_state: Option<RawJson>,
}

/// Model construction warmup context.
#[derive(Debug, Clone)]
pub struct ModelWarmupContext {
    /// Construction cancellation signal.
    pub cancellation: CancellationSignal,
    /// Construction deadline.
    pub deadline: Option<Timestamp>,
    /// Bounded non-secret construction metadata.
    pub metadata: Metadata,
}

/// Reconciliation context for the original effect.
///
/// `run.effect_id` is the application-level idempotency key for tools and
/// models. Implementations should key retries and external lookups on that
/// identity rather than allocating a new one.
#[derive(Debug, Clone)]
pub struct ReconcileContext {
    /// Original run-call context.
    pub run: RunCallContext,
    /// Frozen original input digest.
    pub original_input_digest: Digest,
}

/// Journal-first recovery action for one outstanding model effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelResumeAction {
    /// No outstanding model effect remains.
    NoOutstanding,
    /// A recorded settlement already covers the effect.
    UseRecorded,
    /// Call the provider reconcile hook before dispatch.
    Reconcile,
    /// Re-dispatch the original committed request. First-pass never returns this.
    Retry,
    /// Wait for an external completion or later poll.
    WaitExternal,
    /// Do not request or fabricate a completion.
    SuspendUncertain,
}

/// Classify recovery from committed journal state only.
///
/// First-pass never returns [`ModelResumeAction::Retry`]. Unstarted, in-flight,
/// and completed-but-uncommitted journals are identical (`AwaitingModel` plus
/// pending, no settlement) and classify as [`ModelResumeAction::Reconcile`].
#[must_use]
pub fn model_resume_action(state: &KernelState) -> ModelResumeAction {
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return ModelResumeAction::NoOutstanding;
    };
    if state
        .model_settlements
        .contains_key(&pending.requested.effect_id())
    {
        return ModelResumeAction::UseRecorded;
    }
    match state.phase {
        Some(RunPhase::AwaitingExternal) => match pending
            .deferred
            .as_ref()
            .map(|deferred| deferred.reconciliation)
        {
            Some(ReconciliationPolicy::CallbackOnly | ReconciliationPolicy::ExternalWorkflow) => {
                ModelResumeAction::WaitExternal
            }
            Some(ReconciliationPolicy::Poll | ReconciliationPolicy::CallbackOrPoll) | None => {
                ModelResumeAction::Reconcile
            }
        },
        _ => ModelResumeAction::Reconcile,
    }
}

/// Whether the committed request plus provider capabilities allow a same-identity retry.
#[must_use]
pub fn model_retry_allowed(
    requested: &finstack_ai_kernel::EffectRequested,
    capabilities: &ModelCapabilities,
) -> bool {
    matches!(
        requested.retry_safety(),
        RetrySafety::SafeToRetry | RetrySafety::IdempotentWithKey
    ) && capabilities.idempotent_requests
}

/// Model reconciliation outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelReconcileResult {
    /// Provider reports completed normalized output.
    Completed(ModelResponse),
    /// Provider reports externally deferred output.
    Deferred(ModelDeferral),
    /// Provider proves the effect never started.
    NotStarted,
    /// Provider reports the effect is still running.
    StillRunning(ModelDeferral),
    /// Frozen request may be retried safely.
    RetrySafe,
    /// Provider cannot classify the effect.
    Unknown,
    /// Provider reports non-repeatable uncertainty.
    NonRepeatable,
}

/// Map one provider reconcile result onto the documented post-reconcile action.
#[must_use]
pub fn map_model_reconcile_result(
    state: &KernelState,
    result: &ModelReconcileResult,
    retry_allowed: bool,
) -> ModelResumeAction {
    let Some(pending) = state.pending_model_effect.as_ref() else {
        return ModelResumeAction::NoOutstanding;
    };
    if state
        .model_settlements
        .contains_key(&pending.requested.effect_id())
    {
        return ModelResumeAction::UseRecorded;
    }
    let awaiting_external =
        state.phase == Some(RunPhase::AwaitingExternal) || pending.deferred.is_some();
    match result {
        ModelReconcileResult::Completed(_) => ModelResumeAction::UseRecorded,
        ModelReconcileResult::Deferred(_) | ModelReconcileResult::StillRunning(_) => {
            ModelResumeAction::WaitExternal
        }
        ModelReconcileResult::NonRepeatable => ModelResumeAction::SuspendUncertain,
        ModelReconcileResult::NotStarted | ModelReconcileResult::RetrySafe => {
            if awaiting_external {
                ModelResumeAction::SuspendUncertain
            } else {
                ModelResumeAction::Retry
            }
        }
        ModelReconcileResult::Unknown => {
            if awaiting_external || !retry_allowed {
                ModelResumeAction::SuspendUncertain
            } else {
                ModelResumeAction::Retry
            }
        }
    }
}
