//! Run security context.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::primitives::{ComponentRef, Digest, PrincipalRef, Sensitivity, validated_label};

use super::error::RunError;
use super::propagation::PrincipalPropagation;

/// Durable exact-model authorization for model-assisted compaction.
///
/// Absence of this lock on [`RunSecurityContext`] denies secondary-model
/// compaction. A child may omit the lock or inherit/attenuate it; it cannot
/// change the model or residency digest or raise [`Self::maximum_sensitivity`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionAuthorization {
    allowed_model: ComponentRef,
    maximum_sensitivity: Sensitivity,
    residency_policy_digest: Digest,
}

impl CompactionAuthorization {
    /// Construct a durable compaction authorization lock.
    ///
    /// # Arguments
    ///
    /// * `allowed_model` - Exact secondary model the run may invoke for summarization.
    /// * `maximum_sensitivity` - Highest source sensitivity that model may receive.
    /// * `residency_policy_digest` - Accepted residency/egress-policy digest.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     CompactionAuthorization, ComponentId, ComponentRef, Digest, Sensitivity,
    /// };
    ///
    /// let auth = CompactionAuthorization::new(
    ///     ComponentRef::new(ComponentId::parse("fixture.model").expect("id"), None),
    ///     Sensitivity::Internal,
    ///     Digest::raw_json(b"residency"),
    /// );
    /// assert_eq!(auth.maximum_sensitivity(), Sensitivity::Internal);
    /// ```
    #[must_use]
    pub fn new(
        allowed_model: ComponentRef,
        maximum_sensitivity: Sensitivity,
        residency_policy_digest: Digest,
    ) -> Self {
        Self {
            allowed_model,
            maximum_sensitivity,
            residency_policy_digest,
        }
    }

    /// Exact secondary model authorized for compaction summarization.
    #[must_use]
    pub fn allowed_model(&self) -> &ComponentRef {
        &self.allowed_model
    }

    /// Maximum source sensitivity the authorized model may receive.
    #[must_use]
    pub const fn maximum_sensitivity(&self) -> Sensitivity {
        self.maximum_sensitivity
    }

    /// Accepted residency/egress-policy digest bound to this lock.
    #[must_use]
    pub fn residency_policy_digest(&self) -> &Digest {
        &self.residency_policy_digest
    }

    /// True when `model`, `source_sensitivity`, and `residency_policy_digest` match this lock.
    #[must_use]
    pub fn authorizes(
        &self,
        model: &ComponentRef,
        source_sensitivity: Sensitivity,
        residency_policy_digest: &Digest,
    ) -> bool {
        &self.allowed_model == model
            && &self.residency_policy_digest == residency_policy_digest
            && sensitivity_rank(source_sensitivity) <= sensitivity_rank(self.maximum_sensitivity)
    }

    /// True when `child` preserves model/digest and does not raise sensitivity.
    #[must_use]
    pub fn allows_child_attenuation(&self, child: &Self) -> bool {
        child.allowed_model == self.allowed_model
            && child.residency_policy_digest == self.residency_policy_digest
            && sensitivity_rank(child.maximum_sensitivity)
                <= sensitivity_rank(self.maximum_sensitivity)
    }
}

const fn sensitivity_rank(value: Sensitivity) -> u8 {
    match value {
        Sensitivity::Public => 0,
        Sensitivity::Internal => 1,
        Sensitivity::Confidential => 2,
        Sensitivity::Secret => 3,
        Sensitivity::Credential => 4,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction_authorization: Option<CompactionAuthorization>,
}

impl RunSecurityContext {
    /// Construct a security context.
    ///
    /// Compaction authorization defaults to absent (deny). Attach a lock with
    /// [`Self::with_compaction_authorization`] when model-assisted compaction
    /// is explicitly authorized for the accepted run.
    ///
    /// # Arguments
    ///
    /// * `tenant_scope` - Tenant label captured at acceptance.
    /// * `principal` - Authenticated principal for the run.
    /// * `authentication_method` - Authentication-method label (for example `oidc`).
    /// * `assurance_level` - Assurance-level label (for example `high`).
    /// * `authorization_policy_version` - Policy version that produced the decision.
    /// * `authorization_decision_id` - Stable authorization-decision identity.
    /// * `delegated_from` - Optional delegating principal; `None` for a direct run.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] when labels fail validation.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{PrincipalRef, RunSecurityContext};
    ///
    /// let security = RunSecurityContext::try_new(
    ///     "tenant",
    ///     PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal"),
    ///     "oidc",
    ///     "high",
    ///     "policy-v1",
    ///     "decision-v1",
    ///     None,
    /// )
    /// .expect("security");
    /// assert_eq!(security.tenant_scope(), "tenant");
    /// assert!(security.compaction_authorization().is_none());
    /// ```
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
            compaction_authorization: None,
        })
    }

    /// Attach an exact secondary-model compaction authorization lock.
    #[must_use]
    pub fn with_compaction_authorization(mut self, authorization: CompactionAuthorization) -> Self {
        self.compaction_authorization = Some(authorization);
        self
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

    /// Borrow the optional durable compaction authorization lock.
    ///
    /// Absence means model-assisted compaction is denied for this run.
    #[must_use]
    pub fn compaction_authorization(&self) -> Option<&CompactionAuthorization> {
        self.compaction_authorization.as_ref()
    }

    /// True when `child` preserves tenant and attenuates principal/delegation/compaction.
    #[must_use]
    pub fn allows_child_attenuation(&self, child: &Self, policy: PrincipalPropagation) -> bool {
        if child.tenant_scope() != self.tenant_scope() {
            return false;
        }
        if !Self::allows_child_compaction_authorization(
            self.compaction_authorization.as_ref(),
            child.compaction_authorization.as_ref(),
        ) {
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

    /// Child may omit the lock or inherit/attenuate it; it cannot invent or broaden.
    fn allows_child_compaction_authorization(
        parent: Option<&CompactionAuthorization>,
        child: Option<&CompactionAuthorization>,
    ) -> bool {
        match (parent, child) {
            (_, None) => true,
            (None, Some(_)) => false,
            (Some(parent), Some(child)) => parent.allows_child_attenuation(child),
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
            #[serde(default)]
            compaction_authorization: Option<CompactionAuthorization>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let mut security = Self::try_new(
            wire.tenant_scope.into_inner(),
            wire.principal,
            wire.authentication_method.into_inner(),
            wire.assurance_level.into_inner(),
            wire.authorization_policy_version.into_inner(),
            wire.authorization_decision_id.into_inner(),
            wire.delegated_from,
        )
        .map_err(de::Error::custom)?;
        if let Some(authorization) = wire.compaction_authorization {
            security = security.with_compaction_authorization(authorization);
        }
        Ok(security)
    }
}
