//! Pure policy evaluation core: ToolId-set narrowing rules.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{ContentBlock, MessageRole, ToolId};
use finstack_ai_runtime::{BeforeModelInput, SideEffectClass};

use crate::{JailbreakAction, TOOL_POLICY_JAILBREAK_TRIGGERED, ToolPolicyConfig};

/// Outcome of evaluating the policy against one stage's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Evaluate the full `before_model` policy: [`narrow_universe`] (role
/// allowlist + child-depth gate), then the write budget, then jailbreak
/// triggers.
///
/// # Write budget
///
/// Counts `ContentBlock::ToolCall` blocks in `MessageRole::Assistant`
/// messages of `input.request.messages` whose `tool_name()` names a tool
/// still present in `input.request.tools` with `side_effect !=
/// SideEffectClass::ReadOnly`. Once that count reaches `max_write_calls`,
/// every non-`ReadOnly` tool is dropped from the retain set.
///
/// This is a pure function of the draft, deterministic on replay — not an
/// audited ledger. A call whose tool has since been filtered out of the
/// draft (by an earlier rule, or a prior stage) does not count toward the
/// budget: it is a visibility brake on what the model can still request,
/// not a spend ledger. A journal-backed budget that survives tool
/// filtering would need a different recovery class; YAGNI for now.
///
/// # Jailbreak triggers
///
/// Case-insensitive substring scan of every `ContentBlock::Text` block in
/// `MessageRole::User` messages, and of every `ContentBlock::Text` block
/// nested inside a `ContentBlock::ToolResult` in `MessageRole::Tool`
/// messages. Assistant and system text is trusted and never scanned.
/// Case-insensitivity is via `str::to_lowercase` on both the pattern and
/// the scanned text; patterns are expected to be ASCII-ish config strings,
/// but a non-ASCII pattern still matches — only against its own lowercased
/// Unicode form, which may not equal every locale's notion of "same word".
/// On the first pattern match: `JailbreakAction::Fail` short-circuits to
/// `PolicyVerdict::Fail`; `JailbreakAction::RestrictTo(safe)` intersects
/// the retain set with `safe` and scanning stops. Work is bounded by the
/// draft's own ceilings: at most `ModelRequestDraft::MAX_MESSAGES` (4 096)
/// messages, each matched against at most `MAX_PATTERNS` (64) patterns of
/// at most `MAX_PATTERN_BYTES` (256) bytes.
///
/// Returns `PolicyVerdict::Identity` when the final retain set equals the
/// full tool universe (keeps the fold identity-clean); otherwise
/// `PolicyVerdict::Retain`.
pub(crate) fn evaluate_before_model(
    config: &ToolPolicyConfig,
    input: &BeforeModelInput,
    granted_roles: &[Arc<str>],
) -> PolicyVerdict {
    let universe: BTreeSet<ToolId> = input
        .request
        .tools
        .iter()
        .map(|tool| tool.id.clone())
        .collect();
    let mut retain = narrow_universe(config, &universe, granted_roles);

    if let Some(budget) = config.write_budget() {
        let write_call_count = input
            .request
            .messages
            .iter()
            .filter(|message| message.role() == MessageRole::Assistant)
            .flat_map(|message| message.content().iter())
            .filter_map(|block| match block {
                ContentBlock::ToolCall(call) => Some(call.tool_name()),
                _ => None,
            })
            .filter(|tool_name| {
                input.request.tools.iter().any(|tool| {
                    tool.model_name.as_ref() == *tool_name
                        && tool.side_effect != SideEffectClass::ReadOnly
                })
            })
            .count();
        let write_call_count = u64::try_from(write_call_count).unwrap_or(u64::MAX);
        if write_call_count >= u64::from(budget.max_write_calls()) {
            let write_tool_ids: BTreeSet<ToolId> = input
                .request
                .tools
                .iter()
                .filter(|tool| tool.side_effect != SideEffectClass::ReadOnly)
                .map(|tool| tool.id.clone())
                .collect();
            retain.retain(|id| !write_tool_ids.contains(id));
        }
    }

    if let Some(jailbreak) = config.jailbreak() {
        let lowered_patterns: Vec<String> = jailbreak
            .patterns()
            .iter()
            .map(|pattern| pattern.to_lowercase())
            .collect();
        'scan: for message in input.request.messages.iter() {
            let texts: Vec<&str> = match message.role() {
                MessageRole::User => message
                    .content()
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.text()),
                        _ => None,
                    })
                    .collect(),
                MessageRole::Tool => message
                    .content()
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolResult(result) => Some(result.content().iter()),
                        _ => None,
                    })
                    .flatten()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.text()),
                        _ => None,
                    })
                    .collect(),
                MessageRole::System | MessageRole::Developer | MessageRole::Assistant => Vec::new(),
            };
            for text in &texts {
                let lowered = text.to_lowercase();
                for pattern in &lowered_patterns {
                    if lowered.contains(pattern.as_str()) {
                        match jailbreak.action() {
                            JailbreakAction::Fail => {
                                return PolicyVerdict::Fail {
                                    reason: TOOL_POLICY_JAILBREAK_TRIGGERED,
                                };
                            }
                            JailbreakAction::RestrictTo(safe) => {
                                retain = retain.intersection(safe).cloned().collect();
                            }
                        }
                        break 'scan;
                    }
                }
            }
        }
    }

    if retain == universe {
        PolicyVerdict::Identity
    } else {
        PolicyVerdict::Retain(retain)
    }
}

/// Evaluate the `before_tool_batch` defense-in-depth policy.
///
/// # Why this stage only enforces complete-set rules
///
/// `StageOutcome::FilterTools` is retain-semantics: a leaf hands back the
/// exact set of tools that should survive, and the runtime turns whatever is
/// *not* in that set into per-call `Deny` → `SyntheticClosure` (see
/// `middleware_tool_policy` in
/// `finstack-ai-runtime/src/exec/settlement/stage.rs`) — a filtered
/// call is answered with a synthetic denial, never silently dropped. That
/// means a leaf may only emit a retain set it knows to be *complete*: every
/// tool the model could legitimately have called must be accounted for, or
/// the leaf ends up denying calls to tools it simply never heard about.
///
/// At `before_model`, [`evaluate_before_model`] sees `input.request.tools`
/// and can narrow that concrete universe. At `before_tool_batch` the leaf
/// sees no catalog at all — only the batch of calls the model already made.
/// So the only rule that can express a complete set here is the role
/// allowlist: its effective allow set, `default_allowed ∪ ⋃(roles[r] for r
/// in granted_roles)`, is complete by construction regardless of what tools
/// exist, because it is defined as an allow set rather than derived from a
/// universe. If a child-depth gate is *also* configured and firing (`
/// current_depth >= max_depth`), its `restricted` set is subtracted from
/// that allow set — the gate only ever removes tools, so this stays
/// complete too.
///
/// A depth-only or write-budget-only policy cannot enumerate a complete
/// retain set here (a depth gate only says what to remove, not the full
/// universe to remove it from), so with **no role allowlist configured**
/// this returns `PolicyVerdict::Identity`. That is not a hole: `
/// before_model` already hid ineligible tools from the model at the source,
/// and this stage exists purely as a backstop against a provider emitting
/// calls to tools the model was never shown — a role policy is exactly the
/// kind of rule such a provider could route around, hence it alone gets
/// re-checked here.
pub(crate) fn evaluate_before_tool_batch(
    config: &ToolPolicyConfig,
    granted_roles: &[Arc<str>],
) -> PolicyVerdict {
    let Some(role_allowlist) = config.role_allowlist() else {
        return PolicyVerdict::Identity;
    };

    let mut effective_allow = role_allowlist.default_allowed().clone();
    for role in granted_roles {
        if let Some(tools) = role_allowlist.roles().get(role) {
            effective_allow.extend(tools.iter().cloned());
        }
    }

    if let Some(gate) = config.child_depth()
        && gate.current_depth() >= gate.max_depth()
    {
        for tool in gate.restricted() {
            effective_allow.remove(tool);
        }
    }

    PolicyVerdict::Retain(effective_allow)
}
