//! Run lineage, security context, and `RunAccepted` (TDD §11.5).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::digest::Digest;
use crate::ids::{BudgetScopeId, EffectId, RunId};
use crate::limits::RunLimits;
use crate::refs::{PrincipalRef, RefsError, validated_label};
use crate::time::Timestamp;

/// V1 maximum `RunRelation.depth` (root is 0).
pub const MAX_RUN_RELATION_DEPTH: u16 = 16;

/// How a run relates to its root/parent lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunRelationKind {
    /// Root run.
    Root,
    /// Child agent invocation.
    ChildAgent,
    /// Delegated agent invocation.
    DelegatedAgent,
    /// Workflow step.
    WorkflowStep,
}

/// Immutable run lineage relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunRelation {
    root_run_id: RunId,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_effect_id: Option<EffectId>,
    kind: RunRelationKind,
    depth: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_scope_id: Option<BudgetScopeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    external_work_ref: Option<Arc<str>>,
}

impl RunRelation {
    /// Construct a validated run relation.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] for invalid root/parent shape, depth, or labels.
    pub fn try_new(
        root_run_id: RunId,
        parent_run_id: Option<RunId>,
        parent_effect_id: Option<EffectId>,
        kind: RunRelationKind,
        depth: u16,
        budget_scope_id: Option<BudgetScopeId>,
        external_work_ref: Option<impl AsRef<str>>,
    ) -> Result<Self, RunError> {
        validate_relation_shape(kind, parent_run_id, parent_effect_id, depth)?;
        let external_work_ref = match external_work_ref {
            Some(value) => Some(validated_label(value.as_ref(), "external_work_ref")?),
            None => None,
        };
        Ok(Self {
            root_run_id,
            parent_run_id,
            parent_effect_id,
            kind,
            depth,
            budget_scope_id,
            external_work_ref,
        })
    }

    /// Construct a root relation for `run_id` at depth 0.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] only if label validation fails (not expected for roots).
    pub fn root(run_id: RunId) -> Result<Self, RunError> {
        Self::try_new(
            run_id,
            None,
            None,
            RunRelationKind::Root,
            0,
            None,
            None::<&str>,
        )
    }

    /// Borrow root run id.
    #[must_use]
    pub fn root_run_id(&self) -> RunId {
        self.root_run_id
    }

    /// Borrow parent run id.
    #[must_use]
    pub fn parent_run_id(&self) -> Option<RunId> {
        self.parent_run_id
    }

    /// Borrow parent effect id.
    #[must_use]
    pub fn parent_effect_id(&self) -> Option<EffectId> {
        self.parent_effect_id
    }

    /// Relation kind.
    #[must_use]
    pub fn kind(&self) -> RunRelationKind {
        self.kind
    }

    /// Depth.
    #[must_use]
    pub fn depth(&self) -> u16 {
        self.depth
    }

    /// Budget scope.
    #[must_use]
    pub fn budget_scope_id(&self) -> Option<BudgetScopeId> {
        self.budget_scope_id
    }

    /// External work ref.
    #[must_use]
    pub fn external_work_ref(&self) -> Option<&str> {
        self.external_work_ref.as_deref()
    }
}

impl<'de> Deserialize<'de> for RunRelation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            root_run_id: RunId,
            #[serde(default)]
            parent_run_id: Option<RunId>,
            #[serde(default)]
            parent_effect_id: Option<EffectId>,
            kind: RunRelationKind,
            depth: u16,
            #[serde(default)]
            budget_scope_id: Option<BudgetScopeId>,
            #[serde(default)]
            external_work_ref: Option<String>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.root_run_id,
            wire.parent_run_id,
            wire.parent_effect_id,
            wire.kind,
            wire.depth,
            wire.budget_scope_id,
            wire.external_work_ref,
        )
        .map_err(de::Error::custom)
    }
}

/// Durable security context captured at run acceptance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunSecurityContext {
    tenant_scope: Arc<str>,
    principal: PrincipalRef,
    authentication_method: Arc<str>,
    assurance_level: Arc<str>,
    authorization_policy_version: Arc<str>,
    authorization_decision_id: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delegated_from: Option<PrincipalRef>,
}

impl RunSecurityContext {
    /// Construct a security context.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] when labels fail validation.
    pub fn try_new(
        tenant_scope: impl AsRef<str>,
        principal: PrincipalRef,
        authentication_method: impl AsRef<str>,
        assurance_level: impl AsRef<str>,
        authorization_policy_version: impl AsRef<str>,
        authorization_decision_id: impl AsRef<str>,
        delegated_from: Option<PrincipalRef>,
    ) -> Result<Self, RunError> {
        Ok(Self {
            tenant_scope: validated_label(tenant_scope.as_ref(), "tenant_scope")?,
            principal,
            authentication_method: validated_label(
                authentication_method.as_ref(),
                "authentication_method",
            )?,
            assurance_level: validated_label(assurance_level.as_ref(), "assurance_level")?,
            authorization_policy_version: validated_label(
                authorization_policy_version.as_ref(),
                "authorization_policy_version",
            )?,
            authorization_decision_id: validated_label(
                authorization_decision_id.as_ref(),
                "authorization_decision_id",
            )?,
            delegated_from,
        })
    }

    /// Borrow tenant scope.
    #[must_use]
    pub fn tenant_scope(&self) -> &str {
        &self.tenant_scope
    }

    /// Borrow principal.
    #[must_use]
    pub fn principal(&self) -> &PrincipalRef {
        &self.principal
    }

    /// Borrow delegated-from principal.
    #[must_use]
    pub fn delegated_from(&self) -> Option<&PrincipalRef> {
        self.delegated_from.as_ref()
    }

    /// True when `child` preserves tenant and attenuates principal/delegation.
    #[must_use]
    pub fn allows_child_attenuation(&self, child: &Self) -> bool {
        if child.tenant_scope() != self.tenant_scope() {
            return false;
        }
        match (&self.delegated_from, &child.delegated_from) {
            (None, None) => child.principal() == self.principal(),
            (None, Some(parent)) => {
                parent == self.principal()
                    && (child.principal() == self.principal() || child.principal() != parent)
            }
            (Some(_), None) => false,
            (Some(parent_from), Some(child_from)) => {
                child_from == parent_from
                    || child.principal() == self.principal()
                    || child_from == self.principal()
            }
        }
    }
}

impl<'de> Deserialize<'de> for RunSecurityContext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            tenant_scope: String,
            principal: PrincipalRef,
            authentication_method: String,
            assurance_level: String,
            authorization_policy_version: String,
            authorization_decision_id: String,
            #[serde(default)]
            delegated_from: Option<PrincipalRef>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.tenant_scope,
            wire.principal,
            wire.authentication_method,
            wire.assurance_level,
            wire.authorization_policy_version,
            wire.authorization_decision_id,
            wire.delegated_from,
        )
        .map_err(de::Error::custom)
    }
}

/// Cancellation propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationPropagation {
    /// Cascade cancellation to children.
    Cascade,
    /// Detach only when preauthorized.
    DetachOnlyIfPreauthorized,
}

/// Deadline propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlinePropagation {
    /// Child deadline is min(parent, child).
    MinimumOfParentAndChild,
}

/// Budget propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetPropagation {
    /// Shared budget scope.
    SharedScope,
    /// Reserved child allocation.
    ReservedChildAllocation,
}

/// Principal propagation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalPropagation {
    /// Inherit parent principal.
    Inherit,
    /// Attenuated delegation.
    AttenuatedDelegation,
}

/// Propagation policy bundle on `RunAccepted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPropagationPolicy {
    /// Cancellation policy.
    pub cancellation: CancellationPropagation,
    /// Deadline policy.
    pub deadline: DeadlinePropagation,
    /// Budget policy.
    pub budget: BudgetPropagation,
    /// Principal policy.
    pub principal: PrincipalPropagation,
}

/// Durable run-accepted record body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunAccepted {
    run_id: RunId,
    relation: RunRelation,
    security: RunSecurityContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_deadline: Option<Timestamp>,
    limits: RunLimits,
    propagation: RunPropagationPolicy,
    resolved_agent_lock_digest: Digest,
}

impl RunAccepted {
    /// Construct a validated `RunAccepted` body.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] for invalid lineage or when a child fails attenuation checks
    /// against an optional parent context.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        run_id: RunId,
        relation: RunRelation,
        security: RunSecurityContext,
        effective_deadline: Option<Timestamp>,
        limits: RunLimits,
        propagation: RunPropagationPolicy,
        resolved_agent_lock_digest: Digest,
        parent: Option<&RunAccepted>,
    ) -> Result<Self, RunError> {
        if relation.kind() == RunRelationKind::Root {
            if relation.root_run_id() != run_id {
                return Err(RunError::InvalidRelation {
                    reason: "root relation root_run_id must equal run_id",
                });
            }
        } else if let Some(parent) = parent {
            validate_child_against_parent(
                &relation,
                &security,
                effective_deadline,
                &limits,
                parent,
            )?;
        }
        Ok(Self {
            run_id,
            relation,
            security,
            effective_deadline,
            limits,
            propagation,
            resolved_agent_lock_digest,
        })
    }

    /// Run id.
    #[must_use]
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// Relation.
    #[must_use]
    pub fn relation(&self) -> &RunRelation {
        &self.relation
    }

    /// Security context.
    #[must_use]
    pub fn security(&self) -> &RunSecurityContext {
        &self.security
    }

    /// Effective deadline.
    #[must_use]
    pub fn effective_deadline(&self) -> Option<Timestamp> {
        self.effective_deadline
    }

    /// Limits.
    #[must_use]
    pub fn limits(&self) -> &RunLimits {
        &self.limits
    }

    /// Propagation policy.
    #[must_use]
    pub fn propagation(&self) -> RunPropagationPolicy {
        self.propagation
    }

    /// Resolved-agent lock digest.
    #[must_use]
    pub fn resolved_agent_lock_digest(&self) -> Digest {
        self.resolved_agent_lock_digest
    }
}

impl<'de> Deserialize<'de> for RunAccepted {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            run_id: RunId,
            relation: RunRelation,
            security: RunSecurityContext,
            #[serde(default)]
            effective_deadline: Option<Timestamp>,
            limits: RunLimits,
            propagation: RunPropagationPolicy,
            resolved_agent_lock_digest: Digest,
        }
        let wire = Wire::deserialize(deserializer)?;
        // Parent attenuation is enforced by constructors that have parent context;
        // serde accepts structurally valid payloads (restart fixtures supply parents separately).
        if wire.relation.kind() == RunRelationKind::Root
            && wire.relation.root_run_id() != wire.run_id
        {
            return Err(de::Error::custom(
                "root relation root_run_id must equal run_id",
            ));
        }
        validate_relation_shape(
            wire.relation.kind(),
            wire.relation.parent_run_id(),
            wire.relation.parent_effect_id(),
            wire.relation.depth(),
        )
        .map_err(de::Error::custom)?;
        Ok(Self {
            run_id: wire.run_id,
            relation: wire.relation,
            security: wire.security,
            effective_deadline: wire.effective_deadline,
            limits: wire.limits,
            propagation: wire.propagation,
            resolved_agent_lock_digest: wire.resolved_agent_lock_digest,
        })
    }
}

/// Run lineage / acceptance errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RunError {
    /// Invalid relation shape or depth.
    #[error("invalid run relation: {reason}")]
    InvalidRelation {
        /// Reason.
        reason: &'static str,
    },
    /// Child failed attenuation against parent.
    #[error("child run is not an attenuation of parent: {reason}")]
    NotAttenuated {
        /// Reason.
        reason: &'static str,
    },
    /// Shared label/ref error.
    #[error(transparent)]
    Refs(#[from] RefsError),
}

impl RunError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRelation { .. } => "invalid_run_relation",
            Self::NotAttenuated { .. } => "run_not_attenuated",
            Self::Refs(inner) => inner.code(),
        }
    }
}

fn validate_relation_shape(
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

fn validate_child_against_parent(
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
    if !parent.security().allows_child_attenuation(security) {
        return Err(RunError::NotAttenuated {
            reason: "security context not attenuated",
        });
    }
    if !parent.limits().allows_child_attenuation(limits) {
        return Err(RunError::NotAttenuated {
            reason: "limits not attenuated",
        });
    }
    if let (Some(parent_deadline), Some(child_deadline)) =
        (parent.effective_deadline(), effective_deadline)
        && child_deadline > parent_deadline
    {
        return Err(RunError::NotAttenuated {
            reason: "deadline exceeds parent",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::EffectId;
    use crate::refs::PrincipalRef;

    fn sample_security() -> RunSecurityContext {
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("iss", "sub", Some("tenant-a")).expect("p"),
            "oidc",
            "high",
            "policy-1",
            "decision-1",
            None,
        )
        .expect("security")
    }

    #[test]
    fn root_and_child_lineage() {
        let run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("run");
        let root_rel = RunRelation::root(run).expect("root");
        let root = RunAccepted::try_new(
            run,
            root_rel,
            sample_security(),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br#"{"lock":1}"#),
            None,
        )
        .expect("root accepted");

        let child_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("child");
        let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("effect");
        let child_rel = RunRelation::try_new(
            run,
            Some(run),
            Some(effect),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("child rel");
        let child = RunAccepted::try_new(
            child_run,
            child_rel,
            sample_security(),
            None,
            RunLimits::empty(),
            root.propagation(),
            Digest::raw_json(br#"{"lock":1}"#),
            Some(&root),
        )
        .expect("child");
        assert_eq!(child.relation().depth(), 1);

        assert!(
            RunRelation::try_new(
                run,
                None,
                None,
                RunRelationKind::Root,
                17,
                None,
                None::<&str>,
            )
            .is_err()
        );

        let late = Timestamp::from_unix_ms(2).expect("ts");
        let early = Timestamp::from_unix_ms(1).expect("ts");
        let parent_with_deadline = RunAccepted::try_new(
            run,
            RunRelation::root(run).expect("root"),
            sample_security(),
            Some(early),
            RunLimits::empty(),
            root.propagation(),
            Digest::raw_json(br#"{"lock":1}"#),
            None,
        )
        .expect("parent deadline");
        let err = RunAccepted::try_new(
            child_run,
            child.relation().clone(),
            sample_security(),
            Some(late),
            RunLimits::empty(),
            root.propagation(),
            Digest::raw_json(br#"{"lock":1}"#),
            Some(&parent_with_deadline),
        )
        .expect_err("widened deadline");
        assert_eq!(err.code(), "run_not_attenuated");
    }
}
