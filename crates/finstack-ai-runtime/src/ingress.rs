//! Authenticated external-command routing by direct durable locator.

use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, AppendBatchId, AuthorizationEvidence, CancellationRequestId, Digest, EffectId,
    EventId, ExternalCommandKind, ExternalCommandRejected, ExternalCommandTarget,
    ExternalEffectCompletedInput, ExternalEffectCompletionCommand, ExternalEffectOutcome,
    InteractionId, InteractionResolutionCommand, KernelError, KernelInput, MessageId,
    ModelRequestId, OperationLocator, PrincipalRef, RecordExternalCommandRejected, RecordId,
    Timestamp, ToolBatchId, ToolCallId, TransitionEnv, TurnId,
};
use serde::Serialize;
use thiserror::Error;

use crate::{
    CommitCoordinator, CommitCoordinatorError, CommitOutcome, IdGenerationError, JournalStore,
    OsRandomSource, SecurityAuditCategory, SecurityAuditEvent, SecurityAuditGate, SystemClock,
    UuidV7Generator,
};

const EXTERNAL_COMMAND_DIGEST_DOMAIN: &str = "external-command";
const OPERATION_LOCATOR_DIGEST_DOMAIN: &str = "operation-locator";
const SECURITY_AUDIT_EVENT_DOMAIN: &str = "security-audit-event";

/// Successful classification of one authenticated external completion command.
#[derive(Debug, Clone)]
pub enum ExternalRouteOutcome {
    /// New completion records were committed and applied.
    Committed(CommitOutcome),
    /// Equal command identity and normalized digest were already committed.
    Idempotent {
        /// External command identity.
        command_id: Arc<str>,
        /// Normalized command digest.
        submitted_digest: Digest,
    },
    /// Known authorized command was durably rejected without semantic state change.
    Rejected {
        /// Stable rejection reason.
        reason_code: &'static str,
        /// Applied state-neutral rejection evidence.
        evidence: CommitOutcome,
    },
}

/// Non-secret external-ingress failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExternalRouteError {
    /// One non-existence-revealing response for locator/auth/audit failures.
    #[error("external command rejected")]
    IngressRejected,
    /// Runtime ID generation failed before any append.
    #[error("external command id allocation failed")]
    IdAllocation,
    /// Store/replay/commit processing failed on a known authorized target.
    #[error(transparent)]
    Runtime(CommitCoordinatorError),
    /// Router could not build a valid normalized kernel command.
    #[error("invalid normalized external command")]
    InvalidNormalizedCommand,
}

/// Direct-locator router for authenticated deferred effect completions.
pub struct ExternalCompletionRouter {
    store: Arc<dyn JournalStore>,
    audit: Arc<SecurityAuditGate>,
    ids: UuidV7Generator<SystemClock, OsRandomSource>,
}

impl ExternalCompletionRouter {
    /// Construct a router over a direct store and already-enabled healthy audit gate.
    #[must_use]
    pub fn new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>) -> Self {
        Self {
            store,
            audit,
            ids: UuidV7Generator::new(SystemClock, OsRandomSource),
        }
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

/// Locator/authentication/audit router for interaction commands deferred to PR-044.
pub struct InteractionRouter {
    store: Arc<dyn JournalStore>,
    audit: Arc<SecurityAuditGate>,
}

impl InteractionRouter {
    /// Construct a router over a direct store and enabled healthy audit gate.
    #[must_use]
    pub fn new(store: Arc<dyn JournalStore>, audit: Arc<SecurityAuditGate>) -> Self {
        Self { store, audit }
    }

    /// Validate session identity/authentication, audit, then reject unavailable resolution.
    ///
    /// # Errors
    ///
    /// Always returns the same non-existence-revealing rejection in PR-014.
    pub async fn route(
        &self,
        command: InteractionResolutionCommand,
        submitted_at: Timestamp,
    ) -> Result<(), ExternalRouteError> {
        let locator_digest = normalized_digest(OPERATION_LOCATOR_DIGEST_DOMAIN, &command.locator)?;
        let submitted_digest = normalized_digest(EXTERNAL_COMMAND_DIGEST_DOMAIN, &command)?;
        let coordinator =
            CommitCoordinator::recover(Arc::clone(&self.store), command.locator.session_id).await;
        let valid = coordinator.ok().is_some_and(|coordinator| {
            coordinator.state().session_id == Some(command.locator.session_id)
                && coordinator.state().lane_id == Some(command.locator.lane_id)
                && coordinator
                    .state()
                    .accepted
                    .as_ref()
                    .is_some_and(|accepted| {
                        accepted.run_id() == command.locator.run_id
                            && accepted.security().tenant_scope()
                                == command.locator.tenant_scope.as_ref()
                    })
                && authorization_matches(
                    coordinator.state(),
                    command.resolution.principal(),
                    command.resolution.authorization(),
                )
        });
        let (category, reason_code) = if valid {
            (
                SecurityAuditCategory::UnsupportedInteraction,
                "interaction_resolution_unavailable",
            )
        } else {
            (SecurityAuditCategory::UnknownLocator, "unknown_locator")
        };
        let event = audit_event(
            &command.locator,
            Some(command.resolution.principal().clone()),
            category,
            reason_code,
            locator_digest,
            submitted_digest,
            submitted_at,
        )?;
        self.audit
            .record(event)
            .await
            .map_err(|_| ExternalRouteError::IngressRejected)?;
        Err(ExternalRouteError::IngressRejected)
    }
}

fn known_effect(state: &finstack_ai_kernel::KernelState, effect_id: EffectId) -> bool {
    state
        .pending_model_effect
        .as_ref()
        .is_some_and(|pending| pending.requested.effect_id() == effect_id)
        || state.model_settlements.contains_key(&effect_id)
        || state.tool_settlements.contains_key(&effect_id)
        || state
            .tool_calls
            .values()
            .any(|identity| identity.effect_id == Some(effect_id))
        || state
            .completion_identities
            .values()
            .any(|identity| identity.effect_id == effect_id)
}

fn known_tool_effect(state: &finstack_ai_kernel::KernelState, effect_id: EffectId) -> bool {
    state
        .tool_calls
        .values()
        .any(|identity| identity.effect_id == Some(effect_id))
}

fn authorization_matches(
    state: &finstack_ai_kernel::KernelState,
    principal: &PrincipalRef,
    authorization: &AuthorizationEvidence,
) -> bool {
    state.accepted.as_ref().is_some_and(|accepted| {
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

#[allow(clippy::too_many_arguments)]
fn audit_event(
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

fn allocate_transition_env(
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use finstack_ai_kernel::{
        AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, ErrorCategory,
        ErrorDescriptor, Id, IdTag, Kernel, Metadata, PrincipalPropagation, RawJson,
        RecordEnvelope, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
        RunSecurityContext,
    };

    use super::*;
    use crate::{
        LoadRequest, LoadedSession, PortFuture, SecurityAuditError, SecurityAuditHealth,
        SecurityAuditReceipt, SecurityAuditSink, SnapshotReceipt, SnapshotRequest, StoreError,
        StoreHealth,
    };

    fn id<T: IdTag>(ordinal: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Id::from_bytes(bytes)
    }

    fn timestamp(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn principal(subject: &str) -> PrincipalRef {
        PrincipalRef::try_new("issuer", subject, Some("tenant-a")).expect("principal")
    }

    fn authorization() -> AuthorizationEvidence {
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization")
    }

    fn acceptance() -> RunAccepted {
        let run_id = id(3);
        RunAccepted::try_new(
            run_id,
            RunRelation::root(run_id).expect("relation"),
            RunSecurityContext::try_new(
                "tenant-a",
                principal("subject"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(b"agent"),
            None,
        )
        .expect("accepted")
    }

    fn loaded_session() -> LoadedSession {
        let env = TransitionEnv {
            now: timestamp(1_000),
            ids: AllocatedIds::try_new(
                vec![id(1)],
                vec![id(1)],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![id(1)],
                Vec::new(),
            )
            .expect("ids"),
        };
        let decision = Kernel::default()
            .decide(
                &env,
                KernelInput::AcceptRun(AcceptRun {
                    session_id: id(1),
                    lane_id: id(2),
                    accepted: acceptance(),
                }),
            )
            .expect("decision");
        let records = decision
            .records
            .iter()
            .enumerate()
            .map(|(offset, draft)| {
                let sequence = decision.expected_sequence + u64::try_from(offset).expect("offset");
                RecordEnvelope::try_new(
                    draft.format_version(),
                    draft.kind_version(),
                    draft.record_id(),
                    draft.session_id(),
                    draft.lane_id(),
                    draft.run_id(),
                    sequence,
                    draft.timestamp(),
                    None,
                    Digest::raw_json(b"payload"),
                    None,
                    Digest::raw_json(b"checksum"),
                    draft.derived_event_ids().to_vec(),
                    draft.body().clone(),
                )
                .expect("envelope")
            })
            .collect::<Vec<_>>();
        let batch = finstack_ai_kernel::CommittedBatch::try_new(
            id(1),
            1,
            u64::try_from(records.len()).expect("count"),
            records,
        )
        .expect("batch");
        LoadedSession {
            session_id: id(1),
            head_sequence: batch.last_sequence,
            head_checksum: batch
                .records
                .last()
                .map(finstack_ai_kernel::RecordEnvelope::checksum),
            metadata: Metadata::empty(),
            committed_batches: Arc::from([batch]),
            snapshot: None,
        }
    }

    struct StaticStore(LoadedSession);

    impl JournalStore for StaticStore {
        fn append(
            &self,
            _request: finstack_ai_kernel::AppendRequest,
        ) -> PortFuture<Result<finstack_ai_kernel::CommittedBatch, StoreError>> {
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "not_used",
                })
            })
        }

        fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
            let loaded = if request.session_id == self.0.session_id {
                self.0.clone()
            } else {
                LoadedSession::empty(request.session_id)
            };
            Box::pin(async move { Ok(loaded) })
        }

        fn write_snapshot(
            &self,
            _request: SnapshotRequest,
        ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
            Box::pin(async {
                Err(StoreError::Unavailable {
                    reason_code: "not_used",
                })
            })
        }

        fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
            Box::pin(async {
                Ok(StoreHealth {
                    ready: true,
                    durable: false,
                    detail: Arc::from("test"),
                })
            })
        }
    }

    struct AuditSink {
        fail: bool,
        events: Mutex<BTreeMap<Arc<str>, SecurityAuditEvent>>,
    }

    impl AuditSink {
        fn new(fail: bool) -> Self {
            Self {
                fail,
                events: Mutex::new(BTreeMap::new()),
            }
        }
    }

    impl SecurityAuditSink for AuditSink {
        fn record(
            &self,
            event: SecurityAuditEvent,
        ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
            if self.fail {
                return Box::pin(async {
                    Err(SecurityAuditError::Unavailable {
                        reason_code: "write_failed",
                    })
                });
            }
            let event_id = Arc::<str>::from(event.event_id());
            let recorded_at = event.timestamp();
            self.events
                .lock()
                .expect("lock")
                .entry(Arc::clone(&event_id))
                .or_insert(event);
            Box::pin(async move {
                Ok(SecurityAuditReceipt {
                    event_id,
                    recorded_at,
                })
            })
        }

        fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
            Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
        }
    }

    fn locator() -> OperationLocator {
        OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
    }

    fn completion(subject: &str) -> ExternalEffectCompletionCommand {
        ExternalEffectCompletionCommand::try_new(
            locator(),
            principal(subject),
            authorization(),
            finstack_ai_kernel::ExternalEffectCompletion::try_new(
                id(999),
                "completion-1",
                ExternalEffectOutcome::Failed {
                    error: ErrorDescriptor::new(
                        "provider_failed",
                        "provider failed",
                        ErrorCategory::Model,
                        false,
                    )
                    .expect("error"),
                },
            )
            .expect("completion"),
        )
        .expect("command")
    }

    fn interaction() -> InteractionResolutionCommand {
        InteractionResolutionCommand::try_new(
            locator(),
            finstack_ai_kernel::InteractionResolution::try_new(
                id(777),
                "resolution-1",
                principal("subject"),
                authorization(),
                RawJson::parse(r#"{"approved":true}"#).expect("response"),
                None::<&str>,
            )
            .expect("resolution"),
        )
        .expect("command")
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime")
    }

    #[test]
    fn fresh_router_loads_only_locator_session_and_audits_unknown_target_idempotently() {
        runtime().block_on(async {
            let store: Arc<dyn JournalStore> = Arc::new(StaticStore(loaded_session()));
            let sink = Arc::new(AuditSink::new(false));
            let gate = SecurityAuditGate::enable(Some(sink.clone()), Duration::from_millis(100))
                .await
                .expect("gate");
            let router = ExternalCompletionRouter::new(store, gate);
            let submitted_at = timestamp(2_000);
            for _ in 0..2 {
                assert_eq!(
                    router
                        .route(completion("subject"), submitted_at)
                        .await
                        .expect_err("unknown target"),
                    ExternalRouteError::IngressRejected
                );
            }
            let events = sink.events.lock().expect("lock");
            assert_eq!(events.len(), 1);
            let event = events.values().next().expect("event");
            assert_eq!(event.category(), SecurityAuditCategory::UnknownLocator);
            assert_eq!(event.reason_code(), "unknown_target");
            assert!(event.locator_digest().is_some());
            assert!(event.submission_digest().is_some());
        });
    }

    #[test]
    fn principal_mismatch_and_interaction_unavailable_are_audited_nonrevealing() {
        runtime().block_on(async {
            let store: Arc<dyn JournalStore> = Arc::new(StaticStore(loaded_session()));
            let sink = Arc::new(AuditSink::new(false));
            let gate = SecurityAuditGate::enable(Some(sink.clone()), Duration::from_millis(100))
                .await
                .expect("gate");
            let completion_router =
                ExternalCompletionRouter::new(Arc::clone(&store), Arc::clone(&gate));
            assert_eq!(
                completion_router
                    .route(completion("different-subject"), timestamp(2_000))
                    .await
                    .expect_err("scope"),
                ExternalRouteError::IngressRejected
            );
            let interaction_router = InteractionRouter::new(store, gate);
            assert_eq!(
                interaction_router
                    .route(interaction(), timestamp(2_001))
                    .await
                    .expect_err("unavailable"),
                ExternalRouteError::IngressRejected
            );
            let events = sink.events.lock().expect("lock");
            assert!(
                events
                    .values()
                    .any(|event| event.category() == SecurityAuditCategory::ScopeMismatch)
            );
            assert!(events.values().any(|event| {
                event.category() == SecurityAuditCategory::UnsupportedInteraction
            }));
        });
    }

    #[test]
    fn audit_write_failure_rejects_closed_with_same_response() {
        runtime().block_on(async {
            let store: Arc<dyn JournalStore> = Arc::new(StaticStore(loaded_session()));
            let gate = SecurityAuditGate::enable(
                Some(Arc::new(AuditSink::new(true))),
                Duration::from_millis(100),
            )
            .await
            .expect("gate");
            let router = ExternalCompletionRouter::new(store, gate);
            assert_eq!(
                router
                    .route(completion("subject"), timestamp(2_000))
                    .await
                    .expect_err("closed"),
                ExternalRouteError::IngressRejected
            );
        });
    }
}
