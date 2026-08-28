use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, Digest, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, InteractionRequest, InteractionResolutionCommand, KernelInput,
    OperationLocator, PrincipalRef, RecordExternalCommandRejected, Timestamp,
};

use crate::audit::SecurityAuditCategory;
#[cfg(feature = "native-tokio")]
use crate::audit::SecurityAuditGate;
use crate::commit::{CommitCoordinator, CommitCoordinatorError};
use crate::ports::journal::JournalStore;

use crate::interaction::validate_interaction_response;

use super::shared::{
    EXTERNAL_COMMAND_DIGEST_DOMAIN, OPERATION_LOCATOR_DIGEST_DOMAIN, allocate_transition_env,
    audit_event, authorization_matches, interaction_settled_input, known_interaction,
    normalized_digest,
};
use super::types::{ExternalRouteError, ExternalRouteOutcome};
use super::{IngressIds, ingress_ids};

/// Direct-locator router for authenticated interaction resolutions.
pub struct InteractionRouter {
    store: Arc<dyn JournalStore>,
    #[cfg(feature = "native-tokio")]
    audit: Arc<SecurityAuditGate>,
    ids: IngressIds,
}

impl InteractionRouter {
    /// Construct a router over a direct store and enabled healthy audit gate.
    #[must_use]
    #[cfg(feature = "native-tokio")]
    pub fn new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>) -> Self {
        Self {
            store,
            audit,
            ids: ingress_ids(),
        }
    }

    /// Construct a router over a direct store and an in-process no-op audit gate.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalRouteError::IngressRejected`] when the trusted gate
    /// cannot be enabled.
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        expect(
            clippy::unused_async,
            reason = "the target-neutral constructor enables the audit gate asynchronously on native"
        )
    )]
    pub async fn trusted(store: Arc<dyn JournalStore>) -> Result<Self, ExternalRouteError> {
        #[cfg(feature = "native-tokio")]
        {
            let audit = SecurityAuditGate::enable_noop()
                .await
                .map_err(|_| ExternalRouteError::IngressRejected)?;
            Ok(Self::new(store, audit))
        }
        #[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
        {
            Ok(Self {
                store,
                ids: ingress_ids(),
            })
        }
    }

    /// Route one fully authenticated interaction resolution.
    ///
    /// A matching outstanding request is validated against
    /// [`finstack_ai_kernel::InteractionRequest::response_schema`] with
    /// [`crate::ports::tool::JsonSchemaToolValidatorCompiler`] before any
    /// [`KernelInput::InteractionSettled`] is built. Invalid payloads return
    /// [`ExternalRouteError::InvalidNormalizedCommand`] and do not commit.
    ///
    /// # Errors
    ///
    /// Returns one non-existence-revealing rejection after required audit for locator
    /// or authorization failures, a typed command error when the response fails
    /// the parked schema, and fail-closed runtime errors otherwise.
    #[expect(
        clippy::too_many_lines,
        reason = "the security-sensitive route keeps locator, authorization, expiry, and durable rejection order explicit"
    )]
    pub async fn route(
        &self,
        command: InteractionResolutionCommand,
        submitted_at: Timestamp,
    ) -> Result<ExternalRouteOutcome, ExternalRouteError> {
        let locator_digest = normalized_digest(OPERATION_LOCATOR_DIGEST_DOMAIN, &command.locator)?;
        let submitted_digest = normalized_digest(EXTERNAL_COMMAND_DIGEST_DOMAIN, &command)?;
        let Ok(mut coordinator) =
            CommitCoordinator::recover(Arc::clone(&self.store), command.locator.session_id).await
        else {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.resolution.principal().clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_locator",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
        };

        let identity_valid = coordinator.state().session_id() == Some(command.locator.session_id)
            && coordinator.state().lane_id() == Some(command.locator.lane_id)
            && coordinator.state().accepted().is_some_and(|accepted| {
                accepted.run_id() == command.locator.run_id
                    && accepted.security().tenant_scope() == command.locator.tenant_scope.as_ref()
            });
        if !identity_valid {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.resolution.principal().clone()),
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
            command.resolution.principal(),
            command.resolution.authorization(),
        ) {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.resolution.principal().clone()),
                    SecurityAuditCategory::ScopeMismatch,
                    "scope_mismatch",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
        }

        let interaction_id = command.resolution.interaction_id();
        if !known_interaction(coordinator.state(), interaction_id) {
            return self
                .reject_unknown(
                    &command.locator,
                    Some(command.resolution.principal().clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_target",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await;
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
        let env = match allocate_transition_env(&coordinator, &input, submitted_at, self.ids) {
            Ok(env) => env,
            Err(ExternalRouteError::Runtime(CommitCoordinatorError::Decision {
                code:
                    "conflicting_settlement"
                    | "invalid_phase_input"
                    | "invalid_run_acceptance"
                    | "invalid_input_payload"
                    | "terminal_state_immutable",
            })) => {
                return self
                    .record_rejection(
                        &mut coordinator,
                        &command,
                        submitted_at,
                        submitted_digest,
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
                submitted_digest,
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
                self.record_rejection(
                    &mut coordinator,
                    &command,
                    submitted_at,
                    submitted_digest,
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
        let locator_digest = normalized_digest(OPERATION_LOCATOR_DIGEST_DOMAIN, locator)?;
        let submitted_digest = normalized_digest(
            EXTERNAL_COMMAND_DIGEST_DOMAIN,
            &(locator, principal, authorization),
        )?;
        let Ok(coordinator) =
            CommitCoordinator::recover(Arc::clone(&self.store), locator.session_id).await
        else {
            return self
                .reject_unknown(
                    locator,
                    Some(principal.clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_locator",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await
                .map(|_| Vec::new());
        };
        let identity_valid = coordinator.state().session_id() == Some(locator.session_id)
            && coordinator.state().lane_id() == Some(locator.lane_id)
            && coordinator.state().accepted().is_some_and(|accepted| {
                accepted.run_id() == locator.run_id
                    && accepted.security().tenant_scope() == locator.tenant_scope.as_ref()
            });
        if !identity_valid {
            return self
                .reject_unknown(
                    locator,
                    Some(principal.clone()),
                    SecurityAuditCategory::UnknownLocator,
                    "unknown_locator",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await
                .map(|_| Vec::new());
        }
        if !authorization_matches(coordinator.state(), principal, authorization) {
            return self
                .reject_unknown(
                    locator,
                    Some(principal.clone()),
                    SecurityAuditCategory::ScopeMismatch,
                    "scope_mismatch",
                    locator_digest,
                    submitted_digest,
                    submitted_at,
                )
                .await
                .map(|_| Vec::new());
        }
        Ok(coordinator
            .state()
            .pending_interaction()
            .map(|pending| vec![pending.request.clone()])
            .unwrap_or_default())
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(
        all(feature = "wasm-host", not(feature = "native-tokio")),
        expect(
            clippy::unused_async,
            reason = "the target-neutral rejection path records the audit event asynchronously on native"
        )
    )]
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
        #[cfg(feature = "native-tokio")]
        self.audit
            .record(event)
            .await
            .map_err(|_| ExternalRouteError::IngressRejected)?;
        #[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
        let _ = event;
        Err(ExternalRouteError::IngressRejected)
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_rejection(
        &self,
        coordinator: &mut CommitCoordinator,
        command: &InteractionResolutionCommand,
        submitted_at: Timestamp,
        submitted_digest: Digest,
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
