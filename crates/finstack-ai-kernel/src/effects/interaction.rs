//! Interaction request, resolution, expiry, and cancellation envelopes.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};

use crate::content::{
    BoundedString, ContentBlock, ContentItems, LABEL_MAX_BYTES, validate_content_items,
};
use crate::digest::Digest;
use crate::ids::{EffectId, InteractionId};
use crate::raw_json::{Metadata, RawJson};
use crate::refs::{
    AssigneeHint, AuthorizationEvidence, ComponentRef, PrincipalRef, Version, validated_label,
};
use crate::time::Timestamp;

use super::EffectError;

/// Interaction kind (approval is a profile, not a separate record family).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionKind {
    /// Approval profile.
    Approval,
    /// Choice.
    Choice,
    /// Form.
    Form,
    /// Free text.
    FreeText,
    /// Review.
    Review,
    /// Correction.
    Correction,
    /// Custom.
    Custom {
        /// Custom name.
        name: Arc<str>,
    },
}

impl<'de> Deserialize<'de> for InteractionKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
        enum Wire {
            Approval,
            Choice,
            Form,
            FreeText,
            Review,
            Correction,
            Custom {
                name: BoundedString<LABEL_MAX_BYTES>,
            },
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::Approval => Self::Approval,
            Wire::Choice => Self::Choice,
            Wire::Form => Self::Form,
            Wire::FreeText => Self::FreeText,
            Wire::Review => Self::Review,
            Wire::Correction => Self::Correction,
            Wire::Custom { name } => Self::Custom {
                name: Arc::from(name.into_inner()),
            },
        })
    }
}

/// Interaction request payload (`InteractionRequested` record body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionRequest {
    request_version: u16,
    interaction_id: InteractionId,
    effect_id: EffectId,
    kind: InteractionKind,
    prompt: Arc<[ContentBlock]>,
    prompt_digest: Digest,
    response_schema: RawJson,
    response_schema_digest: Digest,
    policy_component: ComponentRef,
    policy_version: Version,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignee_hint: Option<AssigneeHint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<Timestamp>,
    delegatable: bool,
    metadata: Metadata,
}

impl InteractionRequest {
    /// Construct an interaction request and compute digests.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] on content/digest failures.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        request_version: u16,
        interaction_id: InteractionId,
        effect_id: EffectId,
        kind: InteractionKind,
        prompt: Vec<ContentBlock>,
        response_schema: RawJson,
        policy_component: ComponentRef,
        policy_version: Version,
        assignee_hint: Option<AssigneeHint>,
        expires_at: Option<Timestamp>,
        delegatable: bool,
        metadata: Metadata,
    ) -> Result<Self, EffectError> {
        validate_content_items(&prompt)?;
        if let InteractionKind::Custom { name } = &kind {
            validated_label(name, "interaction_kind")?;
        }
        let prompt_canonical =
            serde_json_canonicalizer::to_vec(&prompt).map_err(|error| EffectError::Serialize {
                detail: error.to_string(),
            })?;
        Ok(Self {
            request_version,
            interaction_id,
            effect_id,
            kind,
            prompt: prompt.into(),
            prompt_digest: Digest::effect_input(&prompt_canonical),
            response_schema_digest: Digest::effect_input(response_schema.as_str().as_bytes()),
            response_schema,
            policy_component,
            policy_version,
            assignee_hint,
            expires_at,
            delegatable,
            metadata,
        })
    }

    /// Request contract version.
    #[must_use]
    pub fn request_version(&self) -> u16 {
        self.request_version
    }

    /// Interaction id.
    #[must_use]
    pub fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    /// Effect id.
    #[must_use]
    pub fn effect_id(&self) -> EffectId {
        self.effect_id
    }

    /// Kind.
    #[must_use]
    pub fn kind(&self) -> &InteractionKind {
        &self.kind
    }

    /// Prompt content.
    #[must_use]
    pub fn prompt(&self) -> &[ContentBlock] {
        &self.prompt
    }

    /// Prompt digest.
    #[must_use]
    pub fn prompt_digest(&self) -> Digest {
        self.prompt_digest
    }

    /// Response schema.
    #[must_use]
    pub fn response_schema(&self) -> &RawJson {
        &self.response_schema
    }

    /// Response-schema digest.
    #[must_use]
    pub fn response_schema_digest(&self) -> Digest {
        self.response_schema_digest
    }

    /// Policy component.
    #[must_use]
    pub fn policy_component(&self) -> &ComponentRef {
        &self.policy_component
    }

    /// Policy version.
    #[must_use]
    pub fn policy_version(&self) -> Version {
        self.policy_version
    }

    /// Non-authoritative assignee hint.
    #[must_use]
    pub fn assignee_hint(&self) -> Option<&AssigneeHint> {
        self.assignee_hint.as_ref()
    }

    /// Expiration time.
    #[must_use]
    pub fn expires_at(&self) -> Option<Timestamp> {
        self.expires_at
    }

    /// Whether the interaction may be delegated.
    #[must_use]
    pub fn delegatable(&self) -> bool {
        self.delegatable
    }

    /// Non-authoritative metadata.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Request digest for pairing with `EffectInput::Interaction`.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when serialization fails.
    pub fn request_digest(&self) -> Result<Digest, EffectError> {
        let canonical =
            serde_json_canonicalizer::to_vec(self).map_err(|error| EffectError::Serialize {
                detail: error.to_string(),
            })?;
        Ok(Digest::effect_input(&canonical))
    }
}

impl<'de> Deserialize<'de> for InteractionRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            request_version: u16,
            interaction_id: InteractionId,
            effect_id: EffectId,
            kind: InteractionKind,
            prompt: ContentItems,
            prompt_digest: Digest,
            response_schema: RawJson,
            response_schema_digest: Digest,
            policy_component: ComponentRef,
            policy_version: Version,
            #[serde(default)]
            assignee_hint: Option<AssigneeHint>,
            #[serde(default)]
            expires_at: Option<Timestamp>,
            delegatable: bool,
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        let constructed = Self::try_new(
            wire.request_version,
            wire.interaction_id,
            wire.effect_id,
            wire.kind,
            wire.prompt.into_inner(),
            wire.response_schema,
            wire.policy_component,
            wire.policy_version,
            wire.assignee_hint,
            wire.expires_at,
            wire.delegatable,
            wire.metadata,
        )
        .map_err(de::Error::custom)?;
        if constructed.prompt_digest != wire.prompt_digest
            || constructed.response_schema_digest != wire.response_schema_digest
        {
            return Err(de::Error::custom("interaction digest mismatch"));
        }
        Ok(constructed)
    }
}

/// Interaction resolution payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionResolution {
    interaction_id: InteractionId,
    resolution_id: Arc<str>,
    principal: PrincipalRef,
    authorization: AuthorizationEvidence,
    response: RawJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    comment: Option<Arc<str>>,
}

impl InteractionResolution {
    /// Construct a resolution.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError`] when labels fail validation.
    pub fn try_new(
        interaction_id: InteractionId,
        resolution_id: impl AsRef<str>,
        principal: PrincipalRef,
        authorization: AuthorizationEvidence,
        response: RawJson,
        comment: Option<impl AsRef<str>>,
    ) -> Result<Self, EffectError> {
        Ok(Self {
            interaction_id,
            resolution_id: validated_label(resolution_id.as_ref(), "resolution_id")?,
            principal,
            authorization,
            response,
            comment: match comment {
                Some(value) => Some(validated_label(value.as_ref(), "comment")?),
                None => None,
            },
        })
    }

    /// Interaction id.
    #[must_use]
    pub fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    /// Idempotent resolution id.
    #[must_use]
    pub fn resolution_id(&self) -> &str {
        &self.resolution_id
    }

    /// Resolving principal.
    #[must_use]
    pub fn principal(&self) -> &PrincipalRef {
        &self.principal
    }

    /// Authorization evidence for the resolution.
    #[must_use]
    pub fn authorization(&self) -> &AuthorizationEvidence {
        &self.authorization
    }

    /// Schema-validated response.
    #[must_use]
    pub fn response(&self) -> &RawJson {
        &self.response
    }

    /// Optional resolver comment.
    #[must_use]
    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }
}

impl<'de> Deserialize<'de> for InteractionResolution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            interaction_id: InteractionId,
            resolution_id: BoundedString<LABEL_MAX_BYTES>,
            principal: PrincipalRef,
            authorization: AuthorizationEvidence,
            response: RawJson,
            #[serde(default)]
            comment: Option<BoundedString<LABEL_MAX_BYTES>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.interaction_id,
            wire.resolution_id.into_inner(),
            wire.principal,
            wire.authorization,
            wire.response,
            wire.comment.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}

/// Interaction expired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionExpired {
    /// Interaction id.
    pub interaction_id: InteractionId,
    /// Expiry timestamp.
    pub expired_at: Timestamp,
}

/// Interaction cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionCancelled {
    /// Interaction id.
    interaction_id: InteractionId,
    /// Optional principal for principal-initiated cancellation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    principal: Option<PrincipalRef>,
    /// Optional authorization paired with principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authorization: Option<AuthorizationEvidence>,
    /// Optional reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<Arc<str>>,
}

impl InteractionCancelled {
    /// Construct a cancellation with principal/authorization pairing rules.
    ///
    /// # Errors
    ///
    /// Returns [`EffectError::InvalidCancellationPair`] when exactly one of
    /// principal/authorization is present.
    pub fn try_new(
        interaction_id: InteractionId,
        principal: Option<PrincipalRef>,
        authorization: Option<AuthorizationEvidence>,
        reason: Option<impl AsRef<str>>,
    ) -> Result<Self, EffectError> {
        match (&principal, &authorization) {
            (Some(_), Some(_)) | (None, None) => {}
            _ => return Err(EffectError::InvalidCancellationPair),
        }
        Ok(Self {
            interaction_id,
            principal,
            authorization,
            reason: match reason {
                Some(value) => Some(validated_label(value.as_ref(), "reason")?),
                None => None,
            },
        })
    }

    /// Interaction id.
    #[must_use]
    pub fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    /// Principal for a principal-initiated cancellation.
    #[must_use]
    pub fn principal(&self) -> Option<&PrincipalRef> {
        self.principal.as_ref()
    }

    /// Authorization evidence paired with [`Self::principal`].
    #[must_use]
    pub fn authorization(&self) -> Option<&AuthorizationEvidence> {
        self.authorization.as_ref()
    }

    /// Optional cancellation reason.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

impl<'de> Deserialize<'de> for InteractionCancelled {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            interaction_id: InteractionId,
            #[serde(default)]
            principal: Option<PrincipalRef>,
            #[serde(default)]
            authorization: Option<AuthorizationEvidence>,
            #[serde(default)]
            reason: Option<BoundedString<LABEL_MAX_BYTES>>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.interaction_id,
            wire.principal,
            wire.authorization,
            wire.reason.map(BoundedString::into_inner),
        )
        .map_err(de::Error::custom)
    }
}
