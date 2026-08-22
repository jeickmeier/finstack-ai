//! Validated tool-policy configuration types.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::ToolId;
use serde::Serialize;
use thiserror::Error;

/// Maximum number of roles in a [`RoleAllowlist`].
pub(crate) const MAX_ROLES: usize = 128;
/// Maximum number of tools in any tool set (aliases `ModelRequestDraft::MAX_TOOLS`).
pub(crate) const MAX_TOOLS_PER_SET: usize =
    finstack_ai_runtime::ports::model::ModelRequestDraft::MAX_TOOLS;
/// Maximum number of jailbreak trigger patterns.
pub(crate) const MAX_PATTERNS: usize = 64;
/// Maximum byte length of a single jailbreak trigger pattern.
pub(crate) const MAX_PATTERN_BYTES: usize = 256;
/// Maximum byte length of a role name.
pub(crate) const MAX_ROLE_NAME_BYTES: usize = 256;
/// Maximum run-relation depth accepted by the kernel (aliases `finstack_ai_kernel::MAX_RUN_RELATION_DEPTH`).
pub(crate) const MAX_KERNEL_DEPTH: u16 = finstack_ai_kernel::MAX_RUN_RELATION_DEPTH;

/// Tool-policy leaf construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ToolPolicyError {
    /// Configuration is malformed.
    #[error("tool_policy_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Action taken when a jailbreak trigger pattern matches.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JailbreakAction {
    /// Restrict the visible tool set to the given tools.
    RestrictTo(BTreeSet<ToolId>),
    /// Fail the stage outright.
    Fail,
}

/// Per-role tool allowlist, plus a default for unmapped roles.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct RoleAllowlist {
    roles: BTreeMap<Arc<str>, BTreeSet<ToolId>>,
    default_allowed: BTreeSet<ToolId>,
}

impl RoleAllowlist {
    /// Per-role allowed tool sets.
    #[must_use]
    pub(crate) fn roles(&self) -> &BTreeMap<Arc<str>, BTreeSet<ToolId>> {
        &self.roles
    }

    /// Tools allowed for roles absent from [`Self::roles`].
    #[must_use]
    pub(crate) fn default_allowed(&self) -> &BTreeSet<ToolId> {
        &self.default_allowed
    }
}

/// Per-run cap on the number of write-classified tool calls.
///
/// `max_write_calls = 0` is a legitimate read-only mode: write-class tools
/// are never made visible.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct WriteBudget {
    max_write_calls: u32,
}

impl WriteBudget {
    /// Maximum number of write-classified tool calls allowed.
    #[must_use]
    pub(crate) fn max_write_calls(&self) -> u32 {
        self.max_write_calls
    }
}

/// Jailbreak-trigger patterns and the action taken when one matches.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct JailbreakTriggers {
    patterns: Arc<[Arc<str>]>,
    action: JailbreakAction,
}

impl JailbreakTriggers {
    /// Trigger patterns.
    #[must_use]
    pub(crate) fn patterns(&self) -> &[Arc<str>] {
        &self.patterns
    }

    /// Action taken when a pattern matches.
    #[must_use]
    pub(crate) fn action(&self) -> &JailbreakAction {
        &self.action
    }
}

/// Restricts a tool set once the run's child-agent depth reaches a threshold.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ChildDepthGate {
    max_depth: u16,
    restricted: BTreeSet<ToolId>,
}

impl ChildDepthGate {
    /// Depth at or beyond which `restricted` applies.
    #[must_use]
    pub(crate) fn max_depth(&self) -> u16 {
        self.max_depth
    }

    /// Tools removed once `max_depth` is reached.
    #[must_use]
    pub(crate) fn restricted(&self) -> &BTreeSet<ToolId> {
        &self.restricted
    }
}

/// Validated tool-policy configuration, built incrementally via `with_*` methods.
///
/// Construct with [`Self::new`] and the `with_*` builders. Rule structs and
/// inspection accessors are crate-private.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolPolicyConfig {
    role_allowlist: Option<RoleAllowlist>,
    write_budget: Option<WriteBudget>,
    jailbreak: Option<JailbreakTriggers>,
    child_depth: Option<ChildDepthGate>,
}

impl ToolPolicyConfig {
    /// Start an empty configuration. Add at least one rule before use.
    ///
    /// `Default` is intentionally not implemented: construct via this constructor
    /// and the `with_*` builders.
    #[must_use]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            role_allowlist: None,
            write_budget: None,
            jailbreak: None,
            child_depth: None,
        }
    }

    /// Add a role-based tool allowlist.
    ///
    /// # Errors
    ///
    /// Rejects an oversized role set, an oversized per-role or default tool
    /// set, an oversized or empty role name, or a role-allowlist slot
    /// already set.
    pub fn with_role_allowlist(
        mut self,
        roles: BTreeMap<Arc<str>, BTreeSet<ToolId>>,
        default_allowed: BTreeSet<ToolId>,
    ) -> Result<Self, ToolPolicyError> {
        if self.role_allowlist.is_some() {
            return Err(ToolPolicyError::Configuration {
                reason: "duplicate_rule",
            });
        }
        if roles.len() > MAX_ROLES {
            return Err(ToolPolicyError::Configuration {
                reason: "too_many_roles",
            });
        }
        if default_allowed.len() > MAX_TOOLS_PER_SET {
            return Err(ToolPolicyError::Configuration {
                reason: "too_many_tools",
            });
        }
        for (name, tools) in &roles {
            if name.is_empty() {
                return Err(ToolPolicyError::Configuration {
                    reason: "empty_role_name",
                });
            }
            if name.len() > MAX_ROLE_NAME_BYTES {
                return Err(ToolPolicyError::Configuration {
                    reason: "role_name_too_long",
                });
            }
            if name.contains('\0') {
                return Err(ToolPolicyError::Configuration {
                    reason: "role_name_contains_nul",
                });
            }
            if tools.len() > MAX_TOOLS_PER_SET {
                return Err(ToolPolicyError::Configuration {
                    reason: "too_many_tools",
                });
            }
        }
        self.role_allowlist = Some(RoleAllowlist {
            roles,
            default_allowed,
        });
        Ok(self)
    }

    /// Add a per-run write-call budget.
    ///
    /// # Errors
    ///
    /// Rejects a write-budget slot already set.
    pub fn with_write_budget(mut self, max_write_calls: u32) -> Result<Self, ToolPolicyError> {
        if self.write_budget.is_some() {
            return Err(ToolPolicyError::Configuration {
                reason: "duplicate_rule",
            });
        }
        self.write_budget = Some(WriteBudget { max_write_calls });
        Ok(self)
    }

    /// Add jailbreak trigger patterns and the action taken when one matches.
    ///
    /// # Errors
    ///
    /// Rejects too many patterns, an empty or oversized pattern, a pattern
    /// containing a NUL byte, or a jailbreak slot already set.
    pub fn with_jailbreak_triggers(
        mut self,
        patterns: Vec<Arc<str>>,
        action: JailbreakAction,
    ) -> Result<Self, ToolPolicyError> {
        if self.jailbreak.is_some() {
            return Err(ToolPolicyError::Configuration {
                reason: "duplicate_rule",
            });
        }
        if patterns.len() > MAX_PATTERNS {
            return Err(ToolPolicyError::Configuration {
                reason: "too_many_patterns",
            });
        }
        let mut normalized_patterns = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            let normalized = pattern.trim().to_lowercase();
            if normalized.is_empty() {
                return Err(ToolPolicyError::Configuration {
                    reason: "empty_pattern",
                });
            }
            if normalized.len() > MAX_PATTERN_BYTES {
                return Err(ToolPolicyError::Configuration {
                    reason: "pattern_too_long",
                });
            }
            if normalized.contains('\0') {
                return Err(ToolPolicyError::Configuration {
                    reason: "pattern_contains_nul",
                });
            }
            normalized_patterns.push(Arc::from(normalized));
        }
        if let JailbreakAction::RestrictTo(tools) = &action
            && tools.len() > MAX_TOOLS_PER_SET
        {
            return Err(ToolPolicyError::Configuration {
                reason: "too_many_tools",
            });
        }
        self.jailbreak = Some(JailbreakTriggers {
            patterns: normalized_patterns.into(),
            action,
        });
        Ok(self)
    }

    /// Add a child-agent depth gate.
    ///
    /// The gate compares the run's live relation depth
    /// (`RunCallContext::relation_depth`) against `max_depth` at every stage
    /// invocation; when `relation_depth >= max_depth`, the `restricted`
    /// tools are hidden.
    ///
    /// `max_depth = 0` is degenerate: since `relation_depth >= 0` always
    /// holds, the gate fires on every run, including the root run at depth
    /// 0, hiding the `restricted` tools everywhere.
    ///
    /// # Errors
    ///
    /// Rejects `max_depth` above the kernel's run-relation depth cap, an
    /// oversized restricted tool set, or a child-depth slot already set.
    pub fn with_child_depth_gate(
        mut self,
        max_depth: u16,
        restricted: BTreeSet<ToolId>,
    ) -> Result<Self, ToolPolicyError> {
        if self.child_depth.is_some() {
            return Err(ToolPolicyError::Configuration {
                reason: "duplicate_rule",
            });
        }
        if max_depth > MAX_KERNEL_DEPTH {
            return Err(ToolPolicyError::Configuration {
                reason: "depth_exceeds_kernel_cap",
            });
        }
        if restricted.len() > MAX_TOOLS_PER_SET {
            return Err(ToolPolicyError::Configuration {
                reason: "too_many_tools",
            });
        }
        if restricted.is_empty() {
            return Err(ToolPolicyError::Configuration {
                reason: "empty_restricted_tools",
            });
        }
        self.child_depth = Some(ChildDepthGate {
            max_depth,
            restricted,
        });
        Ok(self)
    }

    /// The configured role allowlist, if any.
    #[must_use]
    pub(crate) fn role_allowlist(&self) -> Option<&RoleAllowlist> {
        self.role_allowlist.as_ref()
    }

    /// The configured write budget, if any.
    #[must_use]
    pub(crate) fn write_budget(&self) -> Option<&WriteBudget> {
        self.write_budget.as_ref()
    }

    /// The configured jailbreak triggers, if any.
    #[must_use]
    pub(crate) fn jailbreak(&self) -> Option<&JailbreakTriggers> {
        self.jailbreak.as_ref()
    }

    /// The configured child-depth gate, if any.
    #[must_use]
    pub(crate) fn child_depth(&self) -> Option<&ChildDepthGate> {
        self.child_depth.as_ref()
    }

    /// True when no rule has been configured.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.role_allowlist.is_none()
            && self.write_budget.is_none()
            && self.jailbreak.is_none()
            && self.child_depth.is_none()
    }
}
