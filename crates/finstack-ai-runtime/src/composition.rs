//! Commit-aware child-run and shared-budget coordination.

use std::sync::Arc;

use thiserror::Error;

use crate::{
    AGENT_INVOKE_INVALID_ACCEPTANCE, AgentInvokeError, AgentInvoker, AppendBatchId,
    BudgetChargeReceipt, BudgetChargeRecorded, BudgetChargeRequest, BudgetError, BudgetLedger,
    BudgetReleaseReceipt, BudgetReleaseRequest, BudgetRequest, BudgetReservationReleased,
    BudgetReservationRequested, BudgetReservationSettled, BudgetReservationState,
    BudgetReserveRequest, ChildRunContext, ChildRunHandle, ChildRunPrepared, ChildRunRequest,
    CommitCoordinator, CommitCoordinatorError, Digest, OperationLocator, RecordBody, RecordDraft,
    RecordId, Timestamp,
};

/// Stable identities supplied for one prepared child and optional reservation settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildCoordinationIds {
    /// Atomic child-preparation append identity.
    pub preparation_batch_id: AppendBatchId,
    /// Durable child-preparation record identity.
    pub preparation_record_id: RecordId,
    /// Reservation-request record identity when shared budget is requested.
    pub reservation_request_record_id: Option<RecordId>,
    /// Separate reservation-settlement append and record identities.
    pub reservation_settlement: Option<BudgetOperationIds>,
}

/// Stable append and record identities for one budget operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetOperationIds {
    /// Append identity.
    pub batch_id: AppendBatchId,
    /// Record identity.
    pub record_id: RecordId,
}

/// Runtime coordinator for commit-before-invoke child-agent handshakes.
pub struct ChildRunCoordinator {
    invoker: Arc<dyn AgentInvoker>,
    budget_ledger: Option<Arc<dyn BudgetLedger>>,
}

impl ChildRunCoordinator {
    /// Construct a coordinator without shared-budget support.
    #[must_use]
    pub fn new(invoker: Arc<dyn AgentInvoker>) -> Self {
        Self {
            invoker,
            budget_ledger: None,
        }
    }

    /// Install the exact shared-budget ledger used by this resolved agent.
    #[must_use]
    pub fn with_budget_ledger(mut self, budget_ledger: Arc<dyn BudgetLedger>) -> Self {
        self.budget_ledger = Some(budget_ledger);
        self
    }

    /// Prepare one exact child, settle any required reservation, then invoke.
    ///
    /// Equal retries attach to the same durable mapping. The invoker is never
    /// called before the parent journal contains both the child mapping and any
    /// required reservation receipt.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed coordination error for invalid requests,
    /// conflicting durable identities, unavailable services, or mismatched
    /// receipts/acceptances.
    pub async fn start_or_attach(
        &self,
        commit: &mut CommitCoordinator,
        context: ChildRunContext,
        request: ChildRunRequest,
        reservation: Option<BudgetReserveRequest>,
        ids: ChildCoordinationIds,
        timestamp: Timestamp,
    ) -> Result<ChildRunHandle, CompositionError> {
        request.validate().map_err(CompositionError::Agent)?;
        validate_parent(commit, &context.parent)?;
        validate_child_placement(&context.parent, &request)?;
        validate_reservation_shape(&request, reservation.as_ref(), ids)?;
        if let Some(existing) = commit
            .session()
            .child_mapping(context.parent.run_id, context.parent_effect_id)
            && (existing.child != request.locator
                || existing.request_digest != request.request_digest
                || existing.placement != request.placement)
        {
            return Err(CompositionError::Commit(
                CommitCoordinatorError::SidecarConflict,
            ));
        }

        let prepared = ChildRunPrepared {
            parent_run_id: context.parent.run_id,
            parent_effect_id: context.parent_effect_id,
            child: request.locator.clone(),
            request_digest: request.request_digest,
            placement: request.placement,
            budget_reservation_id: reservation.as_ref().map(|value| value.reservation_id),
        };
        prepared
            .validate(&context.parent.tenant_scope)
            .map_err(|_| CompositionError::InvalidRequest {
                code: "child_preparation_invalid",
            })?;

        let mut records = vec![record_draft(
            &context.parent,
            ids.preparation_record_id,
            timestamp,
            RecordBody::ChildRunPrepared(prepared),
        )?];
        if let Some(reserve) = reservation.as_ref() {
            records.push(record_draft(
                &context.parent,
                ids.reservation_request_record_id
                    .ok_or(CompositionError::InvalidRequest {
                        code: "reservation_request_record_id_missing",
                    })?,
                timestamp,
                RecordBody::BudgetReservationRequested(BudgetReservationRequested {
                    request: reserve.clone(),
                }),
            )?);
        }
        commit
            .commit_composition_records(ids.preparation_batch_id, records)
            .await
            .map_err(CompositionError::Commit)?;

        if let Some(reserve) = reservation {
            self.settle_reservation(commit, &context.parent, reserve, ids, timestamp)
                .await?;
        }
        ensure_prepared_for_invoke(commit, &context, &request)?;

        let expected_relation = child_relation_digest(&context, &request)?;
        let handle = self
            .invoker
            .start_or_attach(context, request.clone())
            .await
            .map_err(CompositionError::Agent)?;
        if handle.locator != request.locator || handle.relation_digest != expected_relation {
            return Err(CompositionError::Agent(AgentInvokeError::InvalidRequest {
                code: AGENT_INVOKE_INVALID_ACCEPTANCE,
                message: Arc::from("child acceptance does not match committed relation"),
            }));
        }
        Ok(handle)
    }

    async fn settle_reservation(
        &self,
        commit: &mut CommitCoordinator,
        parent: &OperationLocator,
        request: BudgetReserveRequest,
        ids: ChildCoordinationIds,
        timestamp: Timestamp,
    ) -> Result<(), CompositionError> {
        if commit
            .state()
            .budget_reservations
            .get(&request.reservation_id)
            .and_then(|value| value.settlement.as_ref())
            .is_some()
        {
            return Ok(());
        }
        let ledger = self
            .budget_ledger
            .as_ref()
            .ok_or(CompositionError::ServiceMissing {
                service: "budget_ledger",
            })?;
        let receipt = match ledger
            .reconcile(request.scope_id, request.reservation_id)
            .await
            .map_err(CompositionError::Budget)?
        {
            BudgetReservationState::Reserved(receipt) => receipt,
            BudgetReservationState::NotFound => ledger
                .reserve(request.clone())
                .await
                .map_err(CompositionError::Budget)?,
            BudgetReservationState::Released(_) => {
                return Err(CompositionError::Conflict {
                    code: "reservation_already_released",
                });
            }
            BudgetReservationState::Unknown => {
                return Err(CompositionError::Conflict {
                    code: "reservation_outcome_unknown",
                });
            }
        };
        validate_reservation_receipt(&request, &receipt)?;
        let settlement_ids =
            ids.reservation_settlement
                .ok_or(CompositionError::InvalidRequest {
                    code: "reservation_settlement_ids_missing",
                })?;
        let record = record_draft(
            parent,
            settlement_ids.record_id,
            timestamp,
            RecordBody::BudgetReservationSettled(BudgetReservationSettled { receipt }),
        )?;
        commit
            .commit_composition_records(settlement_ids.batch_id, vec![record])
            .await
            .map_err(CompositionError::Commit)?;
        Ok(())
    }
}

/// Runtime coordinator for post-commit shared-budget charge and release calls.
pub struct BudgetCoordinator {
    ledger: Arc<dyn BudgetLedger>,
}

impl BudgetCoordinator {
    /// Construct a coordinator over the resolved direct ledger handle.
    #[must_use]
    pub fn new(ledger: Arc<dyn BudgetLedger>) -> Self {
        Self { ledger }
    }

    /// Charge usage only after its effect settlement is durable, then journal the receipt.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error for uncommitted usage, invalid receipts, or
    /// journal/service failures.
    pub async fn charge_committed(
        &self,
        commit: &mut CommitCoordinator,
        operation: &OperationLocator,
        request: BudgetChargeRequest,
        ids: BudgetOperationIds,
        timestamp: Timestamp,
    ) -> Result<BudgetChargeReceipt, CompositionError> {
        request
            .validate()
            .map_err(|_| CompositionError::InvalidRequest {
                code: "budget_charge_request_invalid",
            })?;
        validate_parent(commit, operation)?;
        let reservation = settled_reservation(commit, request.reservation_id, request.scope_id)?;
        if reservation.release.is_some() {
            return Err(CompositionError::Conflict {
                code: "reservation_already_released",
            });
        }
        if !commit
            .state()
            .model_settlements
            .contains_key(&request.effect_id)
            && !commit
                .state()
                .tool_settlements
                .contains_key(&request.effect_id)
        {
            return Err(CompositionError::InvalidRequest {
                code: "effect_usage_not_committed",
            });
        }
        if let Some(existing) = commit.state().budget_charges.get(&request.effect_id) {
            validate_charge_receipt(&request, existing)?;
            return Ok(existing.clone());
        }
        let receipt = self
            .ledger
            .charge(request.clone())
            .await
            .map_err(CompositionError::Budget)?;
        validate_charge_receipt(&request, &receipt)?;
        let record = record_draft(
            operation,
            ids.record_id,
            timestamp,
            RecordBody::BudgetChargeRecorded(BudgetChargeRecorded {
                receipt: receipt.clone(),
            }),
        )?;
        commit
            .commit_composition_records(ids.batch_id, vec![record])
            .await
            .map_err(CompositionError::Commit)?;
        Ok(receipt)
    }

    /// Release unused allowance only after terminal commitment, then journal the receipt.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error for a non-terminal run, invalid receipt, or
    /// journal/service failure.
    pub async fn release_committed(
        &self,
        commit: &mut CommitCoordinator,
        operation: &OperationLocator,
        request: BudgetReleaseRequest,
        ids: BudgetOperationIds,
        timestamp: Timestamp,
    ) -> Result<BudgetReleaseReceipt, CompositionError> {
        request
            .validate()
            .map_err(|_| CompositionError::InvalidRequest {
                code: "budget_release_request_invalid",
            })?;
        validate_parent(commit, operation)?;
        if commit.state().terminal.is_none()
            || commit
                .state()
                .accepted
                .as_ref()
                .is_none_or(|accepted| accepted.run_id() != request.terminal_run_id)
        {
            return Err(CompositionError::InvalidRequest {
                code: "terminal_release_not_committed",
            });
        }
        let reservation = settled_reservation(commit, request.reservation_id, request.scope_id)?;
        if let Some(existing) = reservation.release.as_ref() {
            validate_release_receipt(&request, existing)?;
            return Ok(existing.clone());
        }
        let receipt = self
            .ledger
            .release(request.clone())
            .await
            .map_err(CompositionError::Budget)?;
        validate_release_receipt(&request, &receipt)?;
        let record = record_draft(
            operation,
            ids.record_id,
            timestamp,
            RecordBody::BudgetReservationReleased(BudgetReservationReleased {
                receipt: receipt.clone(),
            }),
        )?;
        commit
            .commit_composition_records(ids.batch_id, vec![record])
            .await
            .map_err(CompositionError::Commit)?;
        Ok(receipt)
    }
}

/// Compute the exact parent/child relationship digest accepted by an invoker.
///
/// # Errors
///
/// Returns an invalid-request error if canonical encoding unexpectedly fails.
pub fn child_relation_digest(
    context: &ChildRunContext,
    request: &ChildRunRequest,
) -> Result<Digest, CompositionError> {
    let canonical = serde_json_canonicalizer::to_vec(&(
        &context.parent,
        context.parent_effect_id,
        &request.locator,
        request.request_digest,
    ))
    .map_err(|_| CompositionError::InvalidRequest {
        code: "child_relation_not_serializable",
    })?;
    Digest::domain_separated("child-run-relation", 1, &canonical).map_err(|_| {
        CompositionError::InvalidRequest {
            code: "child_relation_not_serializable",
        }
    })
}

fn validate_parent(
    commit: &CommitCoordinator,
    operation: &OperationLocator,
) -> Result<(), CompositionError> {
    if commit.state().session_id != Some(operation.session_id)
        || commit.state().lane_id != Some(operation.lane_id)
        || commit
            .state()
            .accepted
            .as_ref()
            .is_none_or(|accepted| accepted.run_id() != operation.run_id)
    {
        return Err(CompositionError::InvalidRequest {
            code: "parent_operation_not_committed",
        });
    }
    Ok(())
}

fn validate_reservation_shape(
    child: &ChildRunRequest,
    reservation: Option<&BudgetReserveRequest>,
    ids: ChildCoordinationIds,
) -> Result<(), CompositionError> {
    let budget_requested = child.requested_budget != BudgetRequest::default();
    if budget_requested != reservation.is_some()
        || reservation.is_some() != ids.reservation_request_record_id.is_some()
        || reservation.is_some() != ids.reservation_settlement.is_some()
    {
        return Err(CompositionError::InvalidRequest {
            code: "child_budget_shape_invalid",
        });
    }
    if let Some(request) = reservation {
        request
            .validate()
            .map_err(|_| CompositionError::InvalidRequest {
                code: "budget_reserve_request_invalid",
            })?;
        if request.run_id != child.locator.operation.run_id
            || request.amount != child.requested_budget
        {
            return Err(CompositionError::InvalidRequest {
                code: "child_budget_request_mismatch",
            });
        }
    }
    Ok(())
}

fn validate_child_placement(
    parent: &OperationLocator,
    child: &ChildRunRequest,
) -> Result<(), CompositionError> {
    let same_session = child.locator.operation.session_id == parent.session_id;
    let valid = match child.placement {
        crate::ChildPlacement::CompatibleLaneInParentSession => {
            same_session && child.locator.operation.lane_id != parent.lane_id
        }
        crate::ChildPlacement::IsolatedChildSession | crate::ChildPlacement::RemoteChildSession => {
            !same_session
        }
    };
    if !valid {
        return Err(CompositionError::InvalidRequest {
            code: "child_placement_locator_mismatch",
        });
    }
    Ok(())
}

fn ensure_prepared_for_invoke(
    commit: &CommitCoordinator,
    context: &ChildRunContext,
    request: &ChildRunRequest,
) -> Result<(), CompositionError> {
    let prepared = commit
        .state()
        .child_preparations
        .get(&context.parent_effect_id)
        .ok_or(CompositionError::InvalidRequest {
            code: "child_preparation_not_committed",
        })?;
    if prepared.parent_run_id != context.parent.run_id
        || prepared.child != request.locator
        || prepared.request_digest != request.request_digest
        || prepared.placement != request.placement
    {
        return Err(CompositionError::Conflict {
            code: "child_preparation_conflict",
        });
    }
    if let Some(reservation_id) = prepared.budget_reservation_id
        && commit
            .state()
            .budget_reservations
            .get(&reservation_id)
            .and_then(|value| value.settlement.as_ref())
            .is_none()
    {
        return Err(CompositionError::InvalidRequest {
            code: "reservation_not_settled",
        });
    }
    Ok(())
}

fn settled_reservation(
    commit: &CommitCoordinator,
    reservation_id: crate::BudgetReservationId,
    scope_id: crate::BudgetScopeId,
) -> Result<&crate::BudgetReservationReplay, CompositionError> {
    let reservation = commit
        .state()
        .budget_reservations
        .get(&reservation_id)
        .ok_or(CompositionError::InvalidRequest {
            code: "reservation_not_committed",
        })?;
    if reservation.request.scope_id != scope_id || reservation.settlement.is_none() {
        return Err(CompositionError::InvalidRequest {
            code: "reservation_not_settled",
        });
    }
    Ok(reservation)
}

fn validate_reservation_receipt(
    request: &BudgetReserveRequest,
    receipt: &crate::BudgetReservationReceipt,
) -> Result<(), CompositionError> {
    receipt
        .validate()
        .map_err(|_| CompositionError::InvalidReceipt {
            code: "budget_reservation_receipt_invalid",
        })?;
    if receipt.scope_id != request.scope_id
        || receipt.reservation_id != request.reservation_id
        || receipt.reserved != request.amount
        || receipt.request_digest != request.request_digest
    {
        return Err(CompositionError::InvalidReceipt {
            code: "budget_reservation_receipt_mismatch",
        });
    }
    Ok(())
}

fn validate_charge_receipt(
    request: &BudgetChargeRequest,
    receipt: &BudgetChargeReceipt,
) -> Result<(), CompositionError> {
    receipt
        .validate()
        .map_err(|_| CompositionError::InvalidReceipt {
            code: "budget_charge_receipt_invalid",
        })?;
    if receipt.scope_id != request.scope_id
        || receipt.reservation_id != request.reservation_id
        || receipt.effect_id != request.effect_id
        || receipt.charged_usage != request.usage
        || receipt.usage_digest != request.usage_digest
    {
        return Err(CompositionError::InvalidReceipt {
            code: "budget_charge_receipt_mismatch",
        });
    }
    Ok(())
}

fn validate_release_receipt(
    request: &BudgetReleaseRequest,
    receipt: &BudgetReleaseReceipt,
) -> Result<(), CompositionError> {
    receipt
        .validate()
        .map_err(|_| CompositionError::InvalidReceipt {
            code: "budget_release_receipt_invalid",
        })?;
    if receipt.scope_id != request.scope_id
        || receipt.reservation_id != request.reservation_id
        || receipt.terminal_run_id != request.terminal_run_id
        || receipt.request_digest != request.request_digest
    {
        return Err(CompositionError::InvalidReceipt {
            code: "budget_release_receipt_mismatch",
        });
    }
    Ok(())
}

fn record_draft(
    operation: &OperationLocator,
    record_id: RecordId,
    timestamp: Timestamp,
    body: RecordBody,
) -> Result<RecordDraft, CompositionError> {
    RecordDraft::try_new(
        crate::RECORD_FORMAT_VERSION,
        crate::RECORD_KIND_VERSION,
        record_id,
        operation.session_id,
        operation.lane_id,
        Some(operation.run_id),
        timestamp,
        Vec::new(),
        body,
    )
    .map_err(|_| CompositionError::InvalidRequest {
        code: "composition_record_invalid",
    })
}

/// Commit-aware child/budget coordination failure.
#[derive(Debug, Error)]
pub enum CompositionError {
    /// A normalized request violates a precondition.
    #[error("invalid composition request: {code}")]
    InvalidRequest {
        /// Stable machine-readable code.
        code: &'static str,
    },
    /// A required resolved runtime service is absent.
    #[error("required runtime service missing: {service}")]
    ServiceMissing {
        /// Stable service name.
        service: &'static str,
    },
    /// An idempotency identity is already bound to different content.
    #[error("composition identity conflict: {code}")]
    Conflict {
        /// Stable machine-readable code.
        code: &'static str,
    },
    /// A service returned a malformed or mismatched receipt.
    #[error("invalid composition receipt: {code}")]
    InvalidReceipt {
        /// Stable machine-readable code.
        code: &'static str,
    },
    /// Parent journal commit/apply failure.
    #[error(transparent)]
    Commit(#[from] CommitCoordinatorError),
    /// Child-agent service failure.
    #[error(transparent)]
    Agent(#[from] AgentInvokeError),
    /// Shared-budget service failure.
    #[error(transparent)]
    Budget(#[from] BudgetError),
}
