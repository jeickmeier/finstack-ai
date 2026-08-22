use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    ExternalEffectCompletedInput, ExternalEffectCompletionCommand, ExternalEffectOutcome,
    KernelInput, OperationLocator, PrincipalRef, RecordExternalCommandRejected, Timestamp,
};

use crate::audit::{SecurityAuditCategory, SecurityAuditGate};
use crate::commit::{CommitCoordinator, CommitCoordinatorError};
use crate::ids::{OsRandomSource, SystemClock, UuidV7Generator};
use crate::ports::journal::JournalStore;

use super::shared::{
    EXTERNAL_COMMAND_DIGEST_DOMAIN, OPERATION_LOCATOR_DIGEST_DOMAIN, allocate_transition_env,
    audit_event, authorization_matches, known_effect, known_tool_effect, normalized_digest,
};
use super::types::{ExternalRouteError, ExternalRouteOutcome};

/// Direct-locator router for authenticated deferred effect completions.
pub struct ExternalCompletionRouter {
    store: Arc<dyn JournalStore>,
    audit: Arc<SecurityAuditGate>,
    ids: UuidV7Generator<SystemClock, OsRandomSource>,
    horizon: Option<crate::ports::journal::IdempotencyHorizon>,
}

impl ExternalCompletionRouter {
    /// Construct a router over a direct store and already-enabled healthy audit gate.
    #[must_use]
    pub fn new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>) -> Self {
        Self {
            store,
            audit,
            ids: UuidV7Generator::new(SystemClock, OsRandomSource),
            horizon: None,
        }
    }

    /// Bind the application-configured settlement horizon.
    #[must_use]
    pub fn with_horizon(mut self, horizon: crate::ports::journal::IdempotencyHorizon) -> Self {
        self.horizon = Some(horizon);
        self
    }

    /// Construct a router over a direct store and an in-process no-op audit gate.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalRouteError::IngressRejected`] when the trusted gate
    /// cannot be enabled.
    pub async fn trusted(store: Arc<dyn JournalStore>) -> Result<Self, ExternalRouteError> {
        let audit = SecurityAuditGate::enable_noop()
            .await
            .map_err(|_| ExternalRouteError::IngressRejected)?;
        Ok(Self::new(store, audit))
    }

    /// Route one fully authenticated command without accepting raw callback tokens.
    ///
    /// # Errors
    ///
    /// Returns one non-existence-revealing rejection after required audit for locator
    /// or authorization failures, and fail-closed runtime errors otherwise.
    #[expect(
        clippy::too_many_lines,
        reason = "the security-sensitive route keeps locator, authorization, classification, and durable rejection order explicit"
    )]
    pub async fn route(
        &self,
        command: ExternalEffectCompletionCommand,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let locator_digest = normalized_digest(OPERATION_LOCATOR_DIGEST_DOMAIN, &command.locator)?;
        let submitted_digest = normalized_digest(EXTERNAL_COMMAND_DIGEST_DOMAIN, &command)?;
        if self
            .horizon
            .is_some_and(|horizon| submitted_at >= horizon.expire_at)
        {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.principal.clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "expired_locator",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
        }
        let Ok(mut coordinator) =
            CommitCoordinator::recover(Arc::clone(&self.store), command.locator.session_id).await
        else {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.principal.clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_locator",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
        };

        let identity_valid = coordinator.state().session_id == Some(command.locator.session_id)
            && coordinator.state().lane_id == Some(command.locator.lane_id)
            && coordinator
                .state()
                .accepted
                .as_ref()
                .is_some_and(|accepted| {
                    accepted.run_id() == command.locator.run_id
                        && accepted.security().tenant_scope()
                            == command.locator.tenant_scope.as_ref()
                });
        if !identity_valid {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.principal.clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_locator",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
        }
        if !authorization_matches(
            coordinator.state(),
            &command.principal,
            &command.authorization,
        ) {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.principal.clone()),
                    SecurityAuditCategory::ScopeMismatch,
                    "scope_mismatch",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
        }

        let effect_id = command.completion.effect_id;
        let accepted_digest = coordinator
            .state()
            .completion_identities
            .get(command.completion.completion_id.as_ref())
            .map(|identity| identity.settlement_digest);
        if !known_effect(coordinator.state(), effect_id) {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.principal.clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_target",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
        }
        if matches!(
            command.completion.outcome,
            ExternalEffectOutcome::Completed { .. }
        ) && !known_tool_effect(coordinator.state(), effect_id)
        {
            return self
                .record_rejection(
                    &mut coordinator,
                    &command,
                    submitted_at,
                    submitted_digest,
                    accepted_digest,
                    "model_response_decoder_unavailable",
                )
                .await;
        }

        let input = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
            completion: command.completion.clone(),
            assistant_message: None,
        });
        let env = match allocate_transition_env(&coordinator, &input, submitted_at, self.ids) {
            Ok(env) => env,
            Err(ExternalRouteError::Runtime(CommitCoordinatorError::Decision {
                code:
                    "conflicting_completion_id"
                    | "conflicting_settlement"
                    | "effect_not_pending"
                    | "terminal_state_immutable"
                    | "invalid_phase_input"
                    | "model_settlement_mismatch"
                    | "tool_settlement_mismatch"
                    | "tool_result_mismatch",
            })) => {
                return self
                    .record_rejection(
                        &mut coordinator,
                        &command,
                        submitted_at,
                        submitted_digest,
                        accepted_digest,
                        "conflicting_or_invalid_completion",
                    )
                    .await;
            }
            Err(error) => return Err(error),
        };
        match coordinator.submit(env, input).await {
            Ok(outcome) if outcome.committed.is_none() => Ok(ExternalRouteOutcome::Idempotent {
                command_id: Arc::clone(&command.completion.completion_id),
                submitted_digest,
            }),
            Ok(outcome) => Ok(ExternalRouteOutcome::Committed(outcome)),
            Err(CommitCoordinatorError::Decision {
                code:
                    "conflicting_completion_id"
                    | "conflicting_settlement"
                    | "effect_not_pending"
                    | "terminal_state_immutable"
                    | "model_settlement_mismatch"
                    | "tool_settlement_mismatch"
                    | "tool_result_mismatch",
            }) => {
                self.record_rejection(
                    &mut coordinator,
                    &command,
                    submitted_at,
                    submitted_digest,
                    accepted_digest,
                    "conflicting_or_invalid_completion",
                )
                .await
            }
            Err(error) => Err(ExternalRouteError::Runtime(error)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn reject_unknown(
        &self,
        locator: &OperationLocator,
        principal: Option<PrincipalRef>,
        category: SecurityAuditCategory,
        reason_code: &'static str,
        locator_digest: Digest,
        submission_digest: Digest,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let event = audit_event(
            locator,
            principal,
            category,
            reason_code,
            locator_digest,
            submission_digest,
            submitted_at,
        )?;
        self.audit
            .record(event)
            .await
            .map_err(|_| ExternalRouteError::IngressRejected)?;
        Err(ExternalRouteError::IngressRejected)
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_rejection(
        &self,
        coordinator: &mut CommitCoordinator,
        command: &ExternalEffectCompletionCommand,
        submitted_at: Timestamp,
        submitted_digest: Digest,
        accepted_digest: Option<Digest>,
        reason_code: &'static str,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let rejection = ExternalCommandRejected::try_new(
            ExternalCommandKind::EffectCompletion,
            command.completion.completion_id.as_ref(),
            ExternalCommandTarget::Effect(command.completion.effect_id),
            command.principal.clone(),
            command.authorization.clone(),
            reason_code,
            submitted_digest,
            accepted_digest,
        )
        .map_err(|_| ExternalRouteError::InvalidNormalizedCommand)?;
        let input = KernelInput::RecordExternalCommandRejected(RecordExternalCommandRejected {
            locator: command.locator.clone(),
            rejection,
        });
        let env = allocate_transition_env(coordinator, &input, submitted_at, self.ids)?;
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
