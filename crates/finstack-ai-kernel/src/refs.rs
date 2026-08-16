//! Shared references, diagnostics, sensitivity, and usage (TDD §5.3 / §7.2).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bounds::{BoundedMap, BoundedVec, SEMANTIC_ARRAY_MAX_ITEMS};
use crate::content::{BlobRef, BoundedString, LABEL_MAX_BYTES, TEXT_MAX_BYTES};
use crate::digest::Digest;
use crate::ids::{
    AppendBatchId, ArtifactId, CancellationRequestId, ComponentId, EffectId, EventId,
    InteractionId, LimitKey, MessageId, ModelRequestId, RecordId, ToolBatchId, ToolCallId, TurnId,
};
use crate::raw_json::{Metadata, RawJson};

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
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `stage` is empty, oversized, or NUL-bearing.
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
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when issuer/subject(/tenant) fail label rules.
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
            Wire::Role(role) => Self::Role(Arc::from(role.into_inner())),
            Wire::Queue(queue) => Self::Queue(Arc::from(queue.into_inner())),
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
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when fields fail label rules.
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

/// Non-secret external handle for deferred effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalHandleRef {
    provider: ComponentId,
    handle: Arc<str>,
    reconciliation_metadata: RawJson,
}

impl ExternalHandleRef {
    /// Construct an external handle reference.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `handle` fails label rules.
    pub fn try_new(
        provider: ComponentId,
        handle: impl AsRef<str>,
        reconciliation_metadata: RawJson,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            provider,
            handle: validated_label(handle.as_ref(), "handle")?,
            reconciliation_metadata,
        })
    }

    /// Borrow the provider component id.
    #[must_use]
    pub fn provider(&self) -> &ComponentId {
        &self.provider
    }

    /// Borrow the opaque handle.
    #[must_use]
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Borrow reconciliation metadata.
    #[must_use]
    pub fn reconciliation_metadata(&self) -> &RawJson {
        &self.reconciliation_metadata
    }
}

impl<'de> Deserialize<'de> for ExternalHandleRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            provider: ComponentId,
            handle: BoundedString<LABEL_MAX_BYTES>,
            reconciliation_metadata: RawJson,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.provider,
            wire.handle.into_inner(),
            wire.reconciliation_metadata,
        )
        .map_err(de::Error::custom)
    }
}

/// Digest-bearing artifact reference for replay-required content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactRef {
    id: ArtifactId,
    kind: Arc<str>,
    blob: BlobRef,
    content_digest: Digest,
    scope_digest: Digest,
    metadata: Metadata,
}

impl ArtifactRef {
    /// Construct an artifact reference.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `kind` fails label rules.
    pub fn try_new(
        id: ArtifactId,
        kind: impl AsRef<str>,
        blob: BlobRef,
        content_digest: Digest,
        scope_digest: Digest,
        metadata: Metadata,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            id,
            kind: validated_label(kind.as_ref(), "kind")?,
            blob,
            content_digest,
            scope_digest,
            metadata,
        })
    }

    /// Borrow the artifact id.
    #[must_use]
    pub fn id(&self) -> ArtifactId {
        self.id
    }

    /// Borrow the kind label.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Borrow the blob reference.
    #[must_use]
    pub fn blob(&self) -> &BlobRef {
        &self.blob
    }

    /// Borrow the content digest.
    #[must_use]
    pub fn content_digest(&self) -> Digest {
        self.content_digest
    }

    /// Borrow the scope digest.
    #[must_use]
    pub fn scope_digest(&self) -> Digest {
        self.scope_digest
    }

    /// Borrow metadata.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

impl<'de> Deserialize<'de> for ArtifactRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: ArtifactId,
            kind: BoundedString<LABEL_MAX_BYTES>,
            blob: BlobRef,
            content_digest: Digest,
            scope_digest: Digest,
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.id,
            wire.kind.into_inner(),
            wire.blob,
            wire.content_digest,
            wire.scope_digest,
            wire.metadata,
        )
        .map_err(de::Error::custom)
    }
}

/// Sensitivity classification for events and scoped artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Public.
    Public,
    /// Internal.
    Internal,
    /// Confidential.
    Confidential,
    /// Secret.
    Secret,
    /// Credential material class (never place secrets in records/events).
    Credential,
}

/// Decision-local diagnostic (never a [`crate::events::RunEvent`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    code: Arc<str>,
    message: Arc<str>,
    severity: DiagnosticSeverity,
    metadata: Metadata,
}

impl Diagnostic {
    /// Construct a diagnostic.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when `code` fails label rules, or when
    /// `message` is empty/oversized/NUL-bearing.
    pub fn try_new(
        code: impl AsRef<str>,
        message: impl AsRef<str>,
        severity: DiagnosticSeverity,
        metadata: Metadata,
    ) -> Result<Self, RefsError> {
        let message = message.as_ref();
        if message.is_empty()
            || message.len() > crate::content::TEXT_MAX_BYTES
            || message.as_bytes().contains(&0)
        {
            return Err(RefsError::InvalidLabel { field: "message" });
        }
        Ok(Self {
            code: validated_label(code.as_ref(), "code")?,
            message: Arc::<str>::from(message),
            severity,
            metadata,
        })
    }

    /// Borrow the code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Borrow the message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Borrow severity.
    #[must_use]
    pub fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }

    /// Borrow metadata.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

impl<'de> Deserialize<'de> for Diagnostic {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            code: BoundedString<LABEL_MAX_BYTES>,
            message: BoundedString<TEXT_MAX_BYTES>,
            severity: DiagnosticSeverity,
            metadata: Metadata,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.code.into_inner(),
            wire.message.into_inner(),
            wire.severity,
            wire.metadata,
        )
        .map_err(de::Error::custom)
    }
}

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Debug.
    Debug,
    /// Info.
    Info,
    /// Warning.
    Warning,
    /// Error.
    Error,
}

/// Recorded cost amount (integer millionths; JSON decimal string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct CostAmount {
    unit: Arc<str>,
    #[serde(serialize_with = "serialize_micros")]
    micros: u64,
    pricing_policy_version: Arc<str>,
}

impl CostAmount {
    /// Construct a cost amount.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::InvalidLabel`] when unit/policy labels fail rules.
    pub fn try_new(
        unit: impl AsRef<str>,
        micros: u64,
        pricing_policy_version: impl AsRef<str>,
    ) -> Result<Self, RefsError> {
        Ok(Self {
            unit: validated_label(unit.as_ref(), "unit")?,
            micros,
            pricing_policy_version: validated_label(
                pricing_policy_version.as_ref(),
                "pricing_policy_version",
            )?,
        })
    }

    /// Borrow the unit.
    #[must_use]
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// Return micros.
    #[must_use]
    pub fn micros(&self) -> u64 {
        self.micros
    }

    /// Borrow the pricing policy version.
    #[must_use]
    pub fn pricing_policy_version(&self) -> &str {
        &self.pricing_policy_version
    }
}

impl<'de> Deserialize<'de> for CostAmount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            unit: BoundedString<LABEL_MAX_BYTES>,
            #[serde(deserialize_with = "deserialize_micros")]
            micros: u64,
            pricing_policy_version: BoundedString<LABEL_MAX_BYTES>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.unit.into_inner(),
            wire.micros,
            wire.pricing_policy_version.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}

/// Normalized token/cost usage for effect completion and budget charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Usage {
    /// Optional input token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    /// Optional output token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    /// Optional total token count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    total_tokens: Option<u64>,
    /// Optional recorded cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cost: Option<CostAmount>,
    /// Namespaced extension counters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    extension_counters: BTreeMap<LimitKey, u64>,
}

impl Usage {
    /// V1 maximum registered extension counters per resolved agent.
    pub const MAX_EXTENSION_COUNTERS: usize = 32;

    /// Empty usage.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cost: None,
            extension_counters: BTreeMap::new(),
        }
    }

    /// Construct validated normalized usage.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::TooManyEntries`] when extension counters exceed the v1 ceiling.
    pub fn try_new(
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        total_tokens: Option<u64>,
        cost: Option<CostAmount>,
        extension_counters: BTreeMap<LimitKey, u64>,
    ) -> Result<Self, RefsError> {
        if extension_counters.len() > Self::MAX_EXTENSION_COUNTERS {
            return Err(RefsError::TooManyEntries {
                field: "usage.extension_counters",
                len: extension_counters.len(),
                max: Self::MAX_EXTENSION_COUNTERS,
            });
        }
        Ok(Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cost,
            extension_counters,
        })
    }

    /// Validate collection ceilings.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::TooManyEntries`] when extension counters exceed the v1 ceiling.
    pub fn validate(&self) -> Result<(), RefsError> {
        if self.extension_counters.len() > Self::MAX_EXTENSION_COUNTERS {
            return Err(RefsError::TooManyEntries {
                field: "usage.extension_counters",
                len: self.extension_counters.len(),
                max: Self::MAX_EXTENSION_COUNTERS,
            });
        }
        Ok(())
    }

    /// Input token count.
    #[must_use]
    pub fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    /// Output token count.
    #[must_use]
    pub fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }

    /// Total token count.
    #[must_use]
    pub fn total_tokens(&self) -> Option<u64> {
        self.total_tokens
    }

    /// Recorded cost.
    #[must_use]
    pub fn cost(&self) -> Option<&CostAmount> {
        self.cost.as_ref()
    }

    /// Extension counters.
    #[must_use]
    pub fn extension_counters(&self) -> &BTreeMap<LimitKey, u64> {
        &self.extension_counters
    }

    /// Canonical JSON bytes for digesting usage under effect domains.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::Serialize`] when serialization fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RefsError> {
        serde_json_canonicalizer::to_vec(self).map_err(|error| RefsError::Serialize {
            detail: error.to_string(),
        })
    }
}

impl<'de> Deserialize<'de> for Usage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(default)]
            input_tokens: Option<u64>,
            #[serde(default)]
            output_tokens: Option<u64>,
            #[serde(default)]
            total_tokens: Option<u64>,
            #[serde(default)]
            cost: Option<CostAmount>,
            #[serde(default)]
            extension_counters: BoundedMap<LimitKey, u64, { Usage::MAX_EXTENSION_COUNTERS }>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.input_tokens,
            wire.output_tokens,
            wire.total_tokens,
            wire.cost,
            wire.extension_counters.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}

/// Runtime-owned preallocated `UUIDv7` bags for a transition.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[allow(clippy::struct_field_names)] // Frozen contract fields intentionally end in `_ids`.
pub struct AllocatedIds {
    /// Record ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    record_ids: Vec<RecordId>,
    /// Event ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    event_ids: Vec<EventId>,
    /// Effect ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    effect_ids: Vec<EffectId>,
    /// Interaction ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    interaction_ids: Vec<InteractionId>,
    /// Message ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    message_ids: Vec<MessageId>,
    /// Turn ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    turn_ids: Vec<TurnId>,
    /// Model request ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    model_request_ids: Vec<ModelRequestId>,
    /// Tool batch ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_batch_ids: Vec<ToolBatchId>,
    /// Tool call ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_call_ids: Vec<ToolCallId>,
    /// Append batch ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    append_batch_ids: Vec<AppendBatchId>,
    /// Cancellation request ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cancellation_request_ids: Vec<CancellationRequestId>,
}

impl AllocatedIds {
    /// Construct bounded runtime-owned ID bags.
    ///
    /// # Errors
    ///
    /// Returns [`RefsError::TooManyItems`] when any bag exceeds the v1 array ceiling.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        record_ids: Vec<RecordId>,
        event_ids: Vec<EventId>,
        effect_ids: Vec<EffectId>,
        interaction_ids: Vec<InteractionId>,
        message_ids: Vec<MessageId>,
        turn_ids: Vec<TurnId>,
        model_request_ids: Vec<ModelRequestId>,
        tool_batch_ids: Vec<ToolBatchId>,
        tool_call_ids: Vec<ToolCallId>,
        append_batch_ids: Vec<AppendBatchId>,
        cancellation_request_ids: Vec<CancellationRequestId>,
    ) -> Result<Self, RefsError> {
        for (field, len) in [
            ("record_ids", record_ids.len()),
            ("event_ids", event_ids.len()),
            ("effect_ids", effect_ids.len()),
            ("interaction_ids", interaction_ids.len()),
            ("message_ids", message_ids.len()),
            ("turn_ids", turn_ids.len()),
            ("model_request_ids", model_request_ids.len()),
            ("tool_batch_ids", tool_batch_ids.len()),
            ("tool_call_ids", tool_call_ids.len()),
            ("append_batch_ids", append_batch_ids.len()),
            ("cancellation_request_ids", cancellation_request_ids.len()),
        ] {
            if len > SEMANTIC_ARRAY_MAX_ITEMS {
                return Err(RefsError::TooManyItems {
                    field,
                    len,
                    max: SEMANTIC_ARRAY_MAX_ITEMS,
                });
            }
        }
        Ok(Self {
            record_ids,
            event_ids,
            effect_ids,
            interaction_ids,
            message_ids,
            turn_ids,
            model_request_ids,
            tool_batch_ids,
            tool_call_ids,
            append_batch_ids,
            cancellation_request_ids,
        })
    }

    /// Record ids.
    #[must_use]
    pub fn record_ids(&self) -> &[RecordId] {
        &self.record_ids
    }

    /// Event ids.
    #[must_use]
    pub fn event_ids(&self) -> &[EventId] {
        &self.event_ids
    }

    /// Effect ids.
    #[must_use]
    pub fn effect_ids(&self) -> &[EffectId] {
        &self.effect_ids
    }

    /// Interaction ids.
    #[must_use]
    pub fn interaction_ids(&self) -> &[InteractionId] {
        &self.interaction_ids
    }

    /// Message ids.
    #[must_use]
    pub fn message_ids(&self) -> &[MessageId] {
        &self.message_ids
    }

    /// Turn ids.
    #[must_use]
    pub fn turn_ids(&self) -> &[TurnId] {
        &self.turn_ids
    }

    /// Model request ids.
    #[must_use]
    pub fn model_request_ids(&self) -> &[ModelRequestId] {
        &self.model_request_ids
    }

    /// Tool batch ids.
    #[must_use]
    pub fn tool_batch_ids(&self) -> &[ToolBatchId] {
        &self.tool_batch_ids
    }

    /// Tool call ids.
    #[must_use]
    pub fn tool_call_ids(&self) -> &[ToolCallId] {
        &self.tool_call_ids
    }

    /// Append batch ids.
    #[must_use]
    pub fn append_batch_ids(&self) -> &[AppendBatchId] {
        &self.append_batch_ids
    }

    /// Cancellation request ids.
    #[must_use]
    pub fn cancellation_request_ids(&self) -> &[CancellationRequestId] {
        &self.cancellation_request_ids
    }
}

impl<'de> Deserialize<'de> for AllocatedIds {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(clippy::struct_field_names)] // Mirrors the frozen `AllocatedIds` contract.
        struct Wire {
            #[serde(default)]
            record_ids: BoundedVec<RecordId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            event_ids: BoundedVec<EventId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            effect_ids: BoundedVec<EffectId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            interaction_ids: BoundedVec<InteractionId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            message_ids: BoundedVec<MessageId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            turn_ids: BoundedVec<TurnId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            model_request_ids: BoundedVec<ModelRequestId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            tool_batch_ids: BoundedVec<ToolBatchId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            tool_call_ids: BoundedVec<ToolCallId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            append_batch_ids: BoundedVec<AppendBatchId, SEMANTIC_ARRAY_MAX_ITEMS>,
            #[serde(default)]
            cancellation_request_ids: BoundedVec<CancellationRequestId, SEMANTIC_ARRAY_MAX_ITEMS>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(
            wire.record_ids.into_inner(),
            wire.event_ids.into_inner(),
            wire.effect_ids.into_inner(),
            wire.interaction_ids.into_inner(),
            wire.message_ids.into_inner(),
            wire.turn_ids.into_inner(),
            wire.model_request_ids.into_inner(),
            wire.tool_batch_ids.into_inner(),
            wire.tool_call_ids.into_inner(),
            wire.append_batch_ids.into_inner(),
            wire.cancellation_request_ids.into_inner(),
        )
        .map_err(de::Error::custom)
    }
}

/// Shared reference validation errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RefsError {
    /// Label failed non-empty / size / NUL checks.
    #[error("invalid {field} label")]
    InvalidLabel {
        /// Field name.
        field: &'static str,
    },
    /// Serialization failed while building canonical bytes.
    #[error("serialize failed: {detail}")]
    Serialize {
        /// Detail.
        detail: String,
    },
    /// Semantic map exceeded its v1 entry ceiling.
    #[error("{field} has {len} entries; max {max}")]
    TooManyEntries {
        /// Field name.
        field: &'static str,
        /// Observed entry count.
        len: usize,
        /// Maximum entry count.
        max: usize,
    },
    /// Semantic array exceeded its v1 item ceiling.
    #[error("{field} has {len} items; max {max}")]
    TooManyItems {
        /// Field name.
        field: &'static str,
        /// Observed item count.
        len: usize,
        /// Maximum item count.
        max: usize,
    },
}

impl RefsError {
    /// Stable error code for fixtures.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidLabel { .. } => "invalid_label",
            Self::Serialize { .. } => "serialize_failed",
            Self::TooManyEntries { .. } => "too_many_entries",
            Self::TooManyItems { .. } => "too_many_items",
        }
    }
}

pub(crate) fn validated_label(value: &str, field: &'static str) -> Result<Arc<str>, RefsError> {
    if !crate::label_is_valid(value) {
        return Err(RefsError::InvalidLabel { field });
    }
    Ok(Arc::<str>::from(value))
}

pub(crate) fn validated_text(value: &str, field: &'static str) -> Result<Arc<str>, RefsError> {
    if value.is_empty() || value.len() > TEXT_MAX_BYTES || value.as_bytes().contains(&0) {
        return Err(RefsError::InvalidLabel { field });
    }
    Ok(Arc::<str>::from(value))
}

fn validate_label_ref<E>(value: &str, field: &'static str) -> Result<(), E>
where
    E: serde::ser::Error,
{
    if !crate::label_is_valid(value) {
        return Err(E::custom(RefsError::InvalidLabel { field }));
    }
    Ok(())
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn serialize_micros<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn deserialize_micros<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(de::Error::custom(
            "micros must be a canonical decimal string",
        ));
    }
    text.parse::<u64>()
        .map_err(|_| de::Error::custom("micros overflow or invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ComponentId;

    #[test]
    fn principal_and_usage_roundtrip() {
        let principal = PrincipalRef::try_new("https://issuer.example", "user-1", Some("tenant-a"))
            .expect("principal");
        let json = serde_json::to_string(&principal).expect("ser");
        let back: PrincipalRef = serde_json::from_str(&json).expect("de");
        assert_eq!(back, principal);

        let usage = Usage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            total_tokens: Some(12),
            cost: Some(CostAmount::try_new("USD", 1500, "price-v1").expect("cost")),
            extension_counters: BTreeMap::new(),
        };
        let json = serde_json::to_string(&usage).expect("ser");
        assert!(json.contains("\"1500\""));
        let back: Usage = serde_json::from_str(&json).expect("de");
        assert_eq!(back, usage);

        let component = ComponentRef::new(
            ComponentId::parse("finstack.model.example").expect("id"),
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        );
        assert_eq!(component.version().unwrap().major, 1);
    }

    #[test]
    fn cost_micros_boundary_values_round_trip_as_canonical_decimal_strings() {
        for micros in [0, (1_u64 << 53) - 1, 1_u64 << 53, u64::MAX] {
            let amount = CostAmount::try_new("USD", micros, "price-v1").expect("cost");
            let encoded = serde_json::to_string(&amount).expect("serialize cost");
            assert!(encoded.contains(&format!(r#""micros":"{micros}""#)));
            let decoded: CostAmount = serde_json::from_str(&encoded).expect("deserialize cost");
            assert_eq!(decoded, amount);
        }
        for invalid in [
            r#"{"unit":"USD","micros":0,"pricing_policy_version":"price-v1"}"#,
            r#"{"unit":"USD","micros":"00","pricing_policy_version":"price-v1"}"#,
        ] {
            assert!(serde_json::from_str::<CostAmount>(invalid).is_err());
        }
    }

    #[test]
    fn role_and_queue_assignee_hints_round_trip() {
        for hint in [
            AssigneeHint::Role(Arc::<str>::from("reviewer")),
            AssigneeHint::Queue(Arc::<str>::from("ops")),
        ] {
            let json = serde_json::to_string(&hint).expect("serialize hint");
            let round: AssigneeHint = serde_json::from_str(&json).expect("deserialize hint");
            assert_eq!(round, hint);
        }
    }

    #[test]
    fn allocated_id_arrays_and_usage_maps_enforce_semantic_ceilings() {
        let record_id = RecordId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("record");
        let ids = AllocatedIds {
            record_ids: vec![record_id; crate::content::CONTENT_MAX_ITEMS + 1],
            ..AllocatedIds::default()
        };
        let json = serde_json::to_string(&ids).expect("serialize ids");
        assert!(
            serde_json::from_str::<AllocatedIds>(&json).is_err(),
            "oversized allocated-id array must fail"
        );

        let usage = Usage {
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            cost: None,
            extension_counters: (0..33)
                .map(|index| {
                    (
                        LimitKey::parse(format!("app.counter-{index}")).expect("key"),
                        1,
                    )
                })
                .collect(),
        };
        let json = serde_json::to_string(&usage).expect("serialize usage");
        assert!(
            serde_json::from_str::<Usage>(&json).is_err(),
            "more than 32 extension counters must fail"
        );
    }
}
