//! Accepted run identity and lineage validation.

use serde::de;
use serde::{Deserialize, Serialize};

use crate::primitives::Digest;
use crate::primitives::RunId;
use crate::primitives::Timestamp;
use crate::records::policy::RunLimits;

use super::error::RunError;
use super::propagation::RunPropagationPolicy;
use super::relation::{RunRelation, RunRelationKind};
use super::security::RunSecurityContext;
use super::validate::{validate_child_against_parent, validate_relation_shape};

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
    /// # Arguments
    ///
    /// * `run_id` - Identity of the accepted run.
    /// * `relation` - Immutable lineage. Roots must match `run_id` and omit a parent.
    /// * `security` - Tenant, principal, and authorization captured at acceptance.
    /// * `effective_deadline` - Optional semantic deadline after parent/child
    ///   attenuation; `None` means no deadline.
    /// * `limits` - Value-only run limits.
    /// * `propagation` - How cancellation, deadline, budget, and principal propagate.
    /// * `resolved_agent_lock_digest` - Digest of the immutable resolved agent spec.
    /// * `parent` - Parent accepted run for child attenuation; `None` for roots.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] for invalid lineage or when a child fails attenuation checks
    /// against an optional parent context.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     BudgetPropagation, CancellationPropagation, DeadlinePropagation, Digest, PrincipalPropagation,
    ///     PrincipalRef, RunAccepted, RunId, RunLimits, RunPropagationPolicy, RunRelation,
    ///     RunSecurityContext,
    /// };
    ///
    /// # let run_id = RunId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    /// # let security = RunSecurityContext::try_new(
    /// #     "tenant",
    /// #     PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal"),
    /// #     "oidc",
    /// #     "high",
    /// #     "policy-v1",
    /// #     "decision-v1",
    /// #     None,
    /// # ).expect("security");
    /// let accepted = RunAccepted::try_new(
    ///     run_id,
    ///     RunRelation::root(run_id).expect("root"),
    ///     security,
    ///     None,
    ///     RunLimits::empty(),
    ///     RunPropagationPolicy {
    ///         cancellation: CancellationPropagation::Cascade,
    ///         deadline: DeadlinePropagation::MinimumOfParentAndChild,
    ///         budget: BudgetPropagation::SharedScope,
    ///         principal: PrincipalPropagation::Inherit,
    ///     },
    ///     Digest::raw_json(br#"{"agent":"example"}"#),
    ///     None,
    /// )
    /// .expect("accepted");
    /// assert_eq!(accepted.run_id(), run_id);
    /// ```
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

    pub(crate) fn mark_persisted_lineage_validated(mut self) -> Self {
        self.lineage_validated = LineageValidation(true);
        self
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
