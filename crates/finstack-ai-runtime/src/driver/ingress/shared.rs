use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, AuthorizationEvidence, CancellationRequestId, Digest, EffectId,
    EventId, ExternalCommandRejected, InteractionExpired, InteractionId,
    InteractionResolutionCommand, InteractionSettled, KernelError, KernelInput, MessageId,
    ModelRequestId, OperationLocator, PrincipalRef, RecordExternalCommandRejected, RecordId,
    Timestamp, ToolBatchId, ToolCallId, TransitionEnv, TurnId,
};
use serde::Serialize;

#[cfg(feature = "native-tokio")]
use crate::audit::SecurityAuditGate;
use crate::audit::{SecurityAuditCategory, SecurityAuditEvent};
use crate::commit::{CommitCoordinator, CommitCoordinatorError};
use crate::ids::IdGenerationError;
use crate::ports::journal::JournalStore;

use super::ingress_ids;
use super::types::{ExternalRouteError, ExternalRouteOutcome};

const EXTERNAL_COMMAND_DIGEST_DOMAIN: &str = "external-command";
const OPERATION_LOCATOR_DIGEST_DOMAIN: &str = "operation-locator";
const SECURITY_AUDIT_EVENT_DOMAIN: &str = "security-audit-event";

/// Normalized digests and submission time for one external command.
#[derive(Clone, Copy)]
pub(super) struct Submission {
    pub(super) locator_digest: Digest,
    pub(super) submitted_digest: Digest,
    pub(super) submitted_at: Timestamp,
}

impl Submission {
    pub(super) fn new<T: Serialize>(
        locator: &OperationLocator,
        command: &T,
        submitted_at: Timestamp,
    ) -> Result<Self, ExternalRouteError> {
        Ok(Self {
            locator_digest: normalized_digest(OPERATION_LOCATOR_DIGEST_DOMAIN, locator)?,
            submitted_digest: normalized_digest(EXTERNAL_COMMAND_DIGEST_DOMAIN, command)?,
            submitted_at,
        })
    }
}

/// Store, audit gate, and locator/authorization checks shared by both routers.
pub(super) struct IngressCore {
    store: Arc<dyn JournalStore>,
    #[cfg(feature = "native-tokio")]
    audit: Arc<SecurityAuditGate>,
}

impl IngressCore {
    #[cfg(feature = "native-tokio")]
    pub(super) fn new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>) -> Self {
        Self { store, audit }
    }

    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        expect(
            clippy::unused_async,
            reason = "the target-neutral constructor enables the audit gate asynchronously on native"
        )
    )]
    pub(super) async fn trusted(store: Arc<dyn JournalStore>) -> Result<Self, ExternalRouteError> {
        #[cfg(feature = "native-tokio")]
        {
            let audit = SecurityAuditGate::enable_noop()
                .await
                .map_err(|_| ExternalRouteError::IngressRejected)?;
            Ok(Self::new(store, audit))
        }
        #[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
        {
            Ok(Self { store })
        }
    }

    /// Recover the locator's session and verify its identity and authorization,
    /// auditing and rejecting without revealing existence on any failure.
    pub(super) async fn authorize(
        &self,
        locator: &OperationLocator,
        principal: &PrincipalRef,
        authorization: &AuthorizationEvidence,
        submission: Submission,
    ) -> Result<CommitCoordinator, ExternalRouteError> {
        let coordinator = CommitCoordinator::recover(Arc::clone(&self.store), locator.session_id)
            .await
            .ok();
        let failure = coordinator.as_ref().map_or(
            Some((SecurityAuditCategory::UnknownLocator, "unknown_locator")),
            |coordinator| {
                let state = coordinator.state();
                let identity_valid = state.session_id() == Some(locator.session_id)
                    && state.lane_id() == Some(locator.lane_id)
                    && state.accepted().is_some_and(|accepted| {
                        accepted.run_id() == locator.run_id
                            && accepted.security().tenant_scope() == locator.tenant_scope.as_ref()
                    });
                if !identity_valid {
                    Some((SecurityAuditCategory::UnknownLocator, "unknown_locator"))
                } else if !authorization_matches(state, principal, authorization) {
                    Some((SecurityAuditCategory::ScopeMismatch, "scope_mismatch"))
                } else {
                    None
                }
            },
        );
        let Some((category, reason_code)) = failure else {
            return coordinator.ok_or(ExternalRouteError::IngressRejected);
        };
        // Release the recovered state before the audit write so the rejection
        // path never carries it across the await.
        drop(coordinator);
        Err(self
            .reject_unknown(locator, principal, category, reason_code, submission)
            .await)
    }

    /// Record the required audit event, then return the one non-revealing rejection.
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        expect(
            clippy::unused_async,
            reason = "the target-neutral rejection path records the audit event asynchronously on native"
        )
    )]
    pub(super) async fn reject_unknown(
        &self,
        locator: &OperationLocator,
        principal: &PrincipalRef,
        category: SecurityAuditCategory,
        reason_code: &'static str,
        submission: Submission,
    ) -> ExternalRouteError {
        let id_digest = match normalized_digest(
            SECURITY_AUDIT_EVENT_DOMAIN,
            &(
                submission.locator_digest,
                submission.submitted_digest,
                submission.submitted_at,
                reason_code,
            ),
        ) {
            Ok(digest) => digest,
            Err(error) => return error,
        };
        let Ok(event) = SecurityAuditEvent::try_new(
            id_digest.to_hex(),
            submission.submitted_at,
            Some(principal.clone()),
            Some(locator.tenant_scope.as_ref()),
            category,
            reason_code,
            Some(submission.locator_digest),
            Some(submission.submitted_digest),
        ) else {
            return ExternalRouteError::InvalidNormalizedCommand;
        };
        #[cfg(feature = "native-tokio")]
        if self.audit.record(event).await.is_err() {
            return ExternalRouteError::IngressRejected;
        }
        #[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
        let _ = event;
        ExternalRouteError::IngressRejected
    }

    /// Durably record one state-neutral rejection on a known authorized target.
    pub(super) async fn commit_rejection(
        coordinator: &mut CommitCoordinator,
        locator: &OperationLocator,
        rejection: ExternalCommandRejected,
        submitted_at: Timestamp,
        reason_code: &'static str,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let input = KernelInput::RecordExternalCommandRejected(RecordExternalCommandRejected {
            locator: locator.clone(),
            rejection,
        });
        let env = allocate_transition_env(coordinator, &input, submitted_at)?;
        let evidence = coordinator
            .submit(env, input)
            .await
            .map_err(ExternalRouteError::Runtime)?;
        Ok(ExternalRouteOutcome::Rejected {
            reason_code,
            evidence,
        })
    }
}

pub(super) fn known_interaction(
    state: &finstack_ai_kernel::KernelState,
    interaction_id: InteractionId,
) -> bool {
    state
        .pending_interaction()
        .is_some_and(|pending| pending.request.interaction_id() == interaction_id)
        || state
            .last_interaction_terminal()
            .is_some_and(|terminal| terminal.interaction_id == interaction_id)
        || state
            .resolution_identities()
            .values()
            .any(|identity| identity.interaction_id == interaction_id)
}

pub(super) fn interaction_settled_input(
    state: &finstack_ai_kernel::KernelState,
    command: &InteractionResolutionCommand,
    submitted_at: Timestamp,
) -> KernelInput {
    if let Some(pending) = &state.pending_interaction()
        && pending.request.interaction_id() == command.resolution.interaction_id()
        && pending
            .request
            .expires_at()
            .is_some_and(|deadline| submitted_at >= deadline)
    {
        return KernelInput::InteractionSettled(InteractionSettled::Expired(InteractionExpired {
            interaction_id: pending.request.interaction_id(),
            expired_at: submitted_at,
        }));
    }
    KernelInput::InteractionSettled(InteractionSettled::Resolved(command.resolution.clone()))
}

pub(super) fn known_effect(state: &finstack_ai_kernel::KernelState, effect_id: EffectId) -> bool {
    state
        .pending_model_effect()
        .is_some_and(|pending| pending.requested.effect_id() == effect_id)
        || state.model_settlements().contains_key(&effect_id)
        || state.tool_settlements().contains_key(&effect_id)
        || known_tool_effect(state, effect_id)
        || state
            .completion_identities()
            .values()
            .any(|identity| identity.effect_id == effect_id)
}

pub(super) fn known_tool_effect(
    state: &finstack_ai_kernel::KernelState,
    effect_id: EffectId,
) -> bool {
    state
        .tool_calls()
        .values()
        .any(|identity| identity.effect_id == Some(effect_id))
}

fn authorization_matches(
    state: &finstack_ai_kernel::KernelState,
    principal: &PrincipalRef,
    authorization: &AuthorizationEvidence,
) -> bool {
    state.accepted().is_some_and(|accepted| {
        let security = accepted.security();
        security.principal() == principal
            && security.authorization_policy_version() == authorization.policy_version()
            && security.authorization_decision_id() == authorization.decision_id()
    })
}

fn normalized_digest<T: Serialize>(
    domain: &'static str,
    value: &T,
) -> Result<Digest, ExternalRouteError> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| ExternalRouteError::InvalidNormalizedCommand)?;
    Digest::domain_separated(domain, 1, &bytes)
        .map_err(|_| ExternalRouteError::InvalidNormalizedCommand)
}

#[derive(Default)]
#[expect(
    clippy::struct_field_names,
    reason = "field names intentionally mirror the frozen AllocatedIds queue vocabulary"
)]
struct IdBags {
    record_ids: Vec<RecordId>,
    event_ids: Vec<EventId>,
    effect_ids: Vec<EffectId>,
    interaction_ids: Vec<InteractionId>,
    message_ids: Vec<MessageId>,
    turn_ids: Vec<TurnId>,
    model_request_ids: Vec<ModelRequestId>,
    tool_batch_ids: Vec<ToolBatchId>,
    tool_call_ids: Vec<ToolCallId>,
    append_batch_ids: Vec<AppendBatchId>,
    cancellation_request_ids: Vec<CancellationRequestId>,
}

impl IdBags {
    fn allocated(&self) -> Result<AllocatedIds, ExternalRouteError> {
        AllocatedIds::try_new(
            self.record_ids.clone(),
            self.event_ids.clone(),
            self.effect_ids.clone(),
            self.interaction_ids.clone(),
            self.message_ids.clone(),
            self.turn_ids.clone(),
            self.model_request_ids.clone(),
            self.tool_batch_ids.clone(),
            self.tool_call_ids.clone(),
            self.append_batch_ids.clone(),
            self.cancellation_request_ids.clone(),
        )
        .map_err(|_| ExternalRouteError::IdAllocation)
    }

    fn push(&mut self, kind: &'static str) -> Result<(), ExternalRouteError> {
        let generator = ingress_ids();
        match kind {
            "record_ids" => self
                .record_ids
                .push(generator.generate().map_err(id_error)?),
            "event_ids" => self.event_ids.push(generator.generate().map_err(id_error)?),
            "effect_ids" => self
                .effect_ids
                .push(generator.generate().map_err(id_error)?),
            "interaction_ids" => self
                .interaction_ids
                .push(generator.generate().map_err(id_error)?),
            "message_ids" => self
                .message_ids
                .push(generator.generate().map_err(id_error)?),
            "turn_ids" => self.turn_ids.push(generator.generate().map_err(id_error)?),
            "model_request_ids" => self
                .model_request_ids
                .push(generator.generate().map_err(id_error)?),
            "tool_batch_ids" => self
                .tool_batch_ids
                .push(generator.generate().map_err(id_error)?),
            "tool_call_ids" => self
                .tool_call_ids
                .push(generator.generate().map_err(id_error)?),
            "cancellation_request_ids" => self
                .cancellation_request_ids
                .push(generator.generate().map_err(id_error)?),
            _ => return Err(ExternalRouteError::IdAllocation),
        }
        Ok(())
    }
}

pub(super) fn allocate_transition_env(
    coordinator: &CommitCoordinator,
    input: &KernelInput,
    now: Timestamp,
) -> Result<TransitionEnv, ExternalRouteError> {
    let mut bags = IdBags::default();
    bags.append_batch_ids
        .push(ingress_ids().generate().map_err(id_error)?);
    loop {
        let env = TransitionEnv {
            now,
            ids: bags.allocated()?,
        };
        match coordinator.classify(&env, input.clone()) {
            Ok(_) => return Ok(env),
            Err(KernelError::AllocatedIdsExhausted { kind }) => bags.push(kind)?,
            Err(error) => {
                return Err(ExternalRouteError::Runtime(
                    CommitCoordinatorError::Decision { code: error.code() },
                ));
            }
        }
    }
}

fn id_error(_: IdGenerationError) -> ExternalRouteError {
    ExternalRouteError::IdAllocation
}
