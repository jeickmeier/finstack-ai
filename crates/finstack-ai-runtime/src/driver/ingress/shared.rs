use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, AuthorizationEvidence, CancellationRequestId, Digest, EffectId,
    EventId, InteractionExpired, InteractionId, InteractionResolutionCommand, InteractionSettled,
    KernelError, KernelInput, MessageId, ModelRequestId, OperationLocator, PrincipalRef, RecordId,
    Timestamp, ToolBatchId, ToolCallId, TransitionEnv, TurnId,
};
use serde::Serialize;

use crate::audit::{SecurityAuditCategory, SecurityAuditEvent};
use crate::commit::{CommitCoordinator, CommitCoordinatorError};
use crate::ids::{IdGenerationError, OsRandomSource, SystemClock, UuidV7Generator};

use super::types::ExternalRouteError;

pub(super) const EXTERNAL_COMMAND_DIGEST_DOMAIN: &str = "external-command";
pub(super) const OPERATION_LOCATOR_DIGEST_DOMAIN: &str = "operation-locator";
pub(super) const SECURITY_AUDIT_EVENT_DOMAIN: &str = "security-audit-event";

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
        || state
            .tool_calls()
            .values()
            .any(|identity| identity.effect_id == Some(effect_id))
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

pub(super) fn authorization_matches(
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

pub(super) fn normalized_digest<T: Serialize>(
    domain: &'static str,
    value: &T,
) -> Result<Digest, ExternalRouteError> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| ExternalRouteError::InvalidNormalizedCommand)?;
    Digest::domain_separated(domain, 1, &bytes)
        .map_err(|_| ExternalRouteError::InvalidNormalizedCommand)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn audit_event(
    locator: &OperationLocator,
    principal: Option<PrincipalRef>,
    category: SecurityAuditCategory,
    reason_code: &'static str,
    locator_digest: Digest,
    submission_digest: Digest,
    submitted_at: Timestamp,
) -> Result<SecurityAuditEvent, ExternalRouteError> {
    let id_digest = normalized_digest(
        SECURITY_AUDIT_EVENT_DOMAIN,
        &(locator_digest, submission_digest, submitted_at, reason_code),
    )?;
    SecurityAuditEvent::try_new(
        id_digest.to_hex(),
        submitted_at,
        principal,
        Some(locator.tenant_scope.as_ref()),
        category,
        reason_code,
        Some(locator_digest),
        Some(submission_digest),
    )
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

    fn push(
        &mut self,
        kind: &'static str,
        generator: UuidV7Generator<SystemClock, OsRandomSource>,
    ) -> Result<(), ExternalRouteError> {
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
    generator: UuidV7Generator<SystemClock, OsRandomSource>,
) -> Result<TransitionEnv, ExternalRouteError> {
    let mut bags = IdBags::default();
    bags.append_batch_ids
        .push(generator.generate().map_err(id_error)?);
    loop {
        let env = TransitionEnv {
            now,
            ids: bags.allocated()?,
        };
        match coordinator.classify(&env, input.clone()) {
            Ok(_) => return Ok(env),
            Err(KernelError::AllocatedIdsExhausted { kind }) => bags.push(kind, generator)?,
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
