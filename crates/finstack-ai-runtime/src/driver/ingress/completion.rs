use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    ExternalEffectCompletedInput, ExternalEffectCompletionCommand, ExternalEffectOutcome,
    KernelInput, Timestamp,
};

use crate::audit::SecurityAuditCategory;
#[cfg(feature = "native-tokio")]
use crate::audit::SecurityAuditGate;
use crate::commit::{CommitCoordinator, CommitCoordinatorError};
use crate::ports::journal::{IdempotencyHorizon, JournalStore};

use super::shared::{
    IngressCore, Submission, allocate_transition_env, known_effect, known_tool_effect,
};
use super::types::{ExternalRouteError, ExternalRouteOutcome};

/// Direct-locator router for authenticated deferred effect completions.
pub struct ExternalCompletionRouter {
    core: IngressCore,
    horizon: Option<IdempotencyHorizon>,
}

impl ExternalCompletionRouter {
    /// Construct a router over a direct store and already-enabled healthy audit gate.
    #[must_use]
    #[cfg(feature = "native-tokio")]
    pub fn new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>) -> Self {
        Self {
            core: IngressCore::new(store, audit),
            horizon: None,
        }
    }

    /// Bind the application-configured settlement horizon.
    #[must_use]
    pub fn with_horizon(mut self, horizon: IdempotencyHorizon) -> Self {
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
        Ok(Self {
            core: IngressCore::trusted(store).await?,
            horizon: None,
        })
    }

    /// Route one fully authenticated command without accepting raw callback tokens.
    ///
    /// # Errors
    ///
    /// Returns one non-existence-revealing rejection after required audit for locator
    /// or authorization failures, and fail-closed runtime errors otherwise.
    #[expect(
        clippy::too_many_lines,
        reason = "the security-sensitive route keeps horizon, target, classification, and durable rejection order explicit"
    )]
    pub async fn route(
        &self,
        command: ExternalEffectCompletionCommand,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let submission = Submission::new(&command.locator, &command, submitted_at)?;
        if self
            .horizon
            .is_some_and(|horizon| submitted_at >= horizon.expire_at)
        {
            return Err(self
                .core
                .reject_unknown(
                    &command.locator,
                    &command.principal,
                    SecurityAuditCategory::UnknownLocator,
                    "expired_locator",
                    submission,
                )
                .await);
        }
        let mut coordinator = Box::pin(self.core.authorize(
            &command.locator,
            &command.principal,
            &command.authorization,
            submission,
        ))
        .await?;

        let effect_id = command.completion.effect_id;
        let accepted_digest = coordinator
            .state()
            .completion_identities()
            .get(command.completion.completion_id.as_ref())
            .map(|identity| identity.settlement_digest);
        if !known_effect(coordinator.state(), effect_id) {
            return Err(self
                .core
                .reject_unknown(
                    &command.locator,
                    &command.principal,
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_target",
                    submission,
                )
                .await);
        }
        if matches!(
            command.completion.outcome,
            ExternalEffectOutcome::Completed { .. }
        ) && !known_tool_effect(coordinator.state(), effect_id)
        {
            return Self::record_rejection(
                &mut coordinator,
                &command,
                submission,
                accepted_digest,
                "model_response_decoder_unavailable",
            )
            .await;
        }

        let input = KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
            completion: command.completion.clone(),
            assistant_message: None,
        });
        let env = match allocate_transition_env(&coordinator, &input, submitted_at) {
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
                return Self::record_rejection(
                    &mut coordinator,
                    &command,
                    submission,
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
                submitted_digest: submission.submitted_digest,
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
                Self::record_rejection(
                    &mut coordinator,
                    &command,
                    submission,
                    accepted_digest,
                    "conflicting_or_invalid_completion",
                )
                .await
            }
            Err(error) => Err(ExternalRouteError::Runtime(error)),
        }
    }

    async fn record_rejection(
        coordinator: &mut CommitCoordinator,
        command: &ExternalEffectCompletionCommand,
        submission: Submission,
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
            submission.submitted_digest,
            accepted_digest,
        )
        .map_err(|_| ExternalRouteError::InvalidNormalizedCommand)?;
        Box::pin(IngressCore::commit_rejection(
            coordinator,
            &command.locator,
            rejection,
            submission.submitted_at,
            reason_code,
        ))
        .await
    }
}
