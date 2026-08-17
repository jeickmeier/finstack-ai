//! Relation-shape and child-attenuation validation.

use crate::primitives::Timestamp;
use crate::primitives::{EffectId, RunId};
use crate::records::policy::RunLimits;

use super::MAX_RUN_RELATION_DEPTH;
use super::accepted::RunAccepted;
use super::error::RunError;
use super::propagation::{BudgetPropagation, PrincipalPropagation};
use super::relation::{RunRelation, RunRelationKind};
use super::security::RunSecurityContext;

pub(super) fn validate_relation_shape(
    kind: RunRelationKind,
    parent_run_id: Option<RunId>,
    parent_effect_id: Option<EffectId>,
    depth: u16,
) -> Result<(), RunError> {
    if depth > MAX_RUN_RELATION_DEPTH {
        return Err(RunError::InvalidRelation {
            reason: "depth exceeds MAX_RUN_RELATION_DEPTH",
        });
    }
    match kind {
        RunRelationKind::Root => {
            if depth != 0 || parent_run_id.is_some() || parent_effect_id.is_some() {
                return Err(RunError::InvalidRelation {
                    reason: "root requires depth 0 and no parent ids",
                });
            }
        }
        RunRelationKind::ChildAgent
        | RunRelationKind::DelegatedAgent
        | RunRelationKind::WorkflowStep => {
            if depth == 0 || parent_run_id.is_none() || parent_effect_id.is_none() {
                return Err(RunError::InvalidRelation {
                    reason: "non-root requires depth>=1 and both parent ids",
                });
            }
        }
    }
    Ok(())
}

pub(super) fn validate_child_against_parent(
    relation: &RunRelation,
    security: &RunSecurityContext,
    effective_deadline: Option<Timestamp>,
    limits: &RunLimits,
    parent: &RunAccepted,
) -> Result<(), RunError> {
    if relation.root_run_id() != parent.relation().root_run_id() {
        return Err(RunError::NotAttenuated {
            reason: "root_run_id mismatch",
        });
    }
    if relation.parent_run_id() != Some(parent.run_id()) {
        return Err(RunError::NotAttenuated {
            reason: "parent_run_id mismatch",
        });
    }
    if relation.depth() != parent.relation().depth().saturating_add(1) {
        return Err(RunError::NotAttenuated {
            reason: "depth must be parent.depth + 1",
        });
    }
    if !parent
        .security()
        .allows_child_attenuation(security, parent.propagation().principal)
    {
        return Err(RunError::NotAttenuated {
            reason: "security context not attenuated",
        });
    }
    match parent.propagation().principal {
        PrincipalPropagation::Inherit => {}
        PrincipalPropagation::AttenuatedDelegation => {
            if security.principal() != parent.security().principal()
                && relation.kind() != RunRelationKind::DelegatedAgent
            {
                return Err(RunError::NotAttenuated {
                    reason: "delegated principal requires delegated-agent relation",
                });
            }
        }
    }
    if !parent.limits().allows_child_attenuation(limits) {
        return Err(RunError::NotAttenuated {
            reason: "limits not attenuated",
        });
    }
    match (parent.effective_deadline(), effective_deadline) {
        (Some(_), None) => {
            return Err(RunError::NotAttenuated {
                reason: "child removed parent deadline",
            });
        }
        (Some(parent_deadline), Some(child_deadline)) if child_deadline > parent_deadline => {
            return Err(RunError::NotAttenuated {
                reason: "deadline exceeds parent",
            });
        }
        _ => {}
    }
    match parent.propagation().budget {
        BudgetPropagation::SharedScope => {
            if relation.budget_scope_id() != parent.relation().budget_scope_id() {
                return Err(RunError::NotAttenuated {
                    reason: "shared budget scope changed",
                });
            }
        }
        BudgetPropagation::ReservedChildAllocation => {
            if relation.budget_scope_id().is_none()
                || relation.budget_scope_id() == parent.relation().budget_scope_id()
            {
                return Err(RunError::NotAttenuated {
                    reason: "reserved child allocation requires a distinct budget scope",
                });
            }
        }
    }
    Ok(())
}
