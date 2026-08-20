//! Strict declarative agent and capability specifications.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{
    AGENT_SPEC_DIGEST_SCHEMA_VERSION, AgentId, BundleId, CapabilityId, ComponentId, ComponentRef,
    DOMAIN_AGENT_SPEC, Digest, MiddlewareRef, RawJson, RunLimits,
};
use serde::{Deserialize, Serialize, de};
use thiserror::Error;

pub use finstack_ai_runtime::ApprovalGrantMode;

#[cfg(test)]
mod tests;

/// Current `AgentSpec` JSON schema version.
pub const AGENT_SPEC_SCHEMA_VERSION: u16 = 1;
const MAX_ARRAY_ITEMS: usize = 4_096;
const MAX_MAP_ENTRIES: usize = 256;
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// One bounded declarative instruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstructionSpec {
    text: Arc<str>,
}

impl InstructionSpec {
    /// Construct a non-empty instruction within the v1 text ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`AgentSpecError::InvalidInstruction`] for empty, NUL-bearing,
    /// or oversized text.
    pub fn try_new(text: impl Into<Arc<str>>) -> Result<Self, AgentSpecError> {
        let text = text.into();
        validate_instruction(&text)?;
        Ok(Self { text })
    }

    /// Borrow the instruction text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl<'de> Deserialize<'de> for InstructionSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            text: Arc<str>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::try_new(wire.text).map_err(de::Error::custom)
    }
}

/// Explicit reference to a capability definition in a locked catalog.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRef {
    /// Capability identity.
    pub id: CapabilityId,
    /// Bundle containing the definition, when the reference is bundle-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<BundleId>,
}

/// Declarative capability delivery mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityActivation {
    /// Expand into the base resolved plan.
    Always,
    /// Make available for explicit application activation.
    Application,
    /// Make available to the bounded model-catalog activation policy.
    Model,
    /// Keep the definition unavailable for activation.
    Disabled,
}

/// Data-only capability definition; executable handles never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySpec {
    /// Capability identity.
    pub id: CapabilityId,
    /// Compact non-secret description.
    pub description: Arc<str>,
    /// Instructions contributed after activation.
    pub instructions: Arc<[InstructionSpec]>,
    /// Toolset references contributed after activation.
    pub toolsets: Arc<[ComponentRef]>,
    /// Context-provider references contributed after activation.
    pub context_providers: Arc<[ComponentRef]>,
    /// Middleware references contributed after activation.
    pub middleware: Arc<[ComponentRef]>,
    /// Delivery mode.
    pub activation: CapabilityActivation,
}

impl CapabilitySpec {
    /// Validate intrinsic bounds and duplicate component references.
    ///
    /// # Errors
    ///
    /// Returns a stable specification error when the definition is malformed.
    pub fn validate(&self) -> Result<(), AgentSpecError> {
        validate_description(&self.description)?;
        validate_slice_len("capability.instructions", self.instructions.len())?;
        validate_slice_len("capability.toolsets", self.toolsets.len())?;
        validate_slice_len("capability.context_providers", self.context_providers.len())?;
        validate_slice_len("capability.middleware", self.middleware.len())?;
        ensure_unique_components("capability.toolsets", &self.toolsets)?;
        ensure_unique_components("capability.context_providers", &self.context_providers)?;
        ensure_unique_components("capability.middleware", &self.middleware)?;
        Ok(())
    }
}

impl<'de> Deserialize<'de> for CapabilitySpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            id: CapabilityId,
            description: Arc<str>,
            #[serde(default)]
            instructions: Arc<[InstructionSpec]>,
            #[serde(default)]
            toolsets: Arc<[ComponentRef]>,
            #[serde(default)]
            context_providers: Arc<[ComponentRef]>,
            #[serde(default)]
            middleware: Arc<[ComponentRef]>,
            activation: CapabilityActivation,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            id: wire.id,
            description: wire.description,
            instructions: wire.instructions,
            toolsets: wire.toolsets,
            context_providers: wire.context_providers,
            middleware: wire.middleware,
            activation: wire.activation,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

/// Explicit child-run admission policy attached to an agent specification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ChildRunPolicy {
    /// Reject every child invocation.
    #[default]
    Deny,
    /// Allow children up to the configured inclusive depth.
    Allow {
        /// Maximum child depth accepted by this agent.
        max_depth: u16,
    },
}

/// Serializable run-policy subset owned by agent composition.
///
/// [`Self::approval_grant`] selects how paid-tool approvals park and
/// release. It does not relax [`finstack_ai_runtime::ApprovalRequirement::Policy`]:
/// that floor still applies on every catalog. Existing specs that omit
/// `approval_grant` deserialize as [`ApprovalGrantMode::PerCall`].
///
/// # Examples
///
/// ```
/// use finstack_ai::{ApprovalGrantMode, RunPolicy};
///
/// let policy = RunPolicy {
///     approval_grant: ApprovalGrantMode::InformedBatch,
///     ..RunPolicy::default()
/// };
/// assert_eq!(policy.approval_grant, ApprovalGrantMode::InformedBatch);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPolicy {
    /// Components whose configuration can be overridden at run acceptance.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub run_override_allowlist: BTreeSet<ComponentId>,
    /// Child-run admission policy, denied by default.
    #[serde(default)]
    pub child_runs: ChildRunPolicy,
    /// How paid-tool approvals are parked and released.
    ///
    /// [`ApprovalGrantMode::PerCall`] (default) parks once per unpaid
    /// Policy or Required tool call. [`ApprovalGrantMode::InformedBatch`]
    /// parks once listing every remaining unpaid paid tool. Neither mode
    /// can weaken [`finstack_ai_runtime::ApprovalRequirement::Policy`].
    #[serde(default, skip_serializing_if = "approval_grant_is_per_call")]
    pub approval_grant: ApprovalGrantMode,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if requires fn(&T) -> bool"
)]
fn approval_grant_is_per_call(mode: &ApprovalGrantMode) -> bool {
    matches!(mode, ApprovalGrantMode::PerCall)
}

/// Strict immutable agent specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    /// JSON schema version.
    pub schema_version: u16,
    /// Agent identity.
    pub id: AgentId,
    /// Model component.
    pub model: ComponentRef,
    /// Ordered base instructions.
    pub instructions: Arc<[InstructionSpec]>,
    /// Ordered toolset components.
    pub toolsets: Arc<[ComponentRef]>,
    /// Ordered context-provider components.
    pub context_providers: Arc<[ComponentRef]>,
    /// Ordered middleware components and optional stage assertions.
    pub middleware: Arc<[MiddlewareRef]>,
    /// Journal store for an executable agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<ComponentRef>,
    /// Ordered read-only observers.
    pub observers: Arc<[ComponentRef]>,
    /// Explicit declarative capability references.
    pub capabilities: Arc<[CapabilityRef]>,
    /// Run limits.
    pub limits: RunLimits,
    /// Run and child-invocation policy.
    pub policy: RunPolicy,
    /// Extension-owned non-secret configuration.
    pub extension_config: BTreeMap<ComponentId, RawJson>,
}

impl AgentSpec {
    /// Start a declarative specification builder.
    ///
    /// This produces an [`AgentSpec`] only. Construct a live agent from ready
    /// handles with [`crate::Agent::builder`].
    #[must_use]
    pub fn builder(id: AgentId, model: ComponentRef, store: ComponentRef) -> AgentBuilder {
        AgentBuilder::new(id, model, store)
    }

    /// Validate schema version, bounds, and duplicate identities.
    ///
    /// # Errors
    ///
    /// Returns a stable specification error before any runtime handle is built.
    pub fn validate(&self) -> Result<(), AgentSpecError> {
        if self.schema_version != AGENT_SPEC_SCHEMA_VERSION {
            return Err(AgentSpecError::UnsupportedSchemaVersion {
                actual: self.schema_version,
            });
        }
        validate_slice_len("instructions", self.instructions.len())?;
        validate_slice_len("toolsets", self.toolsets.len())?;
        validate_slice_len("context_providers", self.context_providers.len())?;
        validate_slice_len("middleware", self.middleware.len())?;
        validate_slice_len("observers", self.observers.len())?;
        validate_slice_len("capabilities", self.capabilities.len())?;
        if self.extension_config.len() > MAX_MAP_ENTRIES {
            return Err(AgentSpecError::TooManyItems {
                field: "extension_config",
                len: self.extension_config.len(),
                max: MAX_MAP_ENTRIES,
            });
        }
        self.limits
            .validate()
            .map_err(|error| AgentSpecError::InvalidLimits {
                message: error.to_string(),
            })?;
        ensure_unique_components("toolsets", &self.toolsets)?;
        ensure_unique_components("context_providers", &self.context_providers)?;
        ensure_unique_middleware(&self.middleware)?;
        ensure_unique_components("observers", &self.observers)?;
        let mut capabilities = BTreeSet::new();
        for reference in self.capabilities.iter() {
            let key = (reference.bundle.clone(), reference.id.clone());
            if !capabilities.insert(key) {
                return Err(AgentSpecError::DuplicateCapability {
                    capability: reference.id.clone(),
                });
            }
        }
        Ok(())
    }

    /// RFC 8785 canonical JSON representation used by the agent-spec digest.
    ///
    /// # Errors
    ///
    /// Returns a serialization error if canonical encoding fails.
    pub fn canonical_json(&self) -> Result<Vec<u8>, AgentSpecError> {
        self.validate()?;
        serde_json_canonicalizer::to_vec(self).map_err(|error| AgentSpecError::Serialize {
            message: error.to_string(),
        })
    }

    /// Compute the domain-separated canonical specification fingerprint.
    ///
    /// # Errors
    ///
    /// Returns a specification error if validation or canonicalization fails.
    pub fn fingerprint(&self) -> Result<Digest, AgentSpecError> {
        let canonical = self.canonical_json()?;
        Digest::domain_separated(
            DOMAIN_AGENT_SPEC,
            AGENT_SPEC_DIGEST_SCHEMA_VERSION,
            &canonical,
        )
        .map_err(|error| AgentSpecError::Serialize {
            message: error.to_string(),
        })
    }

    /// Parse and validate one strict JSON specification.
    ///
    /// # Errors
    ///
    /// Returns a structured parse or semantic validation error.
    pub fn from_json(bytes: &[u8]) -> Result<Self, AgentSpecError> {
        serde_json::from_slice(bytes).map_err(|error| AgentSpecError::Parse {
            message: error.to_string(),
        })
    }
}

impl<'de> Deserialize<'de> for AgentSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema_version: u16,
            id: AgentId,
            model: ComponentRef,
            #[serde(default)]
            instructions: Arc<[InstructionSpec]>,
            #[serde(default)]
            toolsets: Arc<[ComponentRef]>,
            #[serde(default)]
            context_providers: Arc<[ComponentRef]>,
            #[serde(default)]
            middleware: Arc<[MiddlewareRef]>,
            #[serde(default)]
            store: Option<ComponentRef>,
            #[serde(default)]
            observers: Arc<[ComponentRef]>,
            #[serde(default)]
            capabilities: Arc<[CapabilityRef]>,
            limits: RunLimits,
            #[serde(default)]
            policy: RunPolicy,
            #[serde(default)]
            extension_config: BTreeMap<ComponentId, RawJson>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let value = Self {
            schema_version: wire.schema_version,
            id: wire.id,
            model: wire.model,
            instructions: wire.instructions,
            toolsets: wire.toolsets,
            context_providers: wire.context_providers,
            middleware: wire.middleware,
            store: wire.store,
            observers: wire.observers,
            capabilities: wire.capabilities,
            limits: wire.limits,
            policy: wire.policy,
            extension_config: wire.extension_config,
        };
        value.validate().map_err(de::Error::custom)?;
        Ok(value)
    }
}

/// Incremental Rust builder that produces the same immutable [`AgentSpec`] as JSON.
///
/// This builder never constructs a live [`crate::Agent`]. Prefer
/// [`AgentSpec::builder`] for spec data. Prefer [`crate::Agent::builder`]
/// ([`crate::NativeAgentBuilder`]) when composing ready native port handles.
#[derive(Debug, Clone)]
pub struct AgentBuilder {
    spec: AgentSpec,
}

impl AgentBuilder {
    /// Start a minimal agent with an explicit model and journal store.
    #[must_use]
    pub fn new(id: AgentId, model: ComponentRef, store: ComponentRef) -> Self {
        Self {
            spec: AgentSpec {
                schema_version: AGENT_SPEC_SCHEMA_VERSION,
                id,
                model,
                instructions: Arc::from([]),
                toolsets: Arc::from([]),
                context_providers: Arc::from([]),
                middleware: Arc::from([]),
                store: Some(store),
                observers: Arc::from([]),
                capabilities: Arc::from([]),
                limits: RunLimits::empty(),
                policy: RunPolicy::default(),
                extension_config: BTreeMap::new(),
            },
        }
    }

    /// Replace the ordered instruction list.
    #[must_use]
    pub fn instructions(mut self, instructions: impl Into<Arc<[InstructionSpec]>>) -> Self {
        self.spec.instructions = instructions.into();
        self
    }

    /// Replace the ordered toolset list.
    #[must_use]
    pub fn toolsets(mut self, toolsets: impl Into<Arc<[ComponentRef]>>) -> Self {
        self.spec.toolsets = toolsets.into();
        self
    }

    /// Replace the ordered context-provider list.
    #[must_use]
    pub fn context_providers(mut self, providers: impl Into<Arc<[ComponentRef]>>) -> Self {
        self.spec.context_providers = providers.into();
        self
    }

    /// Replace the ordered middleware list.
    #[must_use]
    pub fn middleware(mut self, middleware: impl Into<Arc<[MiddlewareRef]>>) -> Self {
        self.spec.middleware = middleware.into();
        self
    }

    /// Replace the ordered observer list.
    #[must_use]
    pub fn observers(mut self, observers: impl Into<Arc<[ComponentRef]>>) -> Self {
        self.spec.observers = observers.into();
        self
    }

    /// Replace explicit capability references.
    #[must_use]
    pub fn capabilities(mut self, capabilities: impl Into<Arc<[CapabilityRef]>>) -> Self {
        self.spec.capabilities = capabilities.into();
        self
    }

    /// Replace run limits.
    #[must_use]
    pub fn limits(mut self, limits: RunLimits) -> Self {
        self.spec.limits = limits;
        self
    }

    /// Replace composition policy.
    #[must_use]
    pub fn policy(mut self, policy: RunPolicy) -> Self {
        self.spec.policy = policy;
        self
    }

    /// Attach extension configuration, replacing an earlier value for the same component.
    #[must_use]
    pub fn extension_config(mut self, component: ComponentId, config: RawJson) -> Self {
        self.spec.extension_config.insert(component, config);
        self
    }

    /// Validate and freeze the specification.
    ///
    /// # Errors
    ///
    /// Returns a stable validation error before component resolution starts.
    pub fn build(self) -> Result<AgentSpec, AgentSpecError> {
        self.spec.validate()?;
        Ok(self.spec)
    }
}

/// Strict specification construction or decoding failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentSpecError {
    /// Unsupported top-level schema version.
    #[error("unsupported AgentSpec schema version {actual}; expected 1")]
    UnsupportedSchemaVersion {
        /// Decoded schema version.
        actual: u16,
    },
    /// A list or map exceeded a frozen v1 ceiling.
    #[error("{field} has {len} items; maximum is {max}")]
    TooManyItems {
        /// Failing field.
        field: &'static str,
        /// Observed length.
        len: usize,
        /// Maximum length.
        max: usize,
    },
    /// Duplicate component identity in one ordered role.
    #[error("duplicate component {component} in {field}")]
    DuplicateComponent {
        /// Failing field.
        field: &'static str,
        /// Duplicate component.
        component: ComponentId,
    },
    /// Duplicate capability identity.
    #[error("duplicate capability reference {capability}")]
    DuplicateCapability {
        /// Duplicate capability.
        capability: CapabilityId,
    },
    /// Invalid instruction text.
    #[error("invalid instruction: {reason}")]
    InvalidInstruction {
        /// Stable reason.
        reason: &'static str,
    },
    /// Invalid capability description.
    #[error("invalid capability description: {reason}")]
    InvalidDescription {
        /// Stable reason.
        reason: &'static str,
    },
    /// Invalid run limits.
    #[error("invalid run limits: {message}")]
    InvalidLimits {
        /// Underlying validation message.
        message: String,
    },
    /// Strict JSON parse failure.
    #[error("AgentSpec JSON is invalid: {message}")]
    Parse {
        /// Decoder message.
        message: String,
    },
    /// Canonical serialization failure.
    #[error("AgentSpec serialization failed: {message}")]
    Serialize {
        /// Encoder message.
        message: String,
    },
}

fn validate_instruction(text: &str) -> Result<(), AgentSpecError> {
    validate_bounded_text(text, |reason| AgentSpecError::InvalidInstruction { reason })
}

fn validate_description(text: &str) -> Result<(), AgentSpecError> {
    validate_bounded_text(text, |reason| AgentSpecError::InvalidDescription { reason })
}

fn validate_bounded_text(
    text: &str,
    invalid: impl Fn(&'static str) -> AgentSpecError,
) -> Result<(), AgentSpecError> {
    if text.is_empty() {
        return Err(invalid("empty"));
    }
    if text.as_bytes().contains(&0) {
        return Err(invalid("contains_nul"));
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(invalid("too_large"));
    }
    Ok(())
}

fn validate_slice_len(field: &'static str, len: usize) -> Result<(), AgentSpecError> {
    if len > MAX_ARRAY_ITEMS {
        return Err(AgentSpecError::TooManyItems {
            field,
            len,
            max: MAX_ARRAY_ITEMS,
        });
    }
    Ok(())
}

fn ensure_unique_components(
    field: &'static str,
    components: &[ComponentRef],
) -> Result<(), AgentSpecError> {
    let mut seen = BTreeSet::new();
    for component in components {
        if !seen.insert(component.id().clone()) {
            return Err(AgentSpecError::DuplicateComponent {
                field,
                component: component.id().clone(),
            });
        }
    }
    Ok(())
}

fn ensure_unique_middleware(middleware: &[MiddlewareRef]) -> Result<(), AgentSpecError> {
    let mut seen = BTreeSet::new();
    for reference in middleware {
        if !seen.insert(reference.component().id().clone()) {
            return Err(AgentSpecError::DuplicateComponent {
                field: "middleware",
                component: reference.component().id().clone(),
            });
        }
    }
    Ok(())
}
