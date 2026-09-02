use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, Digest, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, InteractionRequest, InteractionResolutionCommand, OperationLocator,
    PrincipalRef, Timestamp,
};

use crate::audit::SecurityAuditCategory;
#[cfg(feature = "native-tokio")]
use crate::audit::SecurityAuditGate;
use crate::commit::{CommitCoordinator, CommitCoordinatorError};
use crate::interaction::validate_interaction_response;
use crate::ports::journal::JournalStore;

use super::shared::{
    IngressCore, Submission, allocate_transition_env, interaction_settled_input, known_interaction,
};
use super::types::{ExternalRouteError, ExternalRouteOutcome};

/// Direct-locator router for authenticated interaction resolutions.
pub struct InteractionRouter {
    core: IngressCore,
}

impl InteractionRouter {
    /// Construct a router over a direct store and enabled healthy audit gate.
    #[must_use]
    #[cfg(feature = "native-tokio")]
    pub fn new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>) -> Self {
        Self {
            core: IngressCore::new(store, audit),
        }
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
        })
    }

    /// Route one fully authenticated interaction resolution.
    ///
    /// A matching outstanding request is validated against
    /// [`finstack_ai_kernel::InteractionRequest::response_schema`] with
    /// [`crate::ports::tool::JsonSchemaToolValidatorCompiler`] before any
    /// [`KernelInput::InteractionSettled`](finstack_ai_kernel::KernelInput::InteractionSettled)
    /// is built. Invalid payloads return
    /// [`ExternalRouteError::InvalidNormalizedCommand`] and do not commit.
    ///
    /// # Errors
    ///
    /// Returns one non-existence-revealing rejection after required audit for locator
    /// or authorization failures, a typed command error when the response fails
    /// the parked schema, and fail-closed runtime errors otherwise.
    pub async fn route(
        &self,
        command: InteractionResolutionCommand,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let submission = Submission::new(&command.locator, &command, submitted_at)?;
        let mut coordinator = Box::pin(self.core.authorize(
            &command.locator,
            command.resolution.principal(),
            command.resolution.authorization(),
            submission,
        ))
        .await?;

        let interaction_id = command.resolution.interaction_id();
        if !known_interaction(coordinator.state(), interaction_id) {
            return Err(self
                .core
                .reject_unknown(
                    &command.locator,
                    command.resolution.principal(),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_target",
                    submission,
                )
                .await);
        }
        if let Some(pending) = coordinator.state().pending_interaction()
            && pending.request.interaction_id() == interaction_id
            && pending
                .request
                .expires_at()
                .is_none_or(|deadline| submitted_at < deadline)
        {
            validate_interaction_response(
                pending.request.response_schema(),
                command.resolution.response(),
            )
            .map_err(|_| ExternalRouteError::InvalidNormalizedCommand)?;
        }

        let accepted_digest = coordinator
            .state()
            .resolution_identities()
            .get(command.resolution.resolution_id())
            .map(|identity| identity.settlement_digest);
        let input = interaction_settled_input(coordinator.state(), &command, submitted_at);
        let env = match allocate_transition_env(&coordinator, &input, submitted_at) {
            Ok(env) => env,
            Err(ExternalRouteError::Runtime(CommitCoordinatorError::Decision {
                code:
                    "conflicting_settlement"
                    | "invalid_phase_input"
                    | "invalid_run_acceptance"
                    | "invalid_input_payload"
                    | "terminal_state_immutable",
            })) => {
                return Self::record_rejection(
                    &mut coordinator,
                    &command,
                    submission,
                    accepted_digest,
                    "conflicting_or_invalid_resolution",
                )
                .await;
            }
            Err(error) => return Err(error),
        };
        match coordinator.submit(env, input).await {
            Ok(outcome) if outcome.committed.is_none() => Ok(ExternalRouteOutcome::Idempotent {
                command_id: Arc::from(command.resolution.resolution_id()),
                submitted_digest: submission.submitted_digest,
            }),
            Ok(outcome) => Ok(ExternalRouteOutcome::Committed(outcome)),
            Err(CommitCoordinatorError::Decision {
                code:
                    "conflicting_settlement"
                    | "invalid_phase_input"
                    | "invalid_run_acceptance"
                    | "invalid_input_payload"
                    | "terminal_state_immutable",
            }) => {
                Self::record_rejection(
                    &mut coordinator,
                    &command,
                    submission,
                    accepted_digest,
                    "conflicting_or_invalid_resolution",
                )
                .await
            }
            Err(error) => Err(ExternalRouteError::Runtime(error)),
        }
    }

    /// List the outstanding interaction for one authenticated locator (0 or 1).
    ///
    /// # Errors
    ///
    /// Returns one non-existence-revealing rejection after required audit for locator
    /// or authorization failures.
    pub async fn list(
        &self,
        locator: &OperationLocator,
        principal: &PrincipalRef,
        authorization: &AuthorizationEvidence,
        submitted_at: Timestamp,
    ) -> Result<Vec<InteractionRequest>, ExternalRouteError> {
        let submission =
            Submission::new(locator, &(locator, principal, authorization), submitted_at)?;
        let coordinator = self
            .core
            .authorize(locator, principal, authorization, submission)
            .await?;
        Ok(coordinator
            .state()
            .pending_interaction()
            .map(|pending| vec![pending.request.clone()])
            .unwrap_or_default())
    }

    async fn record_rejection(
        coordinator: &mut CommitCoordinator,
        command: &InteractionResolutionCommand,
        submission: Submission,
        accepted_digest: Option<Digest>,
        reason_code: &'static str,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let rejection = ExternalCommandRejected::try_new(
            ExternalCommandKind::InteractionResolution,
            command.resolution.resolution_id(),
            ExternalCommandTarget::Interaction(command.resolution.interaction_id()),
            command.resolution.principal().clone(),
            command.resolution.authorization().clone(),
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
