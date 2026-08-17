//! Run security context.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::refs::{PrincipalRef, validated_label};

use super::error::RunError;
use super::propagation::PrincipalPropagation;

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
