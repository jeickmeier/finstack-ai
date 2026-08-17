//! Component, principal, and authorization identity references.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::primitives::ComponentId;

use super::refs_error::{RefsError, validate_label_ref, validated_label, validated_text};

/// Semantic version triple for durable component references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    /// Major version component.
    pub major: u16,
    /// Minor version component.
    pub minor: u16,
    /// Patch version component.
    pub patch: u16,
}

/// Reference to a registered component with optional selected version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ComponentRef {
    id: ComponentId,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<Version>,
}

impl ComponentRef {
    /// Construct a component reference.
    #[must_use]
    pub fn new(id: ComponentId, version: Option<Version>) -> Self {
        Self { id, version }
    }

    /// Borrow the component id.
    #[must_use]
    pub fn id(&self) -> &ComponentId {
        &self.id
    }

    /// Borrow the optional version.
    #[must_use]
    pub fn version(&self) -> Option<Version> {
        self.version
    }
}

impl<'de> Deserialize<'de> for ComponentRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: ComponentId,
            #[serde(default)]
            version: Option<Version>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self::new(wire.id, wire.version))
    }
}

/// Middleware component reference with optional stage name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct MiddlewareRef {
    component: ComponentRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<Arc<str>>,
}

impl MiddlewareRef {
    /// Construct a middleware reference.
    ///
    /// # Arguments
    ///
    /// * `component` - Middleware component identity and optional version.
    /// * `stage` - Optional stage label; `None` leaves the stage unset.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `stage` is empty, oversized, or NUL-bearing.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{ComponentId, ComponentRef, MiddlewareRef};
    ///
    /// let middleware = MiddlewareRef::try_new(
    ///     ComponentRef::new(ComponentId::parse("mw.compact").expect("component"), None),
    ///     Some("before_model"),
    /// )
    /// .expect("middleware");
    /// assert_eq!(middleware.stage(), Some("before_model"));
    /// ```
    pub fn try_new(
        component: ComponentRef,
        stage: Option<impl AsRef<str>>,
    ) -> Result<Self, RefsError> {
        let stage = match stage {
            Some(value) => Some(validated_text(value.as_ref(), "stage")?),
            None => None,
        };
        Ok(Self { component, stage })
    }

    /// Borrow the component reference.
    #[must_use]
    pub fn component(&self) -> &ComponentRef {
        &self.component
    }

    /// Borrow the optional stage.
    #[must_use]
    pub fn stage(&self) -> Option<&str> {
        self.stage.as_deref()
    }
}

impl<'de> Deserialize<'de> for MiddlewareRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            component: ComponentRef,
            #[serde(default)]
            stage: Option<BoundedString<TEXT_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.component, wire.stage.map(BoundedString::into_inner))
            .map_err(de::Error::custom)
    }
}

/// Authenticated principal reference (not a bearer credential).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct PrincipalRef {
    issuer: Arc<str>,
    subject: Arc<str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tenant_scope: Option<Arc<str>>,
}

impl PrincipalRef {
    /// Construct a principal reference.
    ///
    /// # Arguments
    ///
    /// * `issuer` - Identity-issuer label (non-empty, no NUL).
    /// * `subject` - Subject label within that issuer.
    /// * `tenant_scope` - Optional tenant label; `None` leaves the principal
    ///   unscoped.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when issuer/subject(/tenant) fail label rules.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::PrincipalRef;
    ///
    /// let principal = PrincipalRef::try_new("oidc", "user-1", Some("tenant")).expect("principal");
    /// assert_eq!(principal.issuer(), "oidc");
    /// assert_eq!(principal.tenant_scope(), Some("tenant"));
    /// ```
    pub fn try_new(
        issuer: impl AsRef<str>,
        subject: impl AsRef<str>,
        tenant_scope: Option<impl AsRef<str>>,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            issuer: validated_label(issuer.as_ref(), "issuer")?,
            subject: validated_label(subject.as_ref(), "subject")?,
            tenant_scope: match tenant_scope {
                Some(value) => Some(validated_label(value.as_ref(), "tenant_scope")?),
                None => None,
            },
        })
    }

    /// Borrow the issuer.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// Borrow the subject.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Borrow the optional tenant scope.
    #[must_use]
    pub fn tenant_scope(&self) -> Option<&str> {
        self.tenant_scope.as_deref()
    }
}

impl<'de> Deserialize<'de> for PrincipalRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            issuer: BoundedString<LABEL_MAX_BYTES>,
            subject: BoundedString<LABEL_MAX_BYTES>,
            #[serde(default)]
            tenant_scope: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.issuer.into_inner(),
            wire.subject.into_inner(),
            wire.tenant_scope.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}

/// Non-authoritative assignee hint for interactions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AssigneeHint {
    /// Concrete principal.
    Principal(PrincipalRef),
    /// Role name.
    Role(Arc<str>),
    /// Queue name.
    Queue(Arc<str>),
}

impl Serialize for AssigneeHint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire<'a> {
            Principal(&'a PrincipalRef),
            Role(&'a str),
            Queue(&'a str),
        }

        let wire = match self {
            Self::Principal(principal) => Wire::Principal(principal),
            Self::Role(role) => {
                validate_label_ref::<S::Error>(role, "role")?;
                Wire::Role(role)
            }
            Self::Queue(queue) => {
                validate_label_ref::<S::Error>(queue, "queue")?;
                Wire::Queue(queue)
            }
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AssigneeHint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Wire {
            Principal(PrincipalRef),
            Role(BoundedString<LABEL_MAX_BYTES>),
            Queue(BoundedString<LABEL_MAX_BYTES>),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Principal(principal) => Self::Principal(principal),
            Wire::Role(role) => {
                Self::Role(validated_label(&role.into_inner(), "role").map_err(de::Error::custom)?)
            }
            Wire::Queue(queue) => Self::Queue(
                validated_label(&queue.into_inner(), "queue").map_err(de::Error::custom)?,
            ),
        })
    }
}

/// Durable authorization evidence identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct AuthorizationEvidence {
    policy_version: Arc<str>,
    decision_id: Arc<str>,
}

impl AuthorizationEvidence {
    /// Construct authorization evidence.
    ///
    /// # Arguments
    ///
    /// * `policy_version` - Policy version that produced the decision.
    /// * `decision_id` - Stable authorization-decision identity.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when fields fail label rules.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::AuthorizationEvidence;
    ///
    /// let evidence = AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("evidence");
    /// assert_eq!(evidence.policy_version(), "policy-v1");
    /// ```
    pub fn try_new(
        policy_version: impl AsRef<str>,
        decision_id: impl AsRef<str>,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            policy_version: validated_label(policy_version.as_ref(), "policy_version")?,
            decision_id: validated_label(decision_id.as_ref(), "decision_id")?,
        })
    }

    /// Borrow the policy version.
    #[must_use]
    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    /// Borrow the decision id.
    #[must_use]
    pub fn decision_id(&self) -> &str {
        &self.decision_id
    }
}

impl<'de> Deserialize<'de> for AuthorizationEvidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            policy_version: BoundedString<LABEL_MAX_BYTES>,
            decision_id: BoundedString<LABEL_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.policy_version.into_inner(),
            wire.decision_id.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}
