//! Authenticated external-command locators and durable rejection evidence.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::content::{BoundedString, LABEL_MAX_BYTES};
use crate::{
    AuthorizationEvidence, Digest, EffectId, ExternalEffectCompletion, InteractionId,
    InteractionResolution, LaneId, PrincipalRef, RunId, SessionId,
};

/// Complete durable locator expanded from an authenticated opaque callback token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationLocator {
    /// Tenant/scope owning the operation.
    pub tenant_scope: Arc<str>,
    /// Session containing the operation.
    pub session_id: SessionId,
    /// Lane containing the run.
    pub lane_id: LaneId,
    /// Run owning the target effect or interaction.
    pub run_id: RunId,
}

impl OperationLocator {
    /// Construct a bounded operation locator.
    ///
    /// # Arguments
    ///
    /// * `tenant_scope` - Tenant/scope label owning the operation.
    /// * `session_id` - Session containing the operation.
    /// * `lane_id` - Lane containing the run.
    /// * `run_id` - Run owning the target effect or interaction.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalCommandError::InvalidLabel`] when `tenant_scope` is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{LaneId, OperationLocator, RunId, SessionId};
    ///
    /// let locator = OperationLocator::try_new(
    ///     "tenant",
    ///     SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session"),
    ///     LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane"),
    ///     RunId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("run"),
    /// )
    /// .expect("locator");
    /// assert_eq!(locator.tenant_scope.as_ref(), "tenant");
    /// ```
    pub fn try_new(
        tenant_scope: impl AsRef<str>,
        session_id: SessionId,
        lane_id: LaneId,
        run_id: RunId,
    ) -> Result<Self, ExternalCommandError> {
        Ok(Self {
            tenant_scope: validated_label(tenant_scope.as_ref(), "tenant_scope")?,
            session_id,
            lane_id,
            run_id,
        })
    }

    fn validate_principal_scope(
        &self,
        principal: &PrincipalRef,
    ) -> Result<(), ExternalCommandError> {
        if principal
            .tenant_scope()
            .is_some_and(|scope| scope != self.tenant_scope.as_ref())
        {
            return Err(ExternalCommandError::PrincipalScopeMismatch);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for OperationLocator {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            tenant_scope: BoundedString<LABEL_MAX_BYTES>,
            session_id: SessionId,
            lane_id: LaneId,
            run_id: RunId,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.tenant_scope.into_inner(),
            wire.session_id,
            wire.lane_id,
            wire.run_id,
        )
        .map_err(de::Error::custom)
    }
}

/// Authenticated command completing one deferred effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalEffectCompletionCommand {
    /// Durable target locator.
    pub locator: OperationLocator,
    /// Authenticated principal (never a bearer credential).
    pub principal: PrincipalRef,
    /// Exact authorization decision evidence.
    pub authorization: AuthorizationEvidence,
    /// Normalized deferred-effect completion.
    pub completion: ExternalEffectCompletion,
}

impl ExternalEffectCompletionCommand {
    /// Construct an authenticated effect-completion command.
    ///
    /// # Arguments
    ///
    /// * `locator` - Durable operation locator expanded from the callback token.
    /// * `principal` - Authenticated principal submitting the completion.
    /// * `authorization` - Authorization evidence for that principal.
    /// * `completion` - External effect completion payload.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalCommandError::PrincipalScopeMismatch`] when the principal's
    /// explicit tenant scope disagrees with the locator.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     AuthorizationEvidence, EffectId, ErrorCategory, ErrorDescriptor, ExternalEffectCompletion,
    ///     ExternalEffectCompletionCommand, ExternalEffectOutcome, LaneId, OperationLocator,
    ///     PrincipalRef, RunId, SessionId,
    /// };
    ///
    /// # let locator = OperationLocator::try_new(
    /// #     "tenant",
    /// #     SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session"),
    /// #     LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane"),
    /// #     RunId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("run"),
    /// # ).expect("locator");
    /// # let principal = PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal");
    /// # let authorization = AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth");
    /// # let completion = ExternalEffectCompletion::try_new(
    /// #     EffectId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("id"),
    /// #     "completion-1",
    /// #     ExternalEffectOutcome::Failed {
    /// #         error: ErrorDescriptor::new(
    /// #             "provider_failed", "provider failed", ErrorCategory::Model, true,
    /// #         ).expect("error"),
    /// #     },
    /// # ).expect("completion");
    /// let command = ExternalEffectCompletionCommand::try_new(
    ///     locator,
    ///     principal,
    ///     authorization,
    ///     completion,
    /// )
    /// .expect("command");
    /// assert_eq!(command.locator.tenant_scope.as_ref(), "tenant");
    /// ```
    pub fn try_new(
        locator: OperationLocator,
        principal: PrincipalRef,
        authorization: AuthorizationEvidence,
        completion: ExternalEffectCompletion,
    ) -> Result<Self, ExternalCommandError> {
        locator.validate_principal_scope(&principal)?;
        Ok(Self {
            locator,
            principal,
            authorization,
            completion,
        })
    }
}

impl<'de> Deserialize<'de> for ExternalEffectCompletionCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            locator: OperationLocator,
            principal: PrincipalRef,
            authorization: AuthorizationEvidence,
            completion: ExternalEffectCompletion,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.locator,
            wire.principal,
            wire.authorization,
            wire.completion,
        )
        .map_err(de::Error::custom)
    }
}

/// Authenticated command resolving one durable interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionResolutionCommand {
    /// Durable target locator.
    pub locator: OperationLocator,
    /// Normalized interaction resolution, including principal and authorization.
    pub resolution: InteractionResolution,
}

impl InteractionResolutionCommand {
    /// Construct an authenticated interaction-resolution command.
    ///
    /// # Arguments
    ///
    /// * `locator` - Durable operation locator expanded from the callback token.
    /// * `resolution` - Interaction resolution, including its resolving principal.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalCommandError::PrincipalScopeMismatch`] when the resolving
    /// principal's explicit tenant scope disagrees with the locator.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     AuthorizationEvidence, InteractionId, InteractionResolution, InteractionResolutionCommand,
    ///     LaneId, OperationLocator, PrincipalRef, RawJson, RunId, SessionId,
    /// };
    ///
    /// # let locator = OperationLocator::try_new(
    /// #     "tenant",
    /// #     SessionId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("session"),
    /// #     LaneId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("lane"),
    /// #     RunId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("run"),
    /// # ).expect("locator");
    /// # let principal = PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal");
    /// # let authorization = AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth");
    /// # let resolution = InteractionResolution::try_new(
    /// #     InteractionId::parse("01234567-89ab-7cde-89ab-0123456789ae").expect("id"),
    /// #     "resolution-1",
    /// #     principal,
    /// #     authorization,
    /// #     RawJson::parse(r#"{"approved":true}"#).expect("json"),
    /// #     None::<&str>,
    /// # ).expect("resolution");
    /// let command = InteractionResolutionCommand::try_new(locator, resolution).expect("command");
    /// assert_eq!(command.resolution.resolution_id(), "resolution-1");
    /// ```
    pub fn try_new(
        locator: OperationLocator,
        resolution: InteractionResolution,
    ) -> Result<Self, ExternalCommandError> {
        locator.validate_principal_scope(resolution.principal())?;
        Ok(Self {
            locator,
            resolution,
        })
    }
}

impl<'de> Deserialize<'de> for InteractionResolutionCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            locator: OperationLocator,
            resolution: InteractionResolution,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.locator, wire.resolution).map_err(de::Error::custom)
    }
}

/// External command family recorded by rejection evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCommandKind {
    /// Deferred effect completion.
    EffectCompletion,
    /// Interaction resolution.
    InteractionResolution,
}

/// Target identity recorded without carrying submitted content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCommandTarget {
    /// Effect target.
    Effect(EffectId),
    /// Interaction target.
    Interaction(InteractionId),
}

/// Durable evidence for a known, authorized external command rejected by semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalCommandRejected {
    /// External command family.
    pub command_kind: ExternalCommandKind,
    /// Bounded idempotency identity supplied by the authenticated command.
    pub command_id: Arc<str>,
    /// Exact target identity.
    pub target: ExternalCommandTarget,
    /// Authenticated principal.
    pub principal: PrincipalRef,
    /// Exact authorization evidence.
    pub authorization: AuthorizationEvidence,
    /// Stable bounded rejection reason.
    pub reason_code: Arc<str>,
    /// Digest of the normalized submitted command.
    pub submitted_digest: Digest,
    /// Prior accepted digest when disclosure policy permits it.
    pub accepted_digest: Option<Digest>,
}

impl ExternalCommandRejected {
    /// Construct bounded, kind/target-consistent durable rejection evidence.
    ///
    /// # Arguments
    ///
    /// * `command_kind` - Rejected command family.
    /// * `command_id` - Caller-assigned command identity label.
    /// * `target` - Target family that must agree with `command_kind`.
    /// * `principal` - Principal that submitted the rejected command.
    /// * `authorization` - Authorization evidence for that principal.
    /// * `reason_code` - Stable rejection-reason label.
    /// * `submitted_digest` - Digest of the submitted command payload.
    /// * `accepted_digest` - Optional digest of an already-accepted conflicting
    ///   command; `None` when none exists.
    ///
    /// # Errors
    ///
    /// Returns [`ExternalCommandError`] when an identity/reason is invalid or the
    /// command kind disagrees with its target family.
    ///
    /// # Examples
    ///
    /// ```
    /// use finstack_ai_kernel::{
    ///     AuthorizationEvidence, Digest, EffectId, ExternalCommandKind, ExternalCommandRejected,
    ///     ExternalCommandTarget, PrincipalRef,
    /// };
    ///
    /// # let principal = PrincipalRef::try_new("issuer", "subject", Some("tenant")).expect("principal");
    /// # let authorization = AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth");
    /// let rejected = ExternalCommandRejected::try_new(
    ///     ExternalCommandKind::EffectCompletion,
    ///     "completion-1",
    ///     ExternalCommandTarget::Effect(
    ///         EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id"),
    ///     ),
    ///     principal,
    ///     authorization,
    ///     "conflicting_completion",
    ///     Digest::raw_json(b"{}"),
    ///     None,
    /// )
    /// .expect("rejected");
    /// assert_eq!(rejected.reason_code.as_ref(), "conflicting_completion");
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        command_kind: ExternalCommandKind,
        command_id: impl AsRef<str>,
        target: ExternalCommandTarget,
        principal: PrincipalRef,
        authorization: AuthorizationEvidence,
        reason_code: impl AsRef<str>,
        submitted_digest: Digest,
        accepted_digest: Option<Digest>,
    ) -> Result<Self, ExternalCommandError> {
        let target_matches = matches!(
            (command_kind, target),
            (
                ExternalCommandKind::EffectCompletion,
                ExternalCommandTarget::Effect(_)
            ) | (
                ExternalCommandKind::InteractionResolution,
                ExternalCommandTarget::Interaction(_)
            )
        );
        if !target_matches {
            return Err(ExternalCommandError::TargetKindMismatch);
        }
        Ok(Self {
            command_kind,
            command_id: validated_label(command_id.as_ref(), "command_id")?,
            target,
            principal,
            authorization,
            reason_code: validated_label(reason_code.as_ref(), "reason_code")?,
            submitted_digest,
            accepted_digest,
        })
    }
}

impl<'de> Deserialize<'de> for ExternalCommandRejected {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            command_kind: ExternalCommandKind,
            command_id: BoundedString<LABEL_MAX_BYTES>,
            target: ExternalCommandTarget,
            principal: PrincipalRef,
            authorization: AuthorizationEvidence,
            reason_code: BoundedString<LABEL_MAX_BYTES>,
            submitted_digest: Digest,
            #[serde(default)]
            accepted_digest: Option<Digest>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.command_kind,
            wire.command_id.into_inner(),
            wire.target,
            wire.principal,
            wire.authorization,
            wire.reason_code.into_inner(),
            wire.submitted_digest,
            wire.accepted_digest,
        )
        .map_err(de::Error::custom)
    }
}

/// Normalized kernel input recording a router-validated external rejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordExternalCommandRejected {
    /// Full authenticated locator validated against the accepted run.
    pub locator: OperationLocator,
    /// Durable redacted rejection body.
    pub rejection: ExternalCommandRejected,
}

/// External-command validation errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExternalCommandError {
    /// A bounded stable label was empty, oversized, or NUL-bearing.
    #[error("invalid {field} label")]
    InvalidLabel {
        /// Invalid field.
        field: &'static str,
    },
    /// Principal and locator tenant scopes disagree.
    #[error("principal tenant scope does not match locator")]
    PrincipalScopeMismatch,
    /// Command family and target identity kind disagree.
    #[error("external command kind does not match target")]
    TargetKindMismatch,
}

impl ExternalCommandError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLabel { .. } => "invalid_label",
            Self::PrincipalScopeMismatch => "principal_scope_mismatch",
            Self::TargetKindMismatch => "target_kind_mismatch",
        }
    }
}

fn validated_label(value: &str, field: &'static str) -> Result<Arc<str>, ExternalCommandError> {
    crate::primitives::label::validated_label(value, field, |field| {
        ExternalCommandError::InvalidLabel { field }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCategory, ErrorDescriptor, ExternalEffectOutcome, RawJson};

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn locator() -> OperationLocator {
        OperationLocator::try_new(
            "tenant-a",
            id::<crate::SessionTag>(1),
            id::<crate::LaneTag>(2),
            id::<crate::RunTag>(3),
        )
        .expect("locator")
    }

    #[test]
    fn external_completion_command_is_strict_and_scope_bound() {
        let principal =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
        let authorization =
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
        let completion = ExternalEffectCompletion::try_new(
            id::<crate::EffectTag>(4),
            "completion-1",
            ExternalEffectOutcome::Failed {
                error: ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    ErrorCategory::Model,
                    true,
                )
                .expect("error"),
            },
        )
        .expect("completion");
        let command = ExternalEffectCompletionCommand::try_new(
            locator(),
            principal,
            authorization,
            completion,
        )
        .expect("command");
        let mut json = serde_json::to_value(&command).expect("serialize");
        assert_eq!(
            serde_json::from_value::<ExternalEffectCompletionCommand>(json.clone())
                .expect("round trip"),
            command
        );
        json.as_object_mut()
            .expect("object")
            .insert("token".into(), serde_json::Value::String("secret".into()));
        assert!(serde_json::from_value::<ExternalEffectCompletionCommand>(json).is_err());

        let mismatched =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-b")).expect("principal");
        assert_eq!(
            locator().validate_principal_scope(&mismatched),
            Err(ExternalCommandError::PrincipalScopeMismatch)
        );
    }

    #[test]
    fn rejection_rejects_invalid_labels_and_target_family() {
        let principal =
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
        let authorization =
            AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
        let digest = Digest::raw_json(RawJson::parse("{}").expect("json").as_bytes());
        assert!(matches!(
            ExternalCommandRejected::try_new(
                ExternalCommandKind::EffectCompletion,
                "",
                ExternalCommandTarget::Effect(id::<crate::EffectTag>(4)),
                principal.clone(),
                authorization.clone(),
                "conflicting_completion",
                digest,
                None,
            ),
            Err(ExternalCommandError::InvalidLabel {
                field: "command_id"
            })
        ));
        assert_eq!(
            ExternalCommandRejected::try_new(
                ExternalCommandKind::EffectCompletion,
                "completion-1",
                ExternalCommandTarget::Interaction(id::<crate::InteractionTag>(4)),
                principal,
                authorization,
                "conflicting_completion",
                digest,
                None,
            ),
            Err(ExternalCommandError::TargetKindMismatch)
        );
    }
}
