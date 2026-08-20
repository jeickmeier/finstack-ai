//! Pure policy evaluation core: ToolId-set narrowing rules.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::ToolId;

use crate::ToolPolicyConfig;

/// Outcome of evaluating the policy against one stage's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // consumed by invoke wiring (Tasks 4-5)
pub(crate) enum PolicyVerdict {
    /// No rule narrowed anything: emit `StageOutcome::Continue`.
    Identity,
    /// Retain exactly these tools: emit `StageOutcome::FilterTools`.
    Retain(BTreeSet<ToolId>),
    /// A `JailbreakAction::Fail` trigger fired: emit `StageOutcome::Fail`.
    Fail { reason: &'static str },
}

/// ToolId-set rules only (role allowlist + child-depth gate), applied to a
/// known universe of visible tools. Pure; used by both stages.
///
/// Role allowlist: effective allow = `default_allowed ∪ ⋃(roles[r] for r in
/// granted_roles)`. Deny-by-default: a tool not in the effective allow set is
/// dropped. Roles come from `ctx.run.authorization.roles`; unknown granted
/// roles are ignored (they contribute nothing). No role rule configured → no
/// narrowing from this rule.
///
/// Child-depth gate: if `current_depth >= max_depth`, drop every tool in
/// `restricted` from the result. Below the threshold → no narrowing.
///
/// Returns `universe ∩ (rule constraints)`; leaves never "add" tools
/// (narrowing is monotone — the fold intersects anyway).
#[allow(dead_code)] // consumed by invoke wiring (Tasks 4-5)
pub(crate) fn narrow_universe(
    config: &ToolPolicyConfig,
    universe: &BTreeSet<ToolId>,
    granted_roles: &[Arc<str>],
) -> BTreeSet<ToolId> {
    let mut result = universe.clone();

    if let Some(role_allowlist) = config.role_allowlist() {
        let mut effective_allow = role_allowlist.default_allowed().clone();
        for role in granted_roles {
            if let Some(tools) = role_allowlist.roles().get(role) {
                effective_allow.extend(tools.iter().cloned());
            }
        }
        result.retain(|tool| effective_allow.contains(tool));
    }

    if let Some(gate) = config.child_depth()
        && gate.current_depth() >= gate.max_depth()
    {
        for tool in gate.restricted() {
            result.remove(tool);
        }
    }

    result
}
