//! Pure policy evaluation core: ToolId-set narrowing rules.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{ContentBlock, MessageRole, ToolId};
use finstack_ai_runtime::{BeforeModelInput, BeforeToolBatchInput, SideEffectClass};

use crate::{
    JailbreakAction, TOOL_POLICY_JAILBREAK_TRIGGERED, TOOL_POLICY_WRITE_BUDGET_EXCEEDED,
    ToolPolicyConfig,
};

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

/// Compute the effective role-allowlist allow set, with the child-depth
/// gate's restricted set subtracted if it fires. Used by [`narrow_universe`]'s
/// role branch, which both `evaluate_before_model` and
/// `evaluate_before_tool_batch` go through.
///
/// Role allowlist: effective allow = `default_allowed ∪ ⋃(roles[r] for r in
/// granted_roles)`. Roles come from `ctx.run.authorization.roles`; unknown
/// granted roles are ignored (they contribute nothing). Returns `None` when
/// no role allowlist is configured.
///
/// Child-depth gate: if `relation_depth >= max_depth`, `restricted` is
/// subtracted from the allow set. Below the threshold (or unconfigured) →
/// no subtraction.
pub(crate) fn compute_effective_allow(
    config: &ToolPolicyConfig,
    granted_roles: &[Arc<str>],
    relation_depth: u16,
) -> Option<BTreeSet<ToolId>> {
    let role_allowlist = config.role_allowlist()?;
    let mut effective_allow = role_allowlist.default_allowed().clone();
    for role in granted_roles {
        if let Some(tools) = role_allowlist.roles().get(role) {
            effective_allow.extend(tools.iter().cloned());
        }
    }

    if let Some(gate) = config.child_depth()
        && relation_depth >= gate.max_depth()
    {
        for tool in gate.restricted() {
            effective_allow.remove(tool);
        }
    }

    Some(effective_allow)
}

/// ToolId-set rules only (role allowlist + child-depth gate), applied to a
/// known universe of visible tools. Pure; used by both stages.
///
/// Role allowlist: deny-by-default — a tool not in
/// [`compute_effective_allow`]'s result is dropped. No role rule configured
/// → no narrowing from this rule.
///
/// Child-depth gate: when no role allowlist is configured, the gate's
/// `restricted` set (if it fires) is still subtracted directly from the
/// universe — [`compute_effective_allow`] returns `None` in that case, so
/// its own gate handling never runs; this preserves depth-gate-only
/// narrowing regardless of role-allowlist configuration.
///
/// Returns `universe ∩ (rule constraints)`; leaves never "add" tools
/// (narrowing is monotone — the fold intersects anyway).
pub(crate) fn narrow_universe(
    config: &ToolPolicyConfig,
    universe: &BTreeSet<ToolId>,
    granted_roles: &[Arc<str>],
    relation_depth: u16,
) -> BTreeSet<ToolId> {
    let mut result = universe.clone();

    if let Some(effective_allow) = compute_effective_allow(config, granted_roles, relation_depth) {
        result.retain(|tool| effective_allow.contains(tool));
    } else if let Some(gate) = config.child_depth()
        && relation_depth >= gate.max_depth()
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
    relation_depth: u16,
) -> PolicyVerdict {
    let universe: BTreeSet<ToolId> = input
        .request
        .tools
        .iter()
        .map(|tool| tool.id.clone())
        .collect();
    let mut retain = narrow_universe(config, &universe, granted_roles, relation_depth);

    if let Some(budget) = config.write_budget() {
        // One pre-pass over `input.request.tools` to classify write-class
        // tools, instead of re-scanning `tools` once per call (O(calls×tools))
        // and then again to rebuild the id set.
        let mut write_model_names: BTreeSet<&str> = BTreeSet::new();
        let mut write_tool_ids: BTreeSet<ToolId> = BTreeSet::new();
        for tool in input.request.tools.iter() {
            if tool.side_effect != SideEffectClass::ReadOnly {
                write_model_names.insert(tool.model_name.as_ref());
                write_tool_ids.insert(tool.id.clone());
            }
        }

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
            .filter(|tool_name| write_model_names.contains(tool_name))
            .count();
        let write_call_count = u64::try_from(write_call_count).unwrap_or(u64::MAX);
        if write_call_count >= u64::from(budget.max_write_calls()) {
            retain.retain(|id| !write_tool_ids.contains(id));
        }
    }

    if let Some(jailbreak) = config.jailbreak() {
        'scan: for message in input.request.messages.iter() {
            let triggered = match message.role() {
                MessageRole::User => message.content().iter().any(|block| match block {
                    ContentBlock::Text(text) => contains_pattern(text.text(), jailbreak.patterns()),
                    _ => false,
                }),
                MessageRole::Tool => message.content().iter().any(|block| match block {
                    ContentBlock::ToolResult(result) => result.content().iter().any(|block| {
                        matches!(block, ContentBlock::Text(text) if contains_pattern(text.text(), jailbreak.patterns()))
                    }),
                    _ => false,
                }),
                MessageRole::System | MessageRole::Developer | MessageRole::Assistant => false,
            };
            if triggered {
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

    if retain == universe {
        PolicyVerdict::Identity
    } else {
        PolicyVerdict::Retain(retain)
    }
}

fn contains_pattern(text: &str, patterns: &[Arc<str>]) -> bool {
    let lowered = text.to_lowercase();
    patterns
        .iter()
        .any(|pattern| lowered.contains(pattern.as_ref()))
}

/// Evaluate the `before_tool_batch` defense-in-depth policy.
///
/// # Why this stage can now narrow like `before_model`
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
/// The batch stage input now carries `input.tools` — the resolved tool
/// universe visible to this run's catalog — so the leaf builds the same kind
/// of concrete universe [`evaluate_before_model`] narrows from
/// `input.request.tools`, and runs it through the identical
/// [`narrow_universe`] rules (role allowlist + child-depth gate). The
/// previous constraint (only a role allowlist can express a complete set,
/// because it alone is defined as an allow set rather than derived from a
/// universe) no longer applies: a depth-only config now narrows the real
/// carried universe too.
///
/// The write budget is authoritative at this stage: the runtime supplies the
/// count of previously committed executable write calls, and this evaluator
/// adds write-class calls in the current batch. Unknown call names are treated
/// conservatively as writes. A batch that would exceed the cap fails before
/// dispatch. Jailbreak scanning stays `before_model`-only because this input
/// deliberately carries no user/tool message text.
///
/// Returns `PolicyVerdict::Identity` when the narrowed retain set equals the
/// carried universe (keeps the fold identity-clean); otherwise
/// `PolicyVerdict::Retain`.
pub(crate) fn evaluate_before_tool_batch(
    config: &ToolPolicyConfig,
    input: &BeforeToolBatchInput,
    granted_roles: &[Arc<str>],
    relation_depth: u16,
) -> PolicyVerdict {
    let universe: BTreeSet<ToolId> = input.tools.iter().map(|tool| tool.id.clone()).collect();
    let retain = narrow_universe(config, &universe, granted_roles, relation_depth);
    if let Some(budget) = config.write_budget() {
        let read_only_names = input
            .tools
            .iter()
            .filter(|tool| tool.side_effect == SideEffectClass::ReadOnly)
            .map(|tool| tool.model_name.as_ref())
            .collect::<BTreeSet<_>>();
        let current_writes = input
            .calls
            .iter()
            .filter(|call| !read_only_names.contains(call.tool_name()))
            .count();
        let attempted = input
            .prior_write_tool_calls
            .saturating_add(u64::try_from(current_writes).unwrap_or(u64::MAX));
        if attempted > u64::from(budget.max_write_calls()) {
            return PolicyVerdict::Fail {
                reason: TOOL_POLICY_WRITE_BUDGET_EXCEEDED,
            };
        }
    }
    if retain == universe {
        PolicyVerdict::Identity
    } else {
        PolicyVerdict::Retain(retain)
    }
}
