//! Run lineage, security context, and `RunAccepted` (TDD §11.5).

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::digest::Digest;
use crate::ids::{BudgetScopeId, EffectId, RunId};
use crate::limits::{LimitsError, RunLimits};
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
            external_work_ref: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.root_run_id,
            wire.parent_run_id,
            wire.parent_effect_id,
            wire.kind,
            wire.depth,
            wire.budget_scope_id,
            wire.external_work_ref.map(BoundedString::into_inner),
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
        let tenant_scope = validated_label(tenant_scope.as_ref(), "tenant_scope")?;
        if principal
            .tenant_scope()
            .is_some_and(|principal_tenant| principal_tenant != tenant_scope.as_ref())
            || delegated_from.as_ref().is_some_and(|delegated| {
                delegated
                    .tenant_scope()
                    .is_some_and(|delegated_tenant| delegated_tenant != tenant_scope.as_ref())
            })
        {
            return Err(RunError::InvalidSecurityContext {
                reason: "principal tenant scope does not match run tenant scope",
            });
        }
        Ok(Self {
            tenant_scope,
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

    /// Authentication method captured at ingress.
    #[must_use]
    pub fn authentication_method(&self) -> &str {
        &self.authentication_method
    }

    /// Authentication assurance level.
    #[must_use]
    pub fn assurance_level(&self) -> &str {
        &self.assurance_level
    }

    /// Authorization-policy version.
    #[must_use]
    pub fn authorization_policy_version(&self) -> &str {
        &self.authorization_policy_version
    }

    /// Authorization decision id.
    #[must_use]
    pub fn authorization_decision_id(&self) -> &str {
        &self.authorization_decision_id
    }

    /// Borrow delegated-from principal.
    #[must_use]
    pub fn delegated_from(&self) -> Option<&PrincipalRef> {
        self.delegated_from.as_ref()
    }

    /// True when `child` preserves tenant and attenuates principal/delegation.
    #[must_use]
    pub fn allows_child_attenuation(&self, child: &Self, policy: PrincipalPropagation) -> bool {
        if child.tenant_scope() != self.tenant_scope() {
            return false;
        }
        match policy {
            PrincipalPropagation::Inherit => {
                child.principal() == self.principal()
                    && child.delegated_from() == self.delegated_from()
            }
            PrincipalPropagation::AttenuatedDelegation => {
                if child.principal() == self.principal() {
                    child.delegated_from() == self.delegated_from()
                } else {
                    child.delegated_from() == Some(self.principal())
                }
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
            tenant_scope: BoundedString<LABEL_MAX_BYTES>,
            principal: PrincipalRef,
            authentication_method: BoundedString<LABEL_MAX_BYTES>,
            assurance_level: BoundedString<LABEL_MAX_BYTES>,
            authorization_policy_version: BoundedString<LABEL_MAX_BYTES>,
            authorization_decision_id: BoundedString<LABEL_MAX_BYTES>,
            #[serde(default)]
            delegated_from: Option<PrincipalRef>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.tenant_scope.into_inner(),
            wire.principal,
            wire.authentication_method.into_inner(),
            wire.assurance_level.into_inner(),
            wire.authorization_policy_version.into_inner(),
            wire.authorization_decision_id.into_inner(),
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
    #[serde(skip)]
    lineage_validated: LineageValidation,
}

#[derive(Debug, Clone, Copy, Default)]
struct LineageValidation(bool);

impl PartialEq for LineageValidation {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for LineageValidation {}

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
        limits.validate()?;
        if relation.kind() == RunRelationKind::Root {
            if parent.is_some() {
                return Err(RunError::InvalidRelation {
                    reason: "root run must not provide parent context",
                });
            }
            if relation.root_run_id() != run_id {
                return Err(RunError::InvalidRelation {
                    reason: "root relation root_run_id must equal run_id",
                });
            }
        } else {
            let parent = parent.ok_or(RunError::InvalidRelation {
                reason: "non-root run requires parent context",
            })?;
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
            lineage_validated: LineageValidation(true),
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

    /// Validate a structurally decoded non-root run against its parent.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] when lineage, principal, deadline, limits, or budget
    /// propagation is not attenuated.
    pub fn validate_against_parent(mut self, parent: &Self) -> Result<Self, RunError> {
        if self.relation.kind() == RunRelationKind::Root {
            return Err(RunError::InvalidRelation {
                reason: "root run does not require parent validation",
            });
        }
        validate_child_against_parent(
            &self.relation,
            &self.security,
            self.effective_deadline,
            &self.limits,
            parent,
        )?;
        self.lineage_validated = LineageValidation(true);
        Ok(self)
    }

    pub(crate) fn lineage_is_validated(&self) -> bool {
        self.lineage_validated.0
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
        let lineage_validated = wire.relation.kind() == RunRelationKind::Root;
        Ok(Self {
            run_id: wire.run_id,
            relation: wire.relation,
            security: wire.security,
            effective_deadline: wire.effective_deadline,
            limits: wire.limits,
            propagation: wire.propagation,
            resolved_agent_lock_digest: wire.resolved_agent_lock_digest,
            lineage_validated: LineageValidation(lineage_validated),
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
    /// Security context fields are internally inconsistent.
    #[error("invalid run security context: {reason}")]
    InvalidSecurityContext {
        /// Reason.
        reason: &'static str,
    },
    /// Shared label/ref error.
    #[error(transparent)]
    Refs(#[from] RefsError),
    /// Run limits failed semantic validation.
    #[error(transparent)]
    Limits(#[from] LimitsError),
}

impl RunError {
    /// Stable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRelation { .. } => "invalid_run_relation",
            Self::NotAttenuated { .. } => "run_not_attenuated",
            Self::InvalidSecurityContext { .. } => "invalid_run_security_context",
            Self::Refs(inner) => inner.code(),
            Self::Limits(inner) => inner.code(),
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
        let json = serde_json::to_string(&child).expect("serialize child");
        let decoded: RunAccepted = serde_json::from_str(&json).expect("decode child");
        assert!(!decoded.lineage_is_validated());
        let revalidated = decoded
            .validate_against_parent(&root)
            .expect("revalidate child");
        assert!(revalidated.lineage_is_validated());

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

    #[test]
    fn non_root_run_requires_parent_context() {
        let root_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("root");
        let child_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("child");
        let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("effect");
        let relation = RunRelation::try_new(
            root_run,
            Some(root_run),
            Some(effect),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("relation");
        let err = RunAccepted::try_new(
            child_run,
            relation,
            sample_security(),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br"{}"),
            None,
        )
        .expect_err("parent context required");
        assert_eq!(err.code(), "invalid_run_relation");
    }

    #[test]
    fn inherited_principal_cannot_change_via_delegated_from_marker() {
        let parent_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("parent");
        let child_run = RunId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("child");
        let effect = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("effect");
        let parent = RunAccepted::try_new(
            parent_run,
            RunRelation::root(parent_run).expect("root"),
            sample_security(),
            None,
            RunLimits::empty(),
            RunPropagationPolicy {
                cancellation: CancellationPropagation::Cascade,
                deadline: DeadlinePropagation::MinimumOfParentAndChild,
                budget: BudgetPropagation::SharedScope,
                principal: PrincipalPropagation::Inherit,
            },
            Digest::raw_json(br"{}"),
            None,
        )
        .expect("parent");
        let child_security = RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("iss", "other", Some("tenant-a")).expect("child principal"),
            "oidc",
            "high",
            "policy-1",
            "decision-2",
            Some(parent.security().principal().clone()),
        )
        .expect("child security");
        let relation = RunRelation::try_new(
            parent_run,
            Some(parent_run),
            Some(effect),
            RunRelationKind::ChildAgent,
            1,
            None,
            None::<&str>,
        )
        .expect("relation");
        let err = RunAccepted::try_new(
            child_run,
            relation,
            child_security,
            None,
            RunLimits::empty(),
            parent.propagation(),
            Digest::raw_json(br"{}"),
            Some(&parent),
        )
        .expect_err("inherit cannot change principal");
        assert_eq!(err.code(), "run_not_attenuated");
    }

    #[test]
    fn security_context_rejects_mismatched_principal_tenant() {
        let err = RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("iss", "sub", Some("tenant-b")).expect("principal"),
            "oidc",
            "high",
            "policy-1",
            "decision-1",
            None,
        )
        .expect_err("tenant mismatch");
        assert_eq!(err.code(), "invalid_run_security_context");
    }
}
